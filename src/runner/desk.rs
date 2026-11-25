//! A desk's processes. The supervisor runs in a systemd scope of its own, which holds the
//! whole desk: everything it starts, and everything they start, stays in the scope's
//! cgroup, and `desk stop` stops the scope. The supervisor starts each process with an
//! exact environment, in a process group of its own, through `desk exec`, which asks the
//! kernel to signal it when the supervisor dies (`PR_SET_PDEATHSIG`) and then becomes the
//! program: a second line, so a killed supervisor's direct processes go at once. The
//! signal is `SIGKILL`, but `SIGTERM` for cage, which then removes its X11 socket (see
//! `Death`). The supervisor stops each one itself, `SIGTERM` first and with a deadline,
//! when the desk stops.

use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::process::Stdio;
use std::time::Duration;

use rustix::process::{Pid, Signal, WaitId, WaitIdOptions, kill_process_group, waitid};
use tokio::process::{Child, ChildStdout, Command};

use crate::desks::environment::Vars;
use crate::error::{ErrorName, ToolError};

/// The subcommand that sets the parent death signal and becomes a desk's program:
/// `desk exec <supervisor pid> <KILL | TERM> -- <program> <args>`.
pub(crate) const EXEC: [&str; 2] = ["desk", "exec"];
/// What puts a desk's supervisor, and so the whole desk, in a scope of its own.
pub(crate) const SYSTEMD_RUN: &str = "systemd-run";
/// How long a killed process gets to be reaped.
const REAP: Duration = Duration::from_secs(2);
/// How often a stop looks whether a process's leader has exited.
const EXIT_POLL: Duration = Duration::from_millis(20);

/// Where a desk process's stdout goes.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Output {
    /// The process's log, with its stderr.
    Log,
    /// A pipe the supervisor reads, such as `dbus-daemon --print-address`.
    Piped,
}

/// The signal a desk process gets when the supervisor dies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Death {
    Kill,
    /// For cage, which removes the X11 socket and lock it binds in `/tmp` on `SIGTERM`,
    /// and which a `SIGKILL` leaves behind. Its client, `desk hold`, exits once cage closes
    /// its display, so cage doesn't wait on it; the scope's stop sends `SIGKILL` to
    /// whatever is left after its own timeout.
    Term,
}

impl Death {
    pub(crate) const fn arg(self) -> &'static str {
        match self {
            Self::Kill => "KILL",
            Self::Term => "TERM",
        }
    }

    pub(crate) fn parse(arg: &str) -> Option<Self> {
        [Self::Kill, Self::Term]
            .into_iter()
            .find(|death| death.arg() == arg)
    }

    const fn signal(self) -> Signal {
        match self {
            Self::Kill => Signal::KILL,
            Self::Term => Signal::TERM,
        }
    }
}

/// How a desk process starts: where its stdout goes, and its death signal.
#[derive(Debug, Clone, Copy)]
pub(crate) struct How {
    pub(crate) output: Output,
    pub(crate) death: Death,
}

impl How {
    pub(crate) const LOG: Self = Self {
        output: Output::Log,
        death: Death::Kill,
    };
    pub(crate) const PIPED: Self = Self {
        output: Output::Piped,
        death: Death::Kill,
    };
}

/// A process of a desk, which its supervisor started.
#[derive(Debug)]
pub(crate) struct DeskChild {
    child: Child,
    group: Option<Pid>,
    program: String,
}

