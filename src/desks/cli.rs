//! `niri-computer-use desk start | stop | list`, and the subcommands the supervisor runs:
//! `desk supervise`, `desk exec` and `desk hold`.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::net::UnixStream;
use tokio::time::Instant;

use super::layout::{Desks, Layout};
use super::name::DeskName;
use super::programs::{ACCESSIBILITY_DIRS, Programs};
use super::record::{self, Mode, Phase, Record, Size, Supervisor};
use super::scope::{self, Generation};
use super::serial::Held;
use super::steps::Plan;
use super::{environment, supervisor};
use crate::control::procs;
use crate::runner::desk::Death;
use crate::{Env, cli};

/// How long `desk start` waits for the desk to be ready.
const START: Duration = Duration::from_secs(90);
/// How long `desk stop` may take in all: the supervisor's orderly stop, then the scope's.
const STOP: Duration = Duration::from_secs(30);
/// What of `STOP` is kept for stopping the scope and checking it is empty, however long
/// the supervisor took.
const SCOPE_STOP: Duration = Duration::from_secs(10);
/// How long `desk start` or `desk stop` waits for another of the same desk to finish.
const LOCK_WAIT: Duration = Duration::from_secs(5);
/// How much of the supervisor's log a failure quotes.
const LOG_TAIL: usize = 2048;

/// A `desk` subcommand.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Command {
    Start {
        name: DeskName,
        mode: Mode,
        size: Size,
    },
    Stop(DeskName),
    List,
    /// The supervisor, which `start` runs in the scope of `generation`.
    Supervise {
        name: DeskName,
        mode: Mode,
        size: Size,
        generation: Generation,
    },
    /// A desk process's first step: set the death signal and become `program`.
    Exec {
        parent: u32,
        death: Death,
        program: OsString,
        args: Vec<OsString>,
    },
    /// cage's client, which lives as long as cage.
    Hold,
}

pub(crate) const USAGE: &str =
    "desk start [--windowed] [--size WIDTHxHEIGHT] <name> | desk stop <name> | desk list";

/// Reads the arguments after `desk`.
pub(crate) fn parse(args: &[OsString]) -> Result<Command, String> {
    let text: Option<Vec<&str>> = args.iter().map(|arg| arg.to_str()).collect();
    if let [exec, parent, death, separator, program, rest @ ..] = args
        && exec == "exec"
        && separator == "--"
    {
        let parent = parent
            .to_str()
            .and_then(|pid| pid.parse().ok())
            .ok_or(USAGE)?;
        let death = death.to_str().and_then(Death::parse).ok_or(USAGE)?;
        return Ok(Command::Exec {
            parent,
            death,
            program: program.clone(),
            args: rest.to_vec(),
        });
    }
    let text = text.ok_or(USAGE)?;
    match text.as_slice() {
        ["start", rest @ ..] => start_args(rest),
        ["stop", name] => Ok(Command::Stop(DeskName::parse(name)?)),
        ["list"] => Ok(Command::List),
        ["hold"] => Ok(Command::Hold),
        ["supervise", name, mode, size, generation] => Ok(Command::Supervise {
            name: DeskName::parse(name)?,
            mode: match *mode {
                "headless" => Mode::Headless,
                "windowed" => Mode::Windowed,
                _ => return Err(USAGE.to_owned()),
            },
            size: Size::parse(size)?,
            generation: Generation::parse(generation)?,
        }),
        _ => Err(USAGE.to_owned()),
    }
}

fn start_args(args: &[&str]) -> Result<Command, String> {
    let (mut windowed, mut size) = (false, None);
    let mut rest = args;
    loop {
        match rest {
            ["--windowed", tail @ ..] => {
                windowed = true;
                rest = tail;
            }
            ["--size", value, tail @ ..] => {
                size = Some(Size::parse(value)?);
                rest = tail;
            }
            [name] => {
                let name = DeskName::parse(name)?;
                if windowed && size.is_some() {
                    return Err("--size sets a headless desk's output; a windowed desk's is its window's size on the host".to_owned());
                }
                let mode = if windowed {
                    Mode::Windowed
                } else {
                    Mode::Headless
                };
                return Ok(Command::Start {
                    name,
                    mode,
                    size: size.unwrap_or(Size::DEFAULT),
                });
            }
            _ => return Err(USAGE.to_owned()),
        }
    }
}

