//! The `watch` loop: polls the runtime directory and follows the audit log, writing a
//! line whenever something changed.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::io::{AsyncWrite, AsyncWriteExt as _};

use super::tail::Tail;
use super::{BACKLOG, Driver, Event, FORMAT, State, action, clients, label_pid};
use crate::control::lease;
use crate::control::runtime::RuntimeDir;

/// How often the desk is looked at.
const POLL: Duration = Duration::from_millis(250);

/// How often `alive` is sent.
const ALIVE: Duration = Duration::from_secs(5);

/// How long a line may take to write before the reader counts as stuck.
const WRITE_DEADLINE: Duration = Duration::from_secs(5);

/// The longest line Noctalia's stream reader passes on whole, without its newline. It
/// cuts a longer one, and its tail would reach the plugin as a line of its own.
const READER_LIMIT: usize = 64 * 1024;

/// What one `watch` follows.
pub(crate) struct Watched {
    pub(crate) runtime: RuntimeDir,
    /// The niri socket's file name, as the audit log names the instance.
    pub(crate) instance: String,
    pub(crate) audit: Option<PathBuf>,
    pub(crate) proc_root: PathBuf,
}

/// Runs `watch` for the instance `env` resolved, until stdout's reader goes away, or ends
/// the process once it stops reading.
pub(crate) async fn run(env: &crate::Env) -> Result<(), String> {
    let watched = Watched {
        runtime: RuntimeDir::of(env)?,
        instance: env
            .instance()
            .ok_or("no niri socket, so there is no instance to watch")?,
        audit: crate::audit::Audit::new(env.state_dir.clone()).path(),
        proc_root: PathBuf::from("/proc"),
    };
    let Err(ended) = follow(&watched, tokio::io::stdout(), POLL).await;
    match ended {
        Ended::ReaderGone => Ok(()),
        Ended::Write(error) => Err(format!("write to stdout: {error}")),
        Ended::Stuck => crate::cli::exit_at_once(format!(
            "stdout's reader took more than {} s to read a line",
            WRITE_DEADLINE.as_secs()
        )),
    }
}

/// Why the loop ended; it runs until its output can't be written.
#[derive(Debug)]
pub(crate) enum Ended {
    ReaderGone,
    Write(std::io::Error),
    Stuck,
}

pub(crate) async fn follow(
    watched: &Watched,
    out: impl AsyncWrite + Unpin,
    poll: Duration,
) -> Result<std::convert::Infallible, Ended> {
    let mut out = Out { out };
    let mut projects = Projects::new(&watched.proc_root, &watched.instance);
    out.send(&Event::Hello {
        format: FORMAT,
        version: env!("CARGO_PKG_VERSION"),
        instance: &watched.instance,
    })
    .await?;
    let (mut tail, recent) = watched.audit.as_ref().map_or_else(
        || (None, Vec::new()),
        |path| {
            let (tail, recent) = Tail::open(path.clone(), BACKLOG);
            (Some(tail), recent)
        },
    );
    let mut state = None;
    let mut alive = tokio::time::interval(ALIVE);
    alive.tick().await;
    let mut tick = tokio::time::interval(poll);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut lines = recent;
    // The first lines are the backlog: calls made before `watch` started.
    let mut backlog = true;
    loop {
        let now = desk(watched, &mut projects);
        if state.as_ref() != Some(&now) {
            out.send(&Event::State(&now)).await?;
            state = Some(now);
        }
        for line in std::mem::take(&mut lines) {
            if let Some(call) = action(&line, &watched.instance) {
                out.send(&Event::Action(projects.annotate(call, backlog)))
                    .await?;
            }
        }
        backlog = false;
        tokio::select! {
            _ = tick.tick() => {}
            _ = alive.tick() => out.send(&Event::Alive).await?,
        }
        lines = tail
            .as_mut()
            .and_then(|tail| tail.poll().ok())
            .unwrap_or_default();
    }
}

fn desk(watched: &Watched, projects: &mut Projects<'_>) -> State {
    let runtime = &watched.runtime;
    State {
        lease: lease::holder(runtime).map(|holder| {
            let project = projects.of(&holder.label);
            Driver::new(holder, project)
        }),
        stopped: runtime.stopped().unwrap_or(false),
        input_dirty: runtime.input_dirty().unwrap_or(false),
        clients: clients::scan(&watched.proc_root, &watched.instance),
    }
}

struct Out<W> {
    out: W,
}

