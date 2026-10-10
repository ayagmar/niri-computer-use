//! The only place the harness starts processes. Every child has a deadline. A child in its
//! own process group has the whole group killed when it exits, times out, is stopped or the
//! harness is interrupted, so nothing it started outlives it unless it left the group. The child is
//! not reaped until after that kill, so its process ID, and with it the group ID, can't
//! have been reused.

use std::ffi::OsString;
use std::fs::File;
use std::io::{self, Read, Write as _};
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Output, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
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
    /// Killed by `Process::stop` before its deadline.
    Stopped,
}

/// Runs a program to completion and requires exit status 0. Errors keep the exit status
/// and stderr, or name the file the output went to.
pub(crate) fn run(invocation: &Invocation<'_>) -> Result<Output> {
    start(invocation)?.wait()
}

/// Starts a program and returns while it runs. The caller ends it with `wait` or `stop`,
/// and dropping the handle stops it. A watchdog kills it at its deadline.
pub(crate) fn start(invocation: &Invocation<'_>) -> Result<Process> {
    spawn(invocation, false)
}

/// Starts with stdin held open until `Process::feed` writes and closes it.
pub(crate) fn start_with_stdin(invocation: &Invocation<'_>) -> Result<Process> {
    spawn(invocation, true)
}

fn spawn(invocation: &Invocation<'_>, piped: bool) -> Result<Process> {
    let program = invocation.program;
    if interrupt::requested() {
        return Err(Failure::new(format!(
            "interrupted before starting {program}"
        )));
    }
    let mut child = command(invocation, piped)?
        .spawn()
        .context(format!("start {program}"))?;
    let pid = Pid::from_child(&child);
    let stdout = read_in_background(child.stdout.take());
    let stderr = read_in_background(child.stderr.take());
    let reaped = Arc::new(Mutex::new(false));
    let end = Instant::now() + invocation.deadline;
    Ok(Process {
        program: program.to_owned(),
        exit: observe_exit(pid),
        watchdog: Some(watchdog(pid, invocation.group, end, Arc::clone(&reaped))),
        reaped,
        stdin: child.stdin.take(),
        child,
        pid,
        group: invocation.group,
        deadline: invocation.deadline,
        end,
        log_file: match &invocation.output {
            Sink::Capture => None,
            Sink::File(path) => Some(path.clone()),
        },
        stdout,
        stderr,
        exited: false,
        finished: false,
    })
}

/// A started program, not yet reaped.
#[derive(Debug)]
pub(crate) struct Process {
    program: String,
    child: Child,
    stdin: Option<ChildStdin>,
    pid: Pid,
    group: Group,
    deadline: Duration,
    end: Instant,
    log_file: Option<PathBuf>,
    exit: Receiver<io::Result<()>>,
    /// Dropped once the child is reaped, which ends the watchdog.
    watchdog: Option<Sender<()>>,
    /// Set under the lock once the child is reaped. The watchdog only kills while it is
    /// false, so it can't signal a process ID that has been reused.
    reaped: Arc<Mutex<bool>>,
    stdout: Option<Receiver<io::Result<Vec<u8>>>>,
    stderr: Option<Receiver<io::Result<Vec<u8>>>>,
    /// The observer's one message has been received.
    exited: bool,
    finished: bool,
}

impl Process {
    /// The child's process ID. It can't be reused before the child is reaped, which only
    /// `wait` and `stop` do, and they take the handle.
    pub(crate) const fn pid(&self) -> i32 {
        self.pid.as_raw_nonzero().get()
    }

    /// Writes all bytes, then closes stdin, within the child's original deadline. The
    /// watchdog also bounds a writer blocked by a child that never reads its pipe.
    pub(crate) fn feed(&mut self, bytes: Vec<u8>) -> Result<()> {
        self.send(bytes)?;
        self.stdin = None;
        if Instant::now() >= self.end {
            return Err(Failure::new(format!(
                "{} stdin deadline passed",
                self.program
            )));
        }
        Ok(())
    }