/// Runs a `desk` subcommand other than `exec` and `hold`, which `main` runs before anything
/// else.
pub(crate) async fn run(env: &Env, command: Command) -> Result<(), String> {
    match command {
        Command::Start { name, mode, size } => start(env, &name, mode, size).await,
        Command::Stop(name) => stop(env, &name).await,
        Command::List => list(env),
        Command::Supervise {
            name,
            mode,
            size,
            generation,
        } => supervise(env, name, mode, size, generation).await,
        Command::Exec { .. } | Command::Hold => Err(USAGE.to_owned()),
    }
}

/// The desks of the host session `env` describes.
pub(crate) fn desks(env: &Env) -> Result<Desks, String> {
    env.runtime_dir
        .as_deref()
        .map(Desks::of)
        .ok_or_else(|| "XDG_RUNTIME_DIR isn't known, so there is no place for desks".to_owned())
}

/// Whether a desk's supervisor runs, by its record.
#[derive(Debug)]
pub(crate) enum Liveness {
    Running(Box<Record>),
    /// The directory is there, with no supervisor for it.
    Stale,
    Absent,
}

/// Whether the desk at `layout` runs: its record names a supervisor that is still the
/// process it recorded. A directory without a record is stale: under the desk's lock, no
/// start is between making it and writing its first record.
pub(crate) fn liveness(layout: &Layout) -> Liveness {
    if !layout.root().exists() {
        return Liveness::Absent;
    }
    match record::read::<Record>(&layout.record()) {
        Ok(record) if alive(record.supervisor) => Liveness::Running(Box::new(record)),
        Ok(_) | Err(_) => Liveness::Stale,
    }
}

pub(crate) fn alive(supervisor: Supervisor) -> bool {
    procs::stat(Path::new("/proc"), supervisor.pid)
        .is_some_and(|stat| !stat.exited() && stat.start_time == supervisor.start_time)
}