/// Starts `program` with `args` and exactly `env` for the desk's supervisor, as `how`
/// says, stderr to `log`. The supervisor's own binary sets the death signal, then becomes
/// `program`.
pub(crate) fn desk_child(
    program: &OsStr,
    args: &[OsString],
    env: &Vars,
    how: How,
    log: File,
) -> Result<DeskChild, ToolError> {
    let name = program.to_string_lossy().into_owned();
    let broken = |error: std::io::Error| {
        ToolError::new(ErrorName::UpstreamError, format!("start {name}: {error}"))
    };
    let stdout = match how.output {
        Output::Log => Stdio::from(log.try_clone().map_err(broken)?),
        Output::Piped => Stdio::piped(),
    };
    let child = command("/proc/self/exe")
        .args(EXEC)
        .arg(std::process::id().to_string())
        .arg(how.death.arg())
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

    /// Waits for the process to exit, reaps it, and says how it ended. Once reaped, or once
    /// a wait failed and its state is unknown, its group is never signalled again.
    pub(crate) async fn exited(&mut self) -> String {
        let waited = self.child.wait().await;
        self.group = None;
        match waited {
            Ok(status) => format!("{} exited with {status}", self.program),
            Err(error) => format!("wait for {} failed: {error}", self.program),
        }
    }

    /// Sends its process group `SIGTERM`, then `SIGKILL` once the leader has exited or
    /// `grace` has passed, and reaps the leader. The group is signalled only while the
    /// leader is unreaped, so its ID can't have been reused by another group; what a
    /// reaped leader's group left is the supervisor's sweep's.
    pub(crate) async fn stop(&mut self, grace: Duration) -> Result<(), String> {
        let Some(group) = self.group else {
            return Ok(());
        };
        kill_process_group(group, Signal::TERM).ok();
        if let Err(error) = self.leader_exits(grace).await {
            self.group = None;
            return Err(error);
        }
        // The leader is still unreaped: the rest of its group goes, as a SIGKILL would take it.
        kill_process_group(group, Signal::KILL).ok();
        match tokio::time::timeout(REAP, self.child.wait()).await {
            Ok(Ok(_)) => {
                self.group = None;
                Ok(())
            }
            Ok(Err(error)) => {
                self.group = None;
                Err(format!("reap {}: {error}", self.program))
            }
            Err(_) => Err(format!(
                "{} didn't exit within {REAP:?} of SIGKILL",
                self.program
            )),
        }
    }

    /// Waits up to `grace` for the leader to exit, leaving it unreaped (`WNOWAIT`). A
    /// failed wait is an error: the leader's state is unknown, so its group isn't
    /// signalled.
    async fn leader_exits(&self, grace: Duration) -> Result<(), String> {
        let pid = self
            .child
            .id()
            .and_then(|id| Pid::from_raw(id.cast_signed()))
            .ok_or_else(|| format!("{} has no process ID left", self.program))?;
        let deadline = tokio::time::Instant::now() + grace;
        let options = WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT;
        loop {
            match waitid(WaitId::Pid(pid), options) {
                Ok(Some(_)) => return Ok(()),
                Ok(None) if tokio::time::Instant::now() >= deadline => return Ok(()),
                Ok(None) => tokio::time::sleep(EXIT_POLL).await,
                Err(error) => return Err(format!("wait for {}: {error}", self.program)),
            }
        }
    }
}