    /// Writes all bytes and keeps stdin open, within the child's original deadline.
    pub(crate) fn send(&mut self, bytes: Vec<u8>) -> Result<()> {
        let mut stdin = self
            .stdin
            .take()
            .ok_or_else(|| Failure::new("stdin is not open"))?;
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let written = stdin.write_all(&bytes).and_then(|()| stdin.flush());
            sender.send((written, stdin))
        });
        let left = self.end.saturating_duration_since(Instant::now());
        let (written, open) = receiver.recv_timeout(left).context(format!(
            "write stdin of {} before its deadline",
            self.program
        ))?;
        written.context(format!("write stdin of {}", self.program))?;
        self.stdin = Some(open);
        Ok(())
    }

    /// Checks that a held-stdin child is still alive without reaping it.
    pub(crate) fn ensure_running(&mut self) -> Result<()> {
        match self.exit.try_recv() {
            Err(TryRecvError::Empty) if Instant::now() < self.end => Ok(()),
            Ok(waited) => {
                self.exited = waited.is_ok();
                waited.context(format!("observe {}", self.program))?;
                let output = self.finish(Ok(Ending::Exited))?;
                Err(Failure::new(format!(
                    "{} exited while stdin was held open with {}{}",
                    self.program,
                    output.status,
                    detail(None, Ok(output.stderr))
                )))
            }
            Err(error) => Err(Failure::new(format!(
                "{} is not running: {error}",
                self.program
            ))),
        }
    }

    /// Waits for the program to exit, until its deadline. An exit seen at or after the
    /// deadline is a timeout, as for `stop`.
    pub(crate) fn wait(mut self) -> Result<Output> {
        let ending = wait_for_exit(&self.exit, self.end);
        self.exited = matches!(ending, Ok(Ending::Exited));
        let ending = ending.map(|ending| self.late(ending));
        self.finish(ending)
    }

    /// Kills a program that runs until it is told to stop. It fails if the program has
    /// already failed, or if its deadline has passed.
    pub(crate) fn stop(self) -> Result<Output> {
        let received = self.exit.try_recv();
        self.stopped(received)
    }

    /// Like `stop`, but first asks the program to exit with SIGTERM and gives it `grace` to
    /// clean up, such as removing sockets it bound outside its directory. Only the leader is
    /// signalled, so it can end its own children; the rest of the group is killed after.
    pub(crate) fn terminate(mut self, grace: Duration) -> Result<Output> {
        let received = match self.exit.try_recv() {
            Err(TryRecvError::Empty) => {
                kill_process(self.pid, Signal::TERM)
                    .context(format!("terminate {}", self.program))?;
                self.exit.recv_timeout(grace)
            }
            before => return self.stopped(before),
        };
        match received {
            Ok(waited) => {
                self.exited = waited.is_ok();
                let ending = waited.map(|()| self.late(Ending::Stopped));
                self.finish(ending)
            }
            Err(RecvTimeoutError::Timeout) => self.stopped(Err(TryRecvError::Empty)),
            Err(RecvTimeoutError::Disconnected) => self.stopped(Err(TryRecvError::Disconnected)),
        }
    }

    fn stopped(
        mut self,
        received: std::result::Result<io::Result<()>, TryRecvError>,
    ) -> Result<Output> {
        let ending = match received {
            Ok(waited) => waited.map(|()| Ending::Exited),
            Err(TryRecvError::Empty) => Ok(Ending::Stopped),
            Err(TryRecvError::Disconnected) => Err(io::Error::other("the exit observer stopped")),
        };
        self.exited = matches!(ending, Ok(Ending::Exited));
        let ending = ending.map(|ending| self.late(ending));
        self.finish(ending)
    }

    /// An ending seen at or after the deadline is a timeout, even an exit. The watchdog may
    /// have caused it, and a result that late doesn't count.
    fn late(&self, ending: Ending) -> Ending {
        if ending != Ending::Interrupted && Instant::now() >= self.end {
            Ending::TimedOut
        } else {
            ending
        }
    }

    fn finish(&mut self, ending: io::Result<Ending>) -> Result<Output> {
        self.finished = true;
        let program = self.program.clone();
        let mut reaped = self
            .reaped
            .lock()
            .map_err(|_| Failure::new(format!("the watchdog of {program} panicked")))?;
        kill(self.pid, self.group, self.exited).context(format!("kill {program}"))?;
        // Do not release a held stdin gate before killing the child.
        self.stdin = None;
        // An observer failure ends here, killed but not reaped: its one message is used up.
        let ending = ending.context(format!("wait for {program}"))?;
        if !self.exited {
            confirm_exit(&self.exit).context(format!("stop {program}"))?;
        }
        let status = self.child.wait().context(format!("reap {program}"))?;
        *reaped = true;
        drop(reaped);
        self.watchdog = None;
        let output_end = Instant::now() + OUTPUT_GRACE;
        let stdout = collect(self.stdout.take(), output_end)
            .context(format!("read the output of {program}"));
        let stderr = collect(self.stderr.take(), output_end)
            .context(format!("read the output of {program}"));
        let failure = match ending {
            Ending::Stopped => None,
            Ending::Exited if status.success() => None,
            Ending::Exited => Some(format!("{program} failed with {status}")),
            Ending::TimedOut => Some(format!(
                "{program} did not finish within {:?}",
                self.deadline
            )),
            Ending::Interrupted => {
                return Err(Failure::new(format!("interrupted while running {program}")));
            }
        };
        match failure {
            None => Ok(Output {
                status,
                stdout: stdout?,
                stderr: stderr?,
            }),
            Some(failure) => Err(Failure::new(format!(
                "{failure}{}",
                detail(self.log_file.as_deref(), stderr)
            ))),
        }
    }
}