async fn start(env: &Env, name: &DeskName, mode: Mode, size: Size) -> Result<(), String> {
    let desks = desks(env)?;
    let layout = desks.desk(name)?;
    let until = Instant::now() + START;
    let held = Held::take(&desks, name, LOCK_WAIT).await?;
    match liveness(&layout) {
        Liveness::Running(record) => {
            return Err(format!(
                "desk {name} is already running (supervisor {}); stop it with `niri-computer-use desk stop {name}`",
                record.supervisor.pid
            ));
        }
        Liveness::Stale => teardown(name, &layout, &held, until).await?,
        Liveness::Absent => {}
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    Programs::find(&path, &accessibility_dirs(), mode)?;
    let generation = Generation::new();
    held.set_generation(&generation)?;
    let logs = logs(env, name)?;
    let log_path = logs.join("supervisor.log");
    let log = std::fs::File::create(&log_path)
        .map_err(|error| format!("create {}: {error}", log_path.display()))?;
    let args = scope_args(name, mode, size, &generation);
    let (mut child, stdout) =
        crate::runner::desk::supervisor(&args, log).map_err(|error| error.detail)?;
    let logs = logs.display();
    let failure = match tokio::time::timeout_at(until, follow(name, stdout)).await {
        // The supervisor runs on, in its scope.
        Ok(Ok(record)) => {
            drop(held);
            return cli::print_json(&record).map_err(|error| format!("print the desk: {error}"));
        }
        Ok(Err(None)) => format!(
            "desk {name}'s supervisor exited before the desk was ready. A desk runs in a systemd user scope of its own, through `systemd-run --user --scope`; its output, and the supervisor's: {}",
            tail(&log_path)
        ),
        Ok(Err(Some(reason))) => {
            format!("desk {name} didn't start: {reason}. Its logs are in {logs}")
        }
        Err(_) => {
            tokio::time::timeout(SCOPE_STOP, ask_stop(&layout))
                .await
                .ok();
            format!("desk {name} wasn't ready within {START:?}. Its logs are in {logs}")
        }
    };
    // A failed desk's error is told once all of it has stopped and its directory is gone.
    let stop_until = Instant::now() + STOP;
    let exited = tokio::time::timeout_at(stop_until - SCOPE_STOP, child.wait())
        .await
        .is_ok_and(|waited| waited.is_ok());
    // A supervisor that exited before it made the directory started nothing.
    if exited && !layout.root().exists() {
        return Err(failure);
    }
    match teardown(name, &layout, &held, stop_until).await {
        Ok(()) => Err(failure),
        Err(left) => Err(format!("{failure}; {left}")),
    }
}

/// `systemd-run`'s arguments for the supervisor of the start `generation` of desk `name`.
fn scope_args(name: &DeskName, mode: Mode, size: Size, generation: &Generation) -> Vec<String> {
    let mode = match mode {
        Mode::Headless => "headless",
        Mode::Windowed => "windowed",
    };
    let supervise = [
        "desk",
        "supervise",
        name.as_str(),
        mode,
        &size.to_string(),
        generation.as_str(),
    ]
    .map(str::to_owned);
    // `systemd-run` becomes the supervisor: this process's own binary, even if its file
    // has been replaced since.
    let exe = format!("/proc/{}/exe", std::process::id());
    scope::run_args(&scope::unit(name, generation), name, &exe, &supervise)
}

/// Reads the supervisor's progress until the desk is ready, or why it isn't: `None` when
/// the supervisor ended without saying.
async fn follow(
    name: &DeskName,
    stdout: tokio::process::ChildStdout,
) -> Result<Record, Option<String>> {
    let mut lines = BufReader::new(stdout).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let Ok(record) = serde_json::from_str::<Record>(&line) else {
            continue;
        };
        match &record.phase {
            Phase::Starting { step } => {
                cli::print_error(&format!("desk {name}: {}", super::join::step_name(*step)));
            }
            Phase::Ready { .. } => return Ok(record),
            Phase::Failed { step, reason } => {
                return Err(Some(format!(
                    "{} failed: {reason}",
                    super::join::step_name(*step)
                )));
            }
            Phase::Stopping | Phase::Stopped { .. } => {
                return Err(Some("it stopped while starting".to_owned()));
            }
        }
    }
    Err(None)
}

/// `desk stop`: asks a running desk's supervisor to stop it, then stops its scope, checks
/// nothing is left in it, and removes its directory, all within `STOP` and under its
/// lock. Fails, keeping the directory and its record, when anything is left.
async fn stop(env: &Env, name: &DeskName) -> Result<(), String> {
    let desks = desks(env)?;
    let layout = desks.desk(name)?;
    let until = Instant::now() + STOP;
    let held = Held::take(&desks, name, LOCK_WAIT).await?;
    let running = match liveness(&layout) {
        Liveness::Absent => return Err(format!("there is no desk {name}")),
        Liveness::Stale => false,
        Liveness::Running(record) => {
            let orderly = until - SCOPE_STOP;
            tokio::time::timeout_at(orderly, ask_stop(&layout))
                .await
                .ok();
            gone(record.supervisor, orderly).await;
            true
        }
    };
    teardown(name, &layout, &held, until).await?;
    if running {
        cli::say(&format!("desk {name} stopped"));
    } else {
        cli::say(&format!(
            "desk {name} wasn't running; stopped its scope and removed what it left in {}",
            layout.root().display()
        ));
    }
    Ok(())
}

