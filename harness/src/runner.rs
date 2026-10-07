//! The only place the harness starts processes. Every child has a deadline. A child in its
//! own process group has the whole group killed when it exits, times out or the harness is
//! interrupted, so nothing it started outlives it unless it left the group. The child is
//! not reaped until after that kill, so its process ID, and with it the group ID, can't
//! have been reused.

use std::ffi::OsString;
use std::fs::File;
use std::io::{self, Read};
use std::os::unix::process::CommandExt as _;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use rustix::io::Errno;
use rustix::process::{
    Pid, Signal, WaitId, WaitIdOptions, kill_process, kill_process_group, waitid,
};

use crate::environment::Env;
use crate::failure::{Context as _, Failure, Result};
use crate::interrupt;

/// How often a wait checks for Ctrl+C.
const POLL: Duration = Duration::from_millis(100);
/// How long to wait for captured output, both streams together, once the child is gone.
const OUTPUT_GRACE: Duration = Duration::from_secs(2);
/// How long a killed child may take to exit before the runner gives up on it.
const KILL_GRACE: Duration = Duration::from_secs(2);

#[derive(Debug)]
pub(crate) enum ChildEnv<'a> {
    /// The harness's own environment: the user's shell for `run`, NESTED for `supervise`.
    Inherit,
    /// Exactly these variables and nothing else.
    Exact(&'a Env),
}

#[derive(Debug)]
pub(crate) enum Sink {
    /// Collect stdout and stderr in memory.
    Capture,
    /// Append stdout and stderr to this file.
    File(PathBuf),
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum Group {
    /// A new process group, killed as a whole when the child is done.
    Own,
    /// The caller's process group, so whoever kills that group also kills this child.
    /// Only the child itself is killed on timeout.
    Caller,
}

#[derive(Debug)]
pub(crate) struct Invocation<'a> {
    pub(crate) program: &'a str,
    pub(crate) args: Vec<OsString>,
    pub(crate) env: ChildEnv<'a>,
    pub(crate) output: Sink,
    pub(crate) group: Group,
    pub(crate) deadline: Duration,
}

#[derive(Debug, PartialEq, Eq)]
enum Ending {
    Exited,
    TimedOut,
    Interrupted,
}

/// Runs a program to completion and requires exit status 0. Errors keep the exit status
/// and stderr, or name the file the output went to.
pub(crate) fn run(invocation: &Invocation<'_>) -> Result<Output> {
    let program = invocation.program;
    if interrupt::requested() {
        return Err(Failure::new(format!(
            "interrupted before starting {program}"
        )));
    }
    let mut child = command(invocation)?
        .spawn()
        .context(format!("start {program}"))?;
    let pid = Pid::from_child(&child);
    let stdout = read_in_background(child.stdout.take());
    let stderr = read_in_background(child.stderr.take());
    let exit = observe_exit(pid);

    let ending = wait_for_exit(&exit, invocation.deadline);
    let exited = matches!(ending, Ok(Ending::Exited));
    kill(pid, invocation.group, exited).context(format!("kill {program}"))?;
    // An observer failure ends here, killed but not reaped: its one message is used up.
    let ending = ending.context(format!("wait for {program}"))?;
    if !exited {
        confirm_exit(&exit).context(format!("stop {program}"))?;
    }
    let status = child.wait().context(format!("reap {program}"))?;
    let output_end = Instant::now() + OUTPUT_GRACE;
    let stdout = collect(stdout, output_end).context(format!("read the output of {program}"));
    let stderr = collect(stderr, output_end).context(format!("read the output of {program}"));
    if ending == Ending::Exited && status.success() {
        return Ok(Output {
            status,
            stdout: stdout?,
            stderr: stderr?,
        });
    }
    let detail = detail(invocation, stderr);
    Err(Failure::new(match ending {
        Ending::Exited => format!("{program} failed with {status}{detail}"),
        Ending::TimedOut => format!(
            "{program} did not finish within {:?}{detail}",
            invocation.deadline
        ),
        Ending::Interrupted => format!("interrupted while running {program}"),
    }))
}