impl<W: AsyncWrite + Unpin> Out<W> {
    async fn send(&mut self, event: &Event<'_>) -> Result<(), Ended> {
        let mut line = serde_json::to_vec(event).map_err(|error| Ended::Write(error.into()))?;
        // `project` and `backlog` can take a call the audit log fits past the limit.
        if line.len() > READER_LIMIT && matches!(event, Event::Action(_)) {
            return Ok(());
        }
        line.push(b'\n');
        let written = async {
            self.out.write_all(&line).await?;
            self.out.flush().await
        };
        match tokio::time::timeout(WRITE_DEADLINE, written).await {
            Err(_) => Err(Ended::Stuck),
            Ok(Err(error)) if error.kind() == std::io::ErrorKind::BrokenPipe => {
                Err(Ended::ReaderGone)
            }
            Ok(Err(error)) => Err(Ended::Write(error)),
            Ok(Ok(())) => Ok(()),
        }
    }
}

/// The project each session's agent runs in, looked up once per session.
struct Projects<'a> {
    proc_root: &'a Path,
    instance: &'a str,
    known: HashMap<String, Option<String>>,
}

impl<'a> Projects<'a> {
    fn new(proc_root: &'a Path, instance: &'a str) -> Self {
        Self {
            proc_root,
            instance,
            known: HashMap::new(),
        }
    }

    /// The last part of the working directory of the process that started the session's
    /// server, while that server runs. A PID another program took since names none.
    fn of(&mut self, session: &str) -> Option<String> {
        if let Some(known) = self.known.get(session) {
            return known.clone();
        }
        let project = self.look_up(session);
        if project.is_some() {
            self.known.insert(session.to_owned(), project.clone());
        }
        project
    }

    fn look_up(&self, session: &str) -> Option<String> {
        clients::client(self.proc_root, label_pid(session)?, self.instance)?.project
    }

    /// Adds the call's `project`, and `backlog: true` to a call made before `watch` began.
    fn annotate(&mut self, mut call: serde_json::Value, backlog: bool) -> serde_json::Value {
        let session = call
            .get("session")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned);
        let project = session.and_then(|session| self.of(&session));
        if let Some(object) = call.as_object_mut() {
            object.insert("project".into(), project.into());
            if backlog {
                object.insert("backlog".into(), true.into());
            }
        }
        call
    }
}

#[cfg(test)]
mod tests {
    use tokio::io::AsyncBufReadExt as _;

    use super::*;

    type Reader = tokio::io::Lines<tokio::io::BufReader<tokio::io::DuplexStream>>;

    async fn next(lines: &mut Reader) -> serde_json::Value {
        let line = tokio::time::timeout(Duration::from_secs(5), lines.next_line())
            .await
            .expect("a line within 5 s")
            .unwrap()
            .expect("the watch still runs");
        serde_json::from_str(&line).unwrap()
    }

    /// The next line that isn't `alive`.
    async fn event(lines: &mut Reader) -> serde_json::Value {
        loop {
            let line = next(lines).await;
            if line["event"] != "alive" {
                return line;
            }
        }
    }

    /// A process `pid` running `exe` as `<program> <command>`, started by `parent`.
    fn fake_process(root: &Path, pid: u32, parent: u32, exe: &str, command: &str) {
        let dir = root.join(pid.to_string());
        std::fs::create_dir_all(&dir).unwrap();
        std::os::unix::fs::symlink(exe, dir.join("exe")).unwrap();
        std::fs::write(dir.join("cmdline"), format!("program\0{command}\0")).unwrap();
        std::fs::write(dir.join("environ"), "NIRI_SOCKET=/run/niri.sock\0").unwrap();
        let status = format!("Pid:\t{pid}\nPPid:\t{parent}\n");
        std::fs::write(dir.join("status"), status).unwrap();
    }

    /// A fake `/proc` where the server `4711` was started by an agent in `~/work/shop`,
    /// and `4800`, a server's PID once, is now a shell started by that agent.
    fn fake_proc(dir: &Path) -> PathBuf {
        let root = dir.join("proc");
        fake_process(&root, 4711, 4100, "/bin/niri-computer-use", "serve");
        fake_process(&root, 4800, 4100, "/bin/bash", "-c");
        std::fs::create_dir_all(root.join("4100")).unwrap();
        std::os::unix::fs::symlink(dir.join("work/shop"), root.join("4100/cwd")).unwrap();
        root
    }

    fn watched(dir: &Path) -> Watched {
        Watched {
            runtime: RuntimeDir::of(&crate::test_support::niri_env(dir)).unwrap(),
            instance: "niri.sock".into(),
            audit: Some(dir.join("audit.jsonl")),
            proc_root: fake_proc(dir),
        }
    }

