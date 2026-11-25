//! A desk's processes. The supervisor starts each with an exact environment, in a process
//! group of its own, through `desk exec`, which asks the kernel to send it `SIGKILL` when
//! the supervisor dies (`PR_SET_PDEATHSIG`) and then becomes the program, so a supervisor
//! that is killed takes its desk down. `SIGKILL`, because cage blocks `SIGTERM` and waits
//! for its client once it has one. The supervisor stops each one itself, `SIGTERM` first
//! and with a deadline, when the desk stops.

use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::process::Stdio;
use std::time::Duration;

use rustix::process::{Pid, Signal, kill_process_group};
use tokio::process::{Child, ChildStdout, Command};

use crate::desks::environment::Vars;
use crate::error::{ErrorName, ToolError};

/// The subcommand that sets the parent death signal and becomes a desk's program:
/// `desk exec <supervisor pid> -- <program> <args>`.
pub(crate) const EXEC: [&str; 2] = ["desk", "exec"];
/// How long a killed process gets to be reaped.
const REAP: Duration = Duration::from_secs(2);

/// Where a desk process's stdout goes.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Output {
    /// The process's log, with its stderr.
    Log,
    /// A pipe the supervisor reads, such as `dbus-daemon --print-address`.
    Piped,
}

/// A process of a desk, which its supervisor started.
#[derive(Debug)]
pub(crate) struct DeskChild {
    child: Child,
    group: Option<Pid>,
    program: String,
}

/// Starts `program` with `args` and exactly `env` for the desk's supervisor, stderr to
/// `log`. The supervisor's own binary sets the death signal, then becomes `program`.
pub(crate) fn desk_child(
    program: &OsStr,
    args: &[OsString],
    env: &Vars,
    output: Output,
    log: File,
) -> Result<DeskChild, ToolError> {
    let name = program.to_string_lossy().into_owned();
    let broken = |error: std::io::Error| {
        ToolError::new(ErrorName::UpstreamError, format!("start {name}: {error}"))
    };
    let stdout = match output {
        Output::Log => Stdio::from(log.try_clone().map_err(broken)?),
        Output::Piped => Stdio::piped(),
    };
    let child = command("/proc/self/exe")
        .args(EXEC)
        .arg(std::process::id().to_string())
        .arg("--")
        .arg(program)
        .args(args)
        .env_clear()
        .envs(env)
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(log)
        .process_group(0)
        .spawn()
        .map_err(broken)?;
    let group = child
        .id()
        .and_then(|id| i32::try_from(id).ok())
        .and_then(Pid::from_raw);
    Ok(DeskChild {
        child,
        group,
        program: name,
    })
}

impl DeskChild {
    /// The process's ID, which is the program's once `desk exec` became it.
    pub(crate) fn pid(&self) -> Option<u32> {
        self.child.id()
    }

    pub(crate) const fn take_stdout(&mut self) -> Option<ChildStdout> {
        self.child.stdout.take()
    }

    pub(crate) fn program(&self) -> &str {
        &self.program
    }

    /// Waits for the process to exit and says how it did.
    pub(crate) async fn exited(&mut self) -> String {
        match self.child.wait().await {
            Ok(status) => {
                self.group = None;
                format!("{} exited with {status}", self.program)
            }
            Err(error) => format!("wait for {}: {error}", self.program),
        }
    }

    /// Whether it has exited, without waiting.
    pub(crate) fn has_exited(&mut self) -> bool {
        !matches!(self.child.try_wait(), Ok(None))
    }

    /// Sends its process group `SIGTERM`, then `SIGKILL` after `grace`, and reaps it.
    pub(crate) async fn stop(&mut self, grace: Duration) -> Result<(), String> {
        let Some(group) = self.group.filter(|_| !self.has_exited()) else {
            return Ok(());
        };
        kill_process_group(group, Signal::TERM).ok();
        if tokio::time::timeout(grace, self.child.wait()).await.is_ok() {
            // Anything else in its group goes too, as a SIGKILL would take it.
            kill_process_group(group, Signal::KILL).ok();
            self.group = None;
            return Ok(());
        }
        kill_process_group(group, Signal::KILL).ok();
        match tokio::time::timeout(REAP, self.child.wait()).await {
            Ok(Ok(_)) => {
                self.group = None;
                Ok(())
            }
            Ok(Err(error)) => Err(format!("reap {}: {error}", self.program)),
            Err(_) => Err(format!(
                "{} didn't exit within {REAP:?} of SIGKILL",
                self.program
            )),
        }
    }
}

/// `desk exec`: asks for `SIGKILL` when the parent dies, checks the parent is still
/// `parent`, then becomes `program`. Returns only on failure, with why.
pub(crate) fn exec_dying_with(parent: u32, program: &OsStr, args: &[OsString]) -> String {
    use std::os::unix::process::CommandExt as _;
    if let Err(error) = die_with_parent_by(Signal::KILL) {
        return error;
    }
    let now =
        rustix::process::getppid().map_or(0, |pid| pid.as_raw_nonzero().get().cast_unsigned());
    if now != parent {
        return format!(
            "the desk's supervisor {parent} exited before {} started",
            program.to_string_lossy()
        );
    }
    let error = std_command(program).args(args).exec();
    format!("start {}: {error}", program.to_string_lossy())
}

/// Asks the kernel for `signal` once this process's parent exits.
pub(crate) fn die_with_parent_by(signal: Signal) -> Result<(), String> {
    rustix::process::set_parent_process_death_signal(Some(signal))
        .map_err(|error| format!("set the parent death signal: {error}"))
}

/// Starts the desk's supervisor, `args` to this binary, with the session's variables `main`
/// resolved, in a process group of its own that outlives the caller. Its stdout is a pipe
/// for its progress, its stderr `log`.
pub(crate) fn supervisor(args: &[String], log: File) -> Result<(Child, ChildStdout), ToolError> {
    let mut child = super::command("/proc/self/exe")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(log)
        .process_group(0)
        .spawn()
        .map_err(|error| {
            ToolError::new(
                ErrorName::UpstreamError,
                format!("start the desk's supervisor: {error}"),
            )
        })?;
    let stdout = child.stdout.take().ok_or_else(|| {
        ToolError::new(
            ErrorName::UpstreamError,
            "the desk's supervisor has no stdout",
        )
    })?;
    Ok((child, stdout))
}

#[expect(
    clippy::disallowed_methods,
    reason = "the runner is the one place that starts processes"
)]
fn command(program: impl AsRef<OsStr>) -> Command {
    Command::new(program)
}

#[expect(
    clippy::disallowed_methods,
    reason = "the runner is the one place that starts processes; exec replaces this one"
)]
fn std_command(program: &OsStr) -> std::process::Command {
    std::process::Command::new(program)
}