/// `desk exec`: asks for `death` when the parent dies, checks the parent is still
/// `parent`, then becomes `program`. Returns only on failure, with why.
pub(crate) fn exec_dying_with(
    parent: u32,
    death: Death,
    program: &OsStr,
    args: &[OsString],
) -> String {
    use std::os::unix::process::CommandExt as _;
    if let Err(error) = die_with_parent_by(death.signal()) {
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

/// Starts the desk's supervisor through `systemd-run` with `args`, which put it in a
/// transient scope of its own and make it this binary's `desk supervise`, with the
/// session's variables `main` resolved, in a process group of its own that outlives the
/// caller. Its stdout is a pipe for its progress, its stderr, and `systemd-run`'s, `log`.
pub(crate) fn supervisor(args: &[String], log: File) -> Result<(Child, ChildStdout), ToolError> {
    let mut child = super::command(SYSTEMD_RUN)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(log)
        .process_group(0)
        .spawn()
        .map_err(|error| {
            ToolError::new(
                ErrorName::UpstreamError,
                format!("start the desk's supervisor through {SYSTEMD_RUN}: {error}"),
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

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::*;

    const DEADLINE: Duration = Duration::from_secs(5);

    /// `sh -c script` as a desk process, without `desk exec`, which the test binary isn't.
    fn shell(script: &str) -> DeskChild {
        let child = command("sh")
            .args(["-c", script])
            .stdin(Stdio::null())
            .process_group(0)
            .spawn()
            .unwrap();
        let group = child.id().and_then(|id| Pid::from_raw(id.cast_signed()));
        DeskChild {
            child,
            group,
            program: "sh".to_owned(),
        }
    }

    /// A shell that starts a `sleep` in its own group, writes its PID, then runs `then`.
    fn with_sleeper(dir: &Path, then: &str) -> DeskChild {
        shell(&format!(
            "{{ trap '' TERM; exec sleep 30; }} & echo $! > '{}'; {then}",
            dir.join("pid").display()
        ))
    }

    async fn sleeper(dir: &Path) -> PathBuf {
        let file = dir.join("pid");
        let written = || {
            std::fs::read_to_string(&file)
                .ok()
                .filter(|text| text.ends_with('\n'))
        };
        let pid = tokio::time::timeout(DEADLINE, async {
            loop {
                match written() {
                    Some(pid) => return pid,
                    None => tokio::time::sleep(Duration::from_millis(10)).await,
                }
            }
        })
        .await
        .unwrap();
        PathBuf::from(format!("/proc/{}/stat", pid.trim()))
    }

    fn alive(stat: &Path) -> bool {
        std::fs::read_to_string(stat).is_ok_and(|line| {
            line.rsplit_once(") ")
                .is_some_and(|(_, rest)| !rest.starts_with('Z'))
        })
    }

    async fn gone(stat: &Path) -> bool {
        tokio::time::timeout(DEADLINE, async {
            while alive(stat) {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .is_ok()
    }

    /// Catches a stop that reaps a leader which exited on its own and then leaves the rest
    /// of its group running, or signals the group after the reap, when its ID may belong to
    /// another group.
    #[tokio::test]
    async fn a_group_whose_leader_exited_is_killed_before_the_leader_is_reaped() {
        let dir = crate::test_support::fresh_dir("desk-leader");
        let mut child = with_sleeper(&dir, "exit 0");
        let stat = sleeper(&dir).await;
        let leader = PathBuf::from(format!("/proc/{}/stat", child.pid().unwrap()));
        while alive(&leader) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(alive(&stat), "the sleeper ignores SIGTERM and runs on");
        child.stop(Duration::from_secs(1)).await.unwrap();
        assert!(gone(&stat).await, "the leader's group outlived its stop");
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Catches a stop that signals a group whose leader was already reaped: its group ID
    /// may name an unrelated group by then.
    #[tokio::test]
    async fn a_reaped_leaders_group_is_never_signalled() {
        let dir = crate::test_support::fresh_dir("desk-reaped");
        let mut child = with_sleeper(&dir, "exit 0");
        let stat = sleeper(&dir).await;
        let said = child.exited().await;
        assert!(said.contains("exited with"), "{said}");
        child.stop(Duration::from_secs(1)).await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(alive(&stat), "a reaped leader's group was signalled");
        let pid = std::fs::read_to_string(dir.join("pid")).unwrap();
        let pid = Pid::from_raw(pid.trim().parse().unwrap()).unwrap();
        rustix::process::kill_process(pid, Signal::KILL).unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A leader that exits on `SIGTERM` takes its group with it, even a member that ignores
    /// `SIGTERM`.
    #[tokio::test]
    async fn a_stop_kills_what_ignores_sigterm_once_the_leader_exits() {
        let dir = crate::test_support::fresh_dir("desk-term");
        let mut child = with_sleeper(&dir, "trap 'exit 0' TERM; while :; do sleep 0.05; done");
        let stat = sleeper(&dir).await;
        let started = tokio::time::Instant::now();
        child.stop(Duration::from_secs(3)).await.unwrap();
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "the stop waited out its grace"
        );
        assert!(gone(&stat).await, "the group outlived its stop");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
