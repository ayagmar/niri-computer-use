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
use super::steps::Plan;
use super::{environment, supervisor};
use crate::control::procs;
use crate::{Env, cli};

/// How long `desk start` waits for the desk to be ready.
const START: Duration = Duration::from_secs(90);
/// How long `desk stop` waits for the supervisor to stop the desk.
const STOP: Duration = Duration::from_secs(30);
/// How long `desk stop` waits for a killed supervisor to be gone.
const KILLED: Duration = Duration::from_secs(5);
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
    /// The supervisor, which `start` runs.
    Supervise {
        name: DeskName,
        mode: Mode,
        size: Size,
    },
    /// A desk process's first step: set the death signal and become `program`.
    Exec {
        parent: u32,
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
    if let [exec, parent, separator, program, rest @ ..] = args
        && exec == "exec"
        && separator == "--"
    {
        let parent = parent
            .to_str()
            .and_then(|pid| pid.parse().ok())
            .ok_or(USAGE)?;
        return Ok(Command::Exec {
            parent,
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
        ["supervise", name, mode, size] => Ok(Command::Supervise {
            name: DeskName::parse(name)?,
            mode: match *mode {
                "headless" => Mode::Headless,
                "windowed" => Mode::Windowed,
                _ => return Err(USAGE.to_owned()),
            },
            size: Size::parse(size)?,
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
        Command::Supervise { name, mode, size } => supervise(env, name, mode, size).await,
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
/// process it recorded. The supervisor writes the record right after it makes the
/// directory, before anything else, so a directory without one is stale.
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
    let layout = desks(env)?.desk(name)?;
    match liveness(&layout) {
        Liveness::Running(record) => {
            return Err(format!(
                "desk {name} is already running (supervisor {}); stop it with `niri-computer-use desk stop {name}`",
                record.supervisor.pid
            ));
        }
        Liveness::Stale => remove_stale(&layout)?,
        Liveness::Absent => {}
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    Programs::find(&path, &accessibility_dirs(), mode)?;
    let logs = logs(env, name)?;
    let log_path = logs.join("supervisor.log");
    let log = std::fs::File::create(&log_path)
        .map_err(|error| format!("create {}: {error}", log_path.display()))?;
    let mode_arg = match mode {
        Mode::Headless => "headless",
        Mode::Windowed => "windowed",
    };
    let args = [
        "desk",
        "supervise",
        name.as_str(),
        mode_arg,
        &size.to_string(),
    ]
    .map(str::to_owned);
    let (mut child, stdout) =
        crate::runner::desk::supervisor(&args, log).map_err(|error| error.detail)?;
    let logs = logs.display();
    let failure = match tokio::time::timeout(START, follow(name, stdout)).await {
        // The supervisor runs on, in a process group of its own.
        Ok(Ok(record)) => {
            return cli::print_json(&record).map_err(|error| format!("print the desk: {error}"));
        }
        Ok(Err(None)) => format!(
            "desk {name}'s supervisor exited before the desk was ready: {}",
            tail(&log_path)
        ),
        Ok(Err(Some(reason))) => {
            format!("desk {name} didn't start: {reason}. Its logs are in {logs}")
        }
        Err(_) => {
            ask_stop(&layout).await.ok();
            format!(
                "desk {name} wasn't ready within {START:?}, so it was stopped. Its logs are in {logs}"
            )
        }
    };
    // A failed desk's error is told once its supervisor has stopped all of it.
    if tokio::time::timeout(STOP, child.wait()).await.is_err() {
        return Err(format!(
            "{failure}; its supervisor (PID {}) is still stopping it",
            child.id().unwrap_or_default()
        ));
    }
    Err(failure)
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
                let step = serde_json::to_value(step).unwrap_or_default();
                cli::print_error(&format!(
                    "desk {name}: {}",
                    step.as_str().unwrap_or("starting")
                ));
            }
            Phase::Ready { .. } => return Ok(record),
            Phase::Failed { step, reason } => {
                let step = serde_json::to_value(step).unwrap_or_default();
                return Err(Some(format!(
                    "{} failed: {reason}",
                    step.as_str().unwrap_or("a step")
                )));
            }
            Phase::Stopping => return Err(Some("it stopped while starting".to_owned())),
        }
    }
    Err(None)
}

async fn stop(env: &Env, name: &DeskName) -> Result<(), String> {
    let layout = desks(env)?.desk(name)?;
    match liveness(&layout) {
        Liveness::Absent => Err(format!("there is no desk {name}")),
        Liveness::Stale => {
            remove_stale(&layout)?;
            cli::say(&format!(
                "desk {name} wasn't running; removed what it left in {}",
                layout.root().display()
            ));
            Ok(())
        }
        Liveness::Running(record) => {
            let asked = tokio::time::timeout(STOP, ask_stop(&layout)).await;
            if !matches!(asked, Ok(Ok(()))) || !gone(record.supervisor, KILLED).await {
                kill(record.supervisor);
                gone(record.supervisor, KILLED).await;
            }
            if layout.root().exists() {
                remove_stale(&layout)?;
            }
            cli::say(&format!("desk {name} stopped"));
            Ok(())
        }
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

/// Whether the supervisor is gone within `limit`.
async fn gone(supervisor: Supervisor, limit: Duration) -> bool {
    let end = Instant::now() + limit;
    while alive(supervisor) {
        if Instant::now() >= end {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    true
}

/// Kills the supervisor's process group; the desk's processes get `SIGTERM` from the
/// kernel as it dies.
fn kill(supervisor: Supervisor) {
    if alive(supervisor)
        && let Some(group) = i32::try_from(supervisor.pid)
            .ok()
            .and_then(rustix::process::Pid::from_raw)
    {
        rustix::process::kill_process_group(group, rustix::process::Signal::KILL).ok();
    }
}

/// Removes a desk directory no supervisor runs for.
fn remove_stale(layout: &Layout) -> Result<(), String> {
    std::fs::remove_dir_all(layout.root())
        .map_err(|error| format!("remove {}: {error}", layout.root().display()))
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

/// `desk supervise`: runs the desk until it stops.
async fn supervise(env: &Env, name: DeskName, mode: Mode, size: Size) -> Result<(), String> {
    let layout = desks(env)?.desk(&name)?;
    let path = std::env::var_os("PATH").unwrap_or_default();
    let programs = Programs::find(&path, &accessibility_dirs(), mode)?;
    let host = record::Host {
        runtime_dir: env
            .runtime_dir
            .clone()
            .ok_or("XDG_RUNTIME_DIR isn't known")?,
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
    };
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
        started: crate::control::marker::now(),
        phase: Phase::Starting {
            step: record::Step::Config,
        },
    };
    let plan = Plan {
        size,
        layout,
        logs: logs(env, &name)?,
        host,
        vars: environment::Host::read(|var| std::env::var_os(var)),
        programs,
        exe: std::env::current_exe()
            .map_err(|error| format!("find this program's file: {error}"))?,
    };
    supervisor::run(plan, name, record).await
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
            &["exec", "x", "--", "niri"],
        ] {
            assert!(parse(&args(bad)).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn exec_keeps_the_programs_arguments_as_they_are() {
        assert_eq!(
            parse(&args(&["exec", "42", "--", "niri", "-c", "--", "x"])).unwrap(),
            Command::Exec {
                parent: 42,
                program: "niri".into(),
                args: args(&["-c", "--", "x"])
            }
        );
    }
}