#[expect(
    clippy::disallowed_methods,
    reason = "this is the runner every other module goes through"
)]
fn command(invocation: &Invocation<'_>) -> Result<Command> {
    let mut command = Command::new(invocation.program);
    command.args(&invocation.args).stdin(Stdio::null());
    if matches!(invocation.group, Group::Own) {
        command.process_group(0);
    }
    if let ChildEnv::Exact(env) = invocation.env {
        command.env_clear().envs(env);
    }
    match &invocation.output {
        Sink::Capture => command.stdout(Stdio::piped()).stderr(Stdio::piped()),
        Sink::File(path) => {
            let file = File::options()
                .create(true)
                .append(true)
                .open(path)
                .context(format!("open {}", path.display()))?;
            let copy = file.try_clone().context("duplicate log file")?;
            command.stdout(file).stderr(copy)
        }
    };
    Ok(command)
}

/// Reports, once, when the child has exited, without reaping it.
fn observe_exit(pid: Pid) -> Receiver<io::Result<()>> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let options = WaitIdOptions::EXITED | WaitIdOptions::NOWAIT;
        let waited = loop {
            match waitid(WaitId::Pid(pid), options) {
                Err(Errno::INTR) => {}
                other => break other.map(drop).map_err(io::Error::from),
            }
        };
        sender.send(waited)
    });
    receiver
}

fn wait_for_exit(exit: &Receiver<io::Result<()>>, deadline: Duration) -> io::Result<Ending> {
    let end = Instant::now() + deadline;
    loop {
        if interrupt::requested() {
            return Ok(Ending::Interrupted);
        }
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Ok(Ending::TimedOut);
        }
        match exit.recv_timeout(left.min(POLL)) {
            Ok(waited) => return waited.map(|()| Ending::Exited),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                return Err(io::Error::other("the exit observer stopped"));
            }
        }
    }
}

/// After a kill: wait a bounded time for the exit. Without it the child isn't reaped.
fn confirm_exit(exit: &Receiver<io::Result<()>>) -> Result<()> {
    match exit.recv_timeout(KILL_GRACE) {
        Ok(waited) => waited.context("wait for exit"),
        Err(RecvTimeoutError::Timeout) => Err(Failure::new(format!(
            "still running {KILL_GRACE:?} after SIGKILL; not reaped"
        ))),
        Err(RecvTimeoutError::Disconnected) => {
            Err(Failure::new("the exit observer stopped; not reaped"))
        }
    }
}

fn kill(pid: Pid, group: Group, exited: bool) -> rustix::io::Result<()> {
    let killed = match group {
        Group::Own => kill_process_group(pid, Signal::KILL),
        Group::Caller if exited => Ok(()),
        Group::Caller => kill_process(pid, Signal::KILL),
    };
    match killed {
        // The group is already empty.
        Err(Errno::SRCH) => Ok(()),
        other => other,
    }
}

fn read_in_background(
    pipe: Option<impl Read + Send + 'static>,
) -> Option<Receiver<io::Result<Vec<u8>>>> {
    let mut pipe = pipe?;
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut bytes = Vec::new();
        sender.send(pipe.read_to_end(&mut bytes).map(|_| bytes))
    });
    Some(receiver)
}

/// A process that left the group can keep the pipe open forever, so this waits only
/// until `end`.
fn collect(output: Option<Receiver<io::Result<Vec<u8>>>>, end: Instant) -> Result<Vec<u8>> {
    let Some(receiver) = output else {
        return Ok(Vec::new());
    };
    let left = end.saturating_duration_since(Instant::now());
    receiver.recv_timeout(left).map_or_else(
        |_| {
            Err(Failure::new(format!(
                "still open {OUTPUT_GRACE:?} after the process ended; \
                 a descendant left its process group"
            )))
        },
        |bytes| bytes.context("read pipe"),
    )
}