/// Stops the scope of the desk's latest start, by its record or else its lock, waits until
/// `until` for the scope to be empty, and only then removes its directory, if it still
/// belongs to that start. Fails, keeping the directory, when anything is left.
async fn teardown(
    name: &DeskName,
    layout: &Layout,
    held: &Held,
    until: Instant,
) -> Result<(), String> {
    let record = record::read::<Record>(&layout.record()).ok();
    let generation = match &record {
        Some(record) => record.generation.clone(),
        None => held.generation()?.ok_or_else(|| {
            format!(
                "desk {name}'s directory {} has no record, and no start of it is recorded, so its scope is unknown; remove the directory once nothing of the desk runs",
                layout.root().display()
            )
        })?,
    };
    let unit = scope::unit(name, &generation);
    let cgroup = record.as_ref().map(|record| record.cgroup.as_str());
    scope::stop(&unit, cgroup, until).await.map_err(|left| {
        format!(
            "desk {name} isn't fully stopped: {left}. Its directory and record stay in {}; run `niri-computer-use desk stop {name}` again",
            layout.root().display()
        )
    })?;
    remove(layout, &generation)
}

/// Removes the desk's directory if its record, when it has one, is of `generation`.
fn remove(layout: &Layout, generation: &Generation) -> Result<(), String> {
    if let Ok(record) = record::read::<Record>(&layout.record())
        && record.generation != *generation
    {
        return Err(format!(
            "{} now belongs to another start of the desk (generation {}, not {}); left as it is",
            layout.root().display(),
            record.generation.as_str(),
            generation.as_str()
        ));
    }
    match std::fs::remove_dir_all(layout.root()) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            Err(format!("remove {}: {error}", layout.root().display()))
        }
        _ => Ok(()),
    }
}

/// Asks the supervisor to stop and waits until it closes the control connection, which it
/// does as it exits.
async fn ask_stop(layout: &Layout) -> Result<(), String> {
    let path = layout.control();
    let mut stream = UnixStream::connect(&path)
        .await
        .map_err(|error| format!("connect to {}: {error}", path.display()))?;
    stream
        .write_all(format!("{}\n", supervisor::STOP).as_bytes())
        .await
        .map_err(|error| format!("write to {}: {error}", path.display()))?;
    let mut rest = Vec::new();
    stream.read_to_end(&mut rest).await.ok();
    Ok(())
}