impl Drop for Process {
    /// Stops a program the caller didn't end, for example after an early return. There is
    /// nowhere to report a failure to, and the caller already has its own error.
    fn drop(&mut self) {
        if !self.finished {
            drop(self.finish(Ok(Ending::Stopped)));
        }
    }
}

/// Kills the child at its deadline unless it has been reaped by then. Ends early when the
/// returned sender is dropped.
fn watchdog(pid: Pid, group: Group, end: Instant, reaped: Arc<Mutex<bool>>) -> Sender<()> {
    let (cancel, cancelled) = mpsc::channel();
    thread::spawn(move || {
        // Never before `end`, the instant `wait` and `stop` treat as the deadline: a kill
        // that lands earlier would read as the program failing on its own.
        let left = end.saturating_duration_since(Instant::now());
        if cancelled.recv_timeout(left) == Err(RecvTimeoutError::Timeout)
            && let Ok(reaped) = reaped.lock()
            && !*reaped
        {
            // Nowhere to report a failure; `wait` or `stop` reports the timeout.
            kill(pid, group, false).ok();
        }
    });
    cancel
}

#[expect(
    clippy::disallowed_methods,
    reason = "this is the runner every other module goes through"
)]
fn command(invocation: &Invocation<'_>, piped: bool) -> Result<Command> {
    let mut command = Command::new(invocation.program);
    command
        .args(&invocation.args)
        .stdin(if piped { Stdio::piped() } else { Stdio::null() });
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

fn wait_for_exit(exit: &Receiver<io::Result<()>>, end: Instant) -> io::Result<Ending> {
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

fn detail(log_file: Option<&Path>, stderr: Result<Vec<u8>>) -> String {
    if let Some(path) = log_file {
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
    fn stdin_writes_all_bytes_and_closes() {
        let call = invocation("cat", &[], Duration::from_secs(5));
        let mut process = start_with_stdin(&call).unwrap();
        process.ensure_running().unwrap();
        let bytes = vec![b'x'; 200_000];
        process.feed(bytes.clone()).unwrap();
        assert_eq!(process.wait().unwrap().stdout, bytes);
    }

    #[test]
    fn stdin_can_be_held_then_closed_without_data() {
        let call = invocation("cat", &[], Duration::from_secs(5));
        let mut process = start_with_stdin(&call).unwrap();
        thread::park_timeout(Duration::from_millis(100));
        process.ensure_running().unwrap();
        process.feed(Vec::new()).unwrap();
        assert_eq!(process.wait().unwrap().stdout, Vec::<u8>::new());
    }

    #[test]
    fn a_blocked_stdin_writer_has_a_deadline() {
        let call = invocation("sleep", &["30"], Duration::from_millis(100));
        let mut process = start_with_stdin(&call).unwrap();
        let started = Instant::now();
        assert!(process.feed(vec![b'x'; 200_000]).is_err());
        assert!(process.wait().is_err());
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn an_early_exit_cannot_pass_a_stdin_gate() {
        let call = invocation("true", &[], Duration::from_secs(5));
        let mut process = start_with_stdin(&call).unwrap();
        thread::park_timeout(Duration::from_millis(100));
        assert!(process.ensure_running().is_err());
        drop(process);
    }

    #[test]
    fn a_gate_child_failure_keeps_status_and_stderr() {
        let call = invocation(
            "sh",
            &["-c", "echo broke >&2; exit 3"],
            Duration::from_secs(5),
        );
        let mut process = start_with_stdin(&call).unwrap();
        thread::park_timeout(Duration::from_millis(100));
        let message = process.ensure_running().unwrap_err().to_string();
        assert!(message.contains("exit status: 3"), "{message}");
        assert!(message.contains("broke"), "{message}");
    }

    #[test]
    fn dropping_held_stdin_does_not_release_the_gated_command() {
        let dir = std::env::temp_dir().join(format!("harness-held-drop-{}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        let path = dir.to_str().unwrap();
        let call = invocation(
            "sh",
            &[
                "-c",
                "touch \"$1/ready\"; read -r ignored; touch \"$1/released\"",
                "gate",
                path,
            ],
            Duration::from_secs(5),
        );
        let mut process = start_with_stdin(&call).unwrap();
        let end = Instant::now() + Duration::from_secs(2);
        while !dir.join("ready").exists() && Instant::now() < end {
            thread::park_timeout(Duration::from_millis(10));
        }
        assert!(dir.join("ready").exists());
        process.ensure_running().unwrap();
        drop(process);
        assert!(!dir.join("released").exists());
        std::fs::remove_dir_all(dir).unwrap();
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
        let waited = wait_for_exit(&receiver, Instant::now() + Duration::from_secs(5));
        assert_eq!(
            waited.unwrap_err().raw_os_error(),
            Some(Errno::CHILD.raw_os_error())
        );

        drop(sender);
        let message = confirm_exit(&receiver).unwrap_err().to_string();
        assert!(message.contains("exit observer stopped"), "{message}");
    }

    #[test]
    fn stop_kills_a_running_program() {
        let started = Instant::now();
        let process = start(&invocation("sleep", &["30"], Duration::from_secs(5))).unwrap();
        process.stop().unwrap();
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    /// Starts `sh` running `script` with `$1` set to a fresh directory, and returns once it
    /// has created `$1/ready`.
    fn started_with_trap(name: &str, script: &str) -> (Process, PathBuf) {
        let dir = std::env::temp_dir().join(format!("harness-{name}-{}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        let call = invocation(
            "sh",
            &["-c", script, name, dir.to_str().unwrap()],
            Duration::from_secs(10),
        );
        let process = start(&call).unwrap();
        let end = Instant::now() + Duration::from_secs(2);
        while !dir.join("ready").exists() && Instant::now() < end {
            thread::park_timeout(Duration::from_millis(10));
        }
        assert!(dir.join("ready").exists());
        (process, dir)
    }

    #[test]
    fn terminate_lets_a_program_clean_up() {
        let (process, dir) = started_with_trap(
            "terminate-clean",
            "trap 'touch \"$1/cleaned\"; exit 0' TERM; touch \"$1/ready\"; \
             while :; do sleep 0.05; done",
        );
        process.terminate(Duration::from_secs(3)).unwrap();
        assert!(dir.join("cleaned").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn terminate_kills_a_program_that_ignores_sigterm() {
        let (process, dir) = started_with_trap(
            "terminate-ignored",
            "trap '' TERM; touch \"$1/ready\"; while :; do sleep 0.05; done",
        );
        let started = Instant::now();
        process.terminate(Duration::from_millis(300)).unwrap();
        assert!(started.elapsed() < Duration::from_secs(3));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn stop_after_the_deadline_fails() {
        let process = start(&invocation("sleep", &["30"], Duration::from_millis(100))).unwrap();
        thread::park_timeout(Duration::from_millis(300));
        let message = process.stop().unwrap_err().to_string();
        assert!(message.contains("did not finish"), "{message}");
    }

    #[test]
    fn stop_after_the_deadline_fails_even_if_the_program_is_gone() {
        let process = start(&invocation("sleep", &["0.1"], Duration::from_millis(50))).unwrap();
        thread::park_timeout(Duration::from_millis(400));
        let message = process.stop().unwrap_err().to_string();
        assert!(message.contains("did not finish"), "{message}");
    }

    #[test]
    fn the_watchdog_kills_a_started_program_at_its_deadline() {
        let mut call = invocation("sleep", &["30"], Duration::from_millis(200));
        call.group = Group::Caller;
        let mut process = start(&call).unwrap();
        let exited = process.exit.recv_timeout(Duration::from_secs(3));
        assert!(matches!(exited, Ok(Ok(()))), "{exited:?}");
        // Hand the observer's one message back for `stop`.
        let (sender, receiver) = mpsc::channel();
        sender.send(Ok(())).unwrap();
        process.exit = receiver;
        assert!(
            process
                .stop()
                .unwrap_err()
                .to_string()
                .contains("did not finish")
        );
    }

    #[test]
    fn stop_reports_a_program_that_already_failed() {
        let call = invocation("sh", &["-c", "exit 3"], Duration::from_secs(5));
        let process = start(&call).unwrap();
        thread::park_timeout(Duration::from_millis(500));
        let message = process.stop().unwrap_err().to_string();
        assert!(message.contains("exit status: 3"), "{message}");
    }

    #[test]
    fn dropping_a_process_kills_it() {
        let process = start(&invocation("sleep", &["30"], Duration::from_secs(5))).unwrap();
        let pid = process.pid.as_raw_nonzero().to_string();
        drop(process);
        assert!(dies_soon(&pid));
    }

    #[test]
    fn exact_env_replaces_the_environment() {
        let env = Env::from([("ONLY", OsString::from("this"))]);
        let mut call = invocation("/usr/bin/env", &[], Duration::from_secs(5));
        call.env = ChildEnv::Exact(&env);
        assert_eq!(run(&call).unwrap().stdout, b"ONLY=this\n");
    }
}