fn detail(invocation: &Invocation<'_>, stderr: Result<Vec<u8>>) -> String {
    if let Sink::File(path) = &invocation.output {
        return format!("; output in {}", path.display());
    }
    match stderr {
        Ok(bytes) => {
            let text = String::from_utf8_lossy(&bytes);
            let text = text.trim();
            if text.is_empty() {
                String::new()
            } else {
                format!(": {text}")
            }
        }
        Err(failure) => format!("; {failure}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn invocation(program: &'static str, args: &[&str], deadline: Duration) -> Invocation<'static> {
        Invocation {
            program,
            args: args.iter().map(OsString::from).collect(),
            env: ChildEnv::Inherit,
            output: Sink::Capture,
            group: Group::Own,
            deadline,
        }
    }

    /// A killed process stays visible until whoever inherited it reaps it, so this
    /// allows up to two seconds.
    fn dies_soon(pid: &str) -> bool {
        let pid = Pid::from_raw(pid.trim().parse().unwrap()).unwrap();
        let end = Instant::now() + Duration::from_secs(2);
        while Instant::now() < end {
            if kill_process(pid, Signal::CONT).is_err() {
                return true;
            }
            thread::park_timeout(Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn captures_stdout() {
        let output = run(&invocation("echo", &["hi"], Duration::from_secs(5))).unwrap();
        assert_eq!(output.stdout, b"hi\n");
    }

    #[test]
    fn failure_keeps_status_and_stderr() {
        let call = invocation(
            "sh",
            &["-c", "echo broke >&2; exit 3"],
            Duration::from_secs(5),
        );
        let message = run(&call).unwrap_err().to_string();
        assert!(message.contains("exit status: 3"), "{message}");
        assert!(message.contains("broke"), "{message}");
    }

    #[test]
    fn exit_kills_what_the_child_left_in_its_group() {
        let call = invocation(
            "sh",
            &["-c", "sleep 30 >/dev/null & echo $!"],
            Duration::from_secs(5),
        );
        let output = run(&call).unwrap();
        assert!(dies_soon(&String::from_utf8_lossy(&output.stdout)));
    }

    #[test]
    fn deadline_kills_the_whole_group() {
        let started = Instant::now();
        let call = invocation(
            "sh",
            &["-c", "sleep 30 & sleep 30"],
            Duration::from_millis(200),
        );
        let message = run(&call).unwrap_err().to_string();
        assert!(message.contains("did not finish"), "{message}");
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn output_held_by_an_escaped_descendant_is_bounded() {
        let started = Instant::now();
        let pid_file = std::env::temp_dir().join(format!("harness-escaped-{}", std::process::id()));
        // The trailing sleep gives `setsid` time to leave the group before the shell exits.
        let script = format!(
            "setsid sleep 4 & echo $! > {}; sleep 0.3",
            pid_file.display()
        );
        let call = invocation("sh", &["-c", &script], Duration::from_secs(5));
        let message = run(&call).unwrap_err().to_string();
        assert!(message.contains("left its process group"), "{message}");
        assert!(started.elapsed() < Duration::from_secs(4));
        let escaped = std::fs::read_to_string(&pid_file).unwrap();
        let escaped = Pid::from_raw(escaped.trim().parse().unwrap()).unwrap();
        kill_process(escaped, Signal::KILL).unwrap();
        std::fs::remove_file(pid_file).unwrap();
    }

    #[test]
    fn caller_group_timeout_kills_only_the_child() {
        let mut call = invocation("sleep", &["30"], Duration::from_millis(200));
        call.group = Group::Caller;
        let message = run(&call).unwrap_err().to_string();
        assert!(message.contains("did not finish"), "{message}");
    }

    #[test]
    fn file_sink_failure_names_the_file() {
        let path = std::env::temp_dir().join(format!("harness-sink-{}", std::process::id()));
        let mut call = invocation("sh", &["-c", "exit 1"], Duration::from_secs(5));
        call.output = Sink::File(path.clone());
        let message = run(&call).unwrap_err().to_string();
        assert!(
            message.contains(&format!("output in {}", path.display())),
            "{message}"
        );
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn observer_errors_are_not_reported_as_a_running_child() {
        let (sender, receiver) = mpsc::channel();
        sender.send(Err(io::Error::from(Errno::CHILD))).unwrap();
        let waited = wait_for_exit(&receiver, Duration::from_secs(5));
        assert_eq!(
            waited.unwrap_err().raw_os_error(),
            Some(Errno::CHILD.raw_os_error())
        );

        drop(sender);
        let message = confirm_exit(&receiver).unwrap_err().to_string();
        assert!(message.contains("exit observer stopped"), "{message}");
    }

    #[test]
    fn exact_env_replaces_the_environment() {
        let env = Env::from([("ONLY", OsString::from("this"))]);
        let mut call = invocation("/usr/bin/env", &[], Duration::from_secs(5));
        call.env = ChildEnv::Exact(&env);
        assert_eq!(run(&call).unwrap().stdout, b"ONLY=this\n");
    }
}