/// Waits until the supervisor is gone, or `until`.
async fn gone(supervisor: Supervisor, until: Instant) {
    while alive(supervisor) && Instant::now() < until {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn list(env: &Env) -> Result<(), String> {
    let desks = desks(env)?;
    let names = desks
        .names()
        .map_err(|error| format!("read {}: {error}", desks.path().display()))?;
    let listed: Vec<serde_json::Value> = names
        .iter()
        .map(|name| {
            let layout = desks.desk(name);
            let record = layout
                .as_ref()
                .ok()
                .and_then(|layout| record::read::<Record>(&layout.record()).ok());
            let running = record
                .as_ref()
                .is_some_and(|record| alive(record.supervisor));
            serde_json::json!({ "name": name, "running": running, "record": record })
        })
        .collect();
    cli::print_json(&listed).map_err(|error| format!("print the desks: {error}"))
}

/// `desk supervise`: runs the desk until it stops, once it has checked it runs in the
/// scope of `generation`.
async fn supervise(
    env: &Env,
    name: DeskName,
    mode: Mode,
    size: Size,
    generation: Generation,
) -> Result<(), String> {
    let layout = desks(env)?.desk(&name)?;
    let cgroup = own_scope(&scope::unit(&name, &generation))?;
    let path = std::env::var_os("PATH").unwrap_or_default();
    let programs = Programs::find(&path, &accessibility_dirs(), mode)?;
    let me = std::process::id();
    let start_time = procs::stat(Path::new("/proc"), me)
        .map(|stat| stat.start_time)
        .ok_or("read the supervisor's own /proc entry")?;
    let record = Record {
        name: name.clone(),
        mode,
        size: (mode == Mode::Headless).then_some(size),
        supervisor: Supervisor {
            pid: me,
            start_time,
        },
        generation,
        cgroup,
        started: crate::control::marker::now(),
        phase: Phase::Starting {
            step: record::Step::Config,
        },
    };
    let plan = Plan {
        size,
        layout,
        logs: logs(env, &name)?,
        host: host(env).await?,
        vars: environment::Host::read(|var| std::env::var_os(var)),
        programs,
        exe: std::env::current_exe()
            .map_err(|error| format!("find this program's file: {error}"))?,
    };
    supervisor::run(plan, record).await
}

/// The supervisor's cgroup, once it is checked to be the scope `unit`'s.
fn own_scope(unit: &str) -> Result<String, String> {
    let text = std::fs::read_to_string("/proc/self/cgroup")
        .map_err(|error| format!("read /proc/self/cgroup: {error}"))?;
    let cgroup = scope::cgroup_of(&text).ok_or(
        "the supervisor is in no cgroup of the unified hierarchy, so a desk can't be contained",
    )?;
    if !scope::is_unit(&cgroup, unit) {
        return Err(format!(
            "the supervisor runs in {cgroup}, not in its scope {unit}; `desk start` runs it through `systemd-run --user --scope`"
        ));
    }
    Ok(cgroup)
}

/// What the desk's server takes from the host session `env` describes.
async fn host(env: &Env) -> Result<record::Host, String> {
    Ok(record::Host {
        runtime_dir: env.runtime_dir.clone().unwrap_or_default(),
        niri_socket: env.instance.socket().ok().map(Path::to_path_buf),
        wayland_display: env
            .wayland_display
            .as_ref()
            .and_then(|display| display.to_str())
            .map(str::to_owned),
        session_bus: env
            .session_bus
            .as_ref()
            .and_then(|bus| bus.to_str())
            .map(str::to_owned),
        lock: crate::control::host_lock_sources(env).await?,
    })
}

fn accessibility_dirs() -> Vec<&'static Path> {
    ACCESSIBILITY_DIRS.iter().map(Path::new).collect()
}

/// The desk's log directory, under the host's state directory, made as needed.
fn logs(env: &Env, name: &DeskName) -> Result<PathBuf, String> {
    let state = env
        .state_dir
        .as_ref()
        .ok_or("neither XDG_STATE_HOME nor HOME is set, so the desk has nowhere for its logs")?;
    let dir = state
        .join("niri-computer-use")
        .join("desks")
        .join(name.as_str());
    std::fs::create_dir_all(&dir).map_err(|error| format!("create {}: {error}", dir.display()))?;
    Ok(dir)
}

/// The end of the log at `path`, for a failure that has nothing better to say.
fn tail(path: &Path) -> String {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let start = text.len().saturating_sub(LOG_TAIL);
    let start = (start..text.len())
        .find(|&index| text.is_char_boundary(index))
        .unwrap_or(text.len());
    let tail = text.get(start..).unwrap_or_default().trim();
    if tail.is_empty() {
        format!("its log {} is empty", path.display())
    } else {
        tail.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
    }

    #[test]
    fn start_takes_its_flags_before_the_name() {
        assert_eq!(
            parse(&args(&["start", "web"])).unwrap(),
            Command::Start {
                name: DeskName::parse("web").unwrap(),
                mode: Mode::Headless,
                size: Size::DEFAULT
            }
        );
        assert_eq!(
            parse(&args(&["start", "--size", "1280x720", "web"])).unwrap(),
            Command::Start {
                name: DeskName::parse("web").unwrap(),
                mode: Mode::Headless,
                size: Size {
                    width: 1280,
                    height: 720
                }
            }
        );
        assert!(matches!(
            parse(&args(&["start", "--windowed", "web"])).unwrap(),
            Command::Start {
                mode: Mode::Windowed,
                ..
            }
        ));
        for bad in [
            &["start"][..],
            &["start", "web", "--windowed"],
            &["start", "--windowed", "--size", "1280x720", "web"],
            &["start", "--size", "web"],
            &["start", "Web"],
            &["stop"],
            &["stop", "-x"],
            &["list", "all"],
            &["exec", "x", "KILL", "--", "niri"],
            &["exec", "42", "--", "niri"],
            &["exec", "42", "HUP", "--", "niri"],
        ] {
            assert!(parse(&args(bad)).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn exec_keeps_the_programs_arguments_as_they_are() {
        assert_eq!(
            parse(&args(&[
                "exec", "42", "TERM", "--", "niri", "-c", "--", "x"
            ]))
            .unwrap(),
            Command::Exec {
                parent: 42,
                death: Death::Term,
                program: "niri".into(),
                args: args(&["-c", "--", "x"])
            }
        );
    }

    fn record(generation: &str) -> Record {
        let me = std::process::id();
        Record {
            name: DeskName::parse("web").unwrap(),
            mode: Mode::Headless,
            size: None,
            supervisor: Supervisor {
                pid: me,
                start_time: procs::stat(Path::new("/proc"), me).unwrap().start_time,
            },
            generation: Generation::parse(generation).unwrap(),
            cgroup: "/app.slice/ncu-desk-web-1.scope".to_owned(),
            started: String::new(),
            phase: Phase::Starting {
                step: record::Step::Config,
            },
        }
    }

    /// A start, as far as the lock and the directory go: takes the lock, makes the
    /// directory, says so on `made`, waits for `go`, then writes the record and lets go.
    async fn start_between(
        desks: Desks,
        made: tokio::sync::mpsc::Sender<()>,
        go: impl Future<Output = ()>,
    ) {
        let name = DeskName::parse("web").unwrap();
        let layout = desks.desk(&name).unwrap();
        let held = Held::take(&desks, &name, Duration::ZERO).await.unwrap();
        std::fs::create_dir_all(layout.root()).unwrap();
        made.send(()).await.unwrap();
        go.await;
        record::write(&layout.record(), &record("2")).unwrap();
        drop(held);
    }

    /// Catches a stop that, in the moment between a start making the desk's directory and
    /// writing its record, takes the directory for stale and removes it under a live
    /// supervisor: the start holds the desk's lock across that moment, and the stop finds
    /// the desk running once it gets the lock.
    #[tokio::test]
    async fn a_stop_waits_for_a_start_to_publish_and_then_sees_it_running() {
        let dir = crate::test_support::short_dir("race");
        let desks = Desks::of(&dir);
        let name = DeskName::parse("web").unwrap();
        let layout = desks.desk(&name).unwrap();
        let (made, mut made_seen) = tokio::sync::mpsc::channel(1);
        let (go, mut go_seen) = tokio::sync::mpsc::channel::<()>(1);
        let starting = tokio::spawn(start_between(desks.clone(), made, async move {
            go_seen.recv().await.unwrap();
        }));
        made_seen.recv().await.unwrap();
        assert!(matches!(liveness(&layout), Liveness::Stale));
        let refused = Held::take(&desks, &name, Duration::from_millis(200)).await;
        assert!(refused.is_err());
        assert!(layout.root().exists());
        go.send(()).await.unwrap();
        let held = Held::take(&desks, &name, Duration::from_secs(5))
            .await
            .unwrap();
        starting.await.unwrap();
        assert!(matches!(liveness(&layout), Liveness::Running(_)));
        drop(held);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Catches a stop that, finishing after a restart, removes the new desk's directory:
    /// a directory goes only when its record is of the start whose scope was stopped.
    #[test]
    fn a_directory_goes_only_with_the_generation_that_was_stopped() {
        let dir = crate::test_support::short_dir("restart");
        let layout = Desks::of(&dir)
            .desk(&DeskName::parse("web").unwrap())
            .unwrap();
        std::fs::create_dir_all(layout.root()).unwrap();
        record::write(&layout.record(), &record("2")).unwrap();
        let old = Generation::parse("1").unwrap();
        let refused = remove(&layout, &old).unwrap_err();
        assert!(refused.contains("another start"), "{refused}");
        assert!(layout.record().exists());
        remove(&layout, &Generation::parse("2").unwrap()).unwrap();
        assert!(!layout.root().exists());
        // A directory without a record is the latest start's, the one the lock names.
        std::fs::create_dir_all(layout.root()).unwrap();
        remove(&layout, &old).unwrap();
        assert!(!layout.root().exists());
        remove(&layout, &old).unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }
}