    fn record(dir: &Path, line: &str) {
        use std::io::Write as _;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("audit.jsonl"))
            .unwrap();
        writeln!(file, "{line}").unwrap();
    }

    #[tokio::test]
    async fn reports_the_desk_and_this_instances_calls_as_they_change() {
        let dir = crate::test_support::fresh_dir("watch-follow");
        let watched = watched(&dir);
        watched.runtime.create().unwrap();
        record(
            &dir,
            r#"{"session":"pi/9","instance":"niri.sock","tool":"status"}"#,
        );
        let (writer, reader) = tokio::io::duplex(64 * 1024);
        let watch = tokio::spawn(async move {
            let Err(_) = follow(&watched, writer, Duration::from_millis(10)).await;
        });
        let mut lines = tokio::io::BufReader::new(reader).lines();
        let hello = event(&mut lines).await;
        assert_eq!(
            (hello["event"].as_str(), hello["format"].as_u64()),
            (Some("hello"), Some(1))
        );
        let idle = event(&mut lines).await;
        assert_eq!(idle["event"], "state");
        assert!(
            idle["lease"].is_null() && idle["stopped"] == false,
            "{idle}"
        );
        let old = event(&mut lines).await;
        assert_eq!(
            (old["tool"].as_str(), old["backlog"].as_bool()),
            (Some("status"), Some(true))
        );

        let runtime = RuntimeDir::of(&crate::test_support::niri_env(&dir)).unwrap();
        let mut held = lease::Lease::acquire(&runtime, "claude-code/4711").unwrap();
        held.publish().unwrap();
        let driving = event(&mut lines).await;
        assert_eq!(driving["lease"]["session"], "claude-code/4711", "{driving}");
        assert_eq!(driving["lease"]["project"], "shop", "{driving}");
        assert_eq!(driving["lease"]["pending"], false, "{driving}");

        record(
            &dir,
            r#"{"session":"claude-code/4711","instance":"other.sock","tool":"key"}"#,
        );
        record(
            &dir,
            r#"{"session":"claude-code/4711","instance":"niri.sock","tool":"click"}"#,
        );
        let click = event(&mut lines).await;
        assert_eq!(
            (click["tool"].as_str(), click["project"].as_str()),
            (Some("click"), Some("shop"))
        );
        assert!(click.get("backlog").is_none(), "{click}");

        runtime.stop().unwrap();
        assert_eq!(event(&mut lines).await["stopped"], true);
        drop(held);
        assert!(event(&mut lines).await["lease"].is_null());
        watch.abort();
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn skips_a_call_whose_line_is_longer_than_noctalia_reads() {
        let dir = crate::test_support::fresh_dir("watch-long");
        let watched = watched(&dir);
        watched.runtime.create().unwrap();
        let head =
            r#"{"session":"claude-code/4711","instance":"niri.sock","tool":"type_text","error":""#;
        let pad = super::super::LINE_LIMIT - head.len() - 2;
        record(&dir, &format!(r#"{head}{}"}}"#, "x".repeat(pad)));
        record(
            &dir,
            r#"{"session":"pi/9","instance":"niri.sock","tool":"status"}"#,
        );
        let (writer, reader) = tokio::io::duplex(256 * 1024);
        let watch = tokio::spawn(async move {
            let Err(_) = follow(&watched, writer, Duration::from_millis(10)).await;
        });
        let mut lines = tokio::io::BufReader::new(reader).lines();
        assert_eq!(event(&mut lines).await["event"], "hello");
        assert_eq!(event(&mut lines).await["event"], "state");
        assert_eq!(event(&mut lines).await["tool"], "status");
        watch.abort();
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_session_whose_pid_now_runs_another_program_has_no_project() {
        let dir = crate::test_support::fresh_dir("watch-reused");
        let watched = watched(&dir);
        let mut projects = Projects::new(&watched.proc_root, &watched.instance);
        assert_eq!(projects.of("claude-code/4711").as_deref(), Some("shop"));
        assert_eq!(projects.of("pi/4800"), None);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn ends_when_the_reader_goes_away() {
        let dir = crate::test_support::fresh_dir("watch-gone");
        let watched = watched(&dir);
        let (writer, reader) = tokio::io::duplex(64);
        drop(reader);
        let ended = follow(&watched, writer, Duration::from_millis(10)).await;
        assert!(matches!(ended, Err(Ended::ReaderGone)), "{ended:?}");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
