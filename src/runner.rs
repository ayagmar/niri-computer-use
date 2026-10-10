//! The only place the server starts processes. Each child runs in a process group of its
//! own with a deadline, with no stdin or with stdin held until the caller feeds it. If
//! the call times out or is cancelled before the child has been reaped, the whole group
//! is killed. A child that is still unreaped keeps its process ID, and with it the group
//! ID, from being reused, so the kill can't reach another group.

use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use rustix::process::{Pid, Signal, kill_process_group};
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWriteExt as _};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::time::Instant;

use crate::error::{ErrorName, ToolError};

/// How much stderr an error keeps. The rest is read and discarded, so a verbose child
/// never sees its stderr closed.
const MAX_STDERR: u64 = 16 * 1024;

#[derive(Debug)]
pub(crate) struct Finished {
    pub(crate) status: ExitStatus,
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: String,
}

impl Finished {
    /// The upstream error for a non-zero exit, with its status and stderr.
    pub(crate) fn failure(&self, program: &str) -> ToolError {
        ToolError::new(
            ErrorName::UpstreamError,
            format!(
                "{program} exited with {}: {}",
                self.status,
                self.stderr.trim()
            ),
        )
    }
}

/// Runs `program` with `args` and no stdin, and collects its output, all within
/// `deadline`. Stdout over `max_stdout` bytes is an error. A non-zero exit is returned,
/// not treated as an error, because some programs report ordinary outcomes that way.
pub(crate) async fn run(
    program: &str,
    args: &[String],
    deadline: Duration,
    max_stdout: u64,
) -> Result<Finished, ToolError> {
    let child = command(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| {
            ToolError::new(
                ErrorName::UpstreamError,
                format!("start {program}: {error}"),
            )
        })?;
    let mut running = Running::new(child);
    tokio::time::timeout(deadline, running.finish(program, max_stdout))
        .await
        .unwrap_or_else(|_| {
            Err(ToolError::new(
                ErrorName::DeadlineExceeded,
                format!("{program} didn't finish within {deadline:?}"),
            ))
        })
}

/// A child started with its stdin held open: it may not read anything until `feed`, so
/// its PID can be recorded before it acts (plan §11, the stdin gate). Dropping it before
/// it has been reaped kills its process group.
#[derive(Debug)]
pub(crate) struct Gated {
    program: String,
    running: Running,
    stdin: ChildStdin,
    /// Counted from the start, so the gate's wait counts too.
    deadline: Instant,
}

/// Starts `program` with `args` and stdin as a pipe that stays open until `feed`.
/// `deadline` covers the whole run, from now until the child is reaped.
pub(crate) fn gated(
    program: &str,
    args: &[String],
    env: &[(&str, &str)],
    deadline: Duration,
) -> Result<Gated, ToolError> {
    let mut child = command(program)
        .args(args)
        .envs(env.iter().copied())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| {
            ToolError::new(
                ErrorName::UpstreamError,
                format!("start {program}: {error}"),
            )
        })?;
    let stdin = child.stdin.take().ok_or_else(|| {
        ToolError::new(ErrorName::UpstreamError, format!("{program} has no stdin"))
    })?;
    Ok(Gated {
        program: program.to_owned(),
        running: Running::new(child),
        stdin,
        deadline: Instant::now() + deadline,
    })
}

impl Gated {
    pub(crate) fn pid(&self) -> Option<u32> {
        self.running.child.id()
    }

    /// Writes `input` to the child's stdin, closes it, and collects the child's output,
    /// all before the deadline. Past it, the child's group is killed.
    pub(crate) async fn feed(
        mut self,
        input: &[u8],
        max_stdout: u64,
    ) -> Result<Finished, ToolError> {
        let program = self.program.clone();
        let deadline = self.deadline;
        let run = async {
            match self.stdin.write_all(input).await {
                // The child exited without reading; its exit status says why.
                Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => {}
                written => written.map_err(|error| {
                    ToolError::new(
                        ErrorName::UpstreamError,
                        format!("write to {program}: {error}"),
                    )
                })?,
            }
            drop(self.stdin);
            self.running.finish(&program, max_stdout).await
        };
        tokio::time::timeout_at(deadline, run)
            .await
            .unwrap_or_else(|_| {
                Err(ToolError::new(
                    ErrorName::DeadlineExceeded,
                    format!("{program} didn't finish within its deadline"),
                ))
            })
    }
}

/// A child that lives as long as the server and only waits for its stdin to end. The
/// server holds the pipe's one write end, so the child reads end of file when the server
/// exits, however it exits. It has a process group of its own, so a signal to the
/// server's group doesn't reach it, and dropping the handle doesn't kill it: it has
/// nothing to wait for but the server, and every step it takes after that has its own
/// deadline. Its stdout is closed, because the server's is the MCP transport.
#[derive(Debug)]
pub(crate) struct Watcher {
    /// Held so the server keeps its handle on the child for its lifetime.
    _child: Child,
    /// The write end; closing it, or the server's exit, ends the child's wait.
    _stdin: ChildStdin,
}

pub(crate) fn watcher(program: &str, args: &[String]) -> Result<Watcher, ToolError> {
    let (child, stdin, _) = detached(program, args, Stdio::null())?;
    Ok(Watcher {
        _child: child,
        _stdin: stdin,
    })
}

/// A child that may outlive the call that starts it, and the server: it talks to the
/// server over its stdin and stdout, in a process group of its own, and dropping the handle
/// doesn't kill it. Closing its stdin tells it the server is done with it; every step it
/// takes has its own deadline, except what the caller documents as unbounded.
#[derive(Debug)]
pub(crate) struct Companion {
    /// Held so the server keeps its handle on the child.
    _child: Child,
    pub(crate) stdin: ChildStdin,
    pub(crate) stdout: ChildStdout,
}

pub(crate) fn companion(program: &str, args: &[String]) -> Result<Companion, ToolError> {
    let (child, stdin, stdout) = detached(program, args, Stdio::piped())?;
    let stdout = stdout.ok_or_else(|| {
        ToolError::new(ErrorName::UpstreamError, format!("{program} has no stdout"))
    })?;
    Ok(Companion {
        _child: child,
        stdin,
        stdout,
    })
}

/// Starts a child with a pipe as its stdin, in a process group of its own, not killed on
/// drop. Its stderr is the server's.
fn detached(
    program: &str,
    args: &[String],
    output: Stdio,
) -> Result<(Child, ChildStdin, Option<ChildStdout>), ToolError> {
    let mut child = command(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(output)
        .stderr(Stdio::inherit())
        .process_group(0)
        .spawn()
        .map_err(|error| {
            ToolError::new(
                ErrorName::UpstreamError,
                format!("start {program}: {error}"),
            )
        })?;
    let stdin = child.stdin.take().ok_or_else(|| {
        ToolError::new(ErrorName::UpstreamError, format!("{program} has no stdin"))
    })?;
    let stdout = child.stdout.take();
    Ok((child, stdin, stdout))
}

#[expect(
    clippy::disallowed_methods,
    reason = "the runner is the one place that starts processes"
)]
fn command(program: &str) -> Command {
    Command::new(program)
}

/// A started child. Dropping it before it has been reaped kills its process group.
#[derive(Debug)]
struct Running {
    child: Child,
    group: Option<Pid>,
}

impl Running {
    fn new(child: Child) -> Self {
        let group = child
            .id()
            .and_then(|id| i32::try_from(id).ok())
            .and_then(Pid::from_raw);
        Self { child, group }
    }

    /// Reads both pipes, then reaps the child. Stderr is read to the end; stdout until the
    /// end or one byte past its limit. The pipes close once the child and anything it
    /// started that kept them have exited.
    async fn finish(&mut self, program: &str, max_stdout: u64) -> Result<Finished, ToolError> {
        let (stdout, stderr) = tokio::join!(
            read_limited(self.child.stdout.take(), max_stdout + 1),
            read_keeping(self.child.stderr.take(), MAX_STDERR),
        );
        let broken = |error: std::io::Error| {
            ToolError::new(ErrorName::UpstreamError, format!("{program}: {error}"))
        };
        let (stdout, stderr) = (stdout.map_err(broken)?, stderr.map_err(broken)?);
        let status = self.child.wait().await.map_err(broken)?;
        self.group = None;
        if u64::try_from(stdout.len()).unwrap_or(u64::MAX) > max_stdout {
            return Err(ToolError::new(
                ErrorName::UpstreamError,
                format!("{program} wrote more than {max_stdout} bytes"),
            ));
        }
        Ok(Finished {
            status,
            stdout,
            stderr: String::from_utf8_lossy(&stderr).into_owned(),
        })
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        if let Some(group) = self.group {
            // The child is unreaped, so its group ID still belongs to it. It may already
            // have exited, so a missing group is fine.
            kill_process_group(group, Signal::KILL).ok();
        }
    }
}

/// Keeps the first `keep` bytes and discards the rest, reading to the end.
async fn read_keeping(pipe: Option<impl AsyncRead + Unpin>, keep: u64) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    if let Some(mut pipe) = pipe {
        (&mut pipe).take(keep).read_to_end(&mut bytes).await?;
        tokio::io::copy(&mut pipe, &mut tokio::io::sink()).await?;
    }
    Ok(bytes)
}

async fn read_limited(
    pipe: Option<impl AsyncRead + Unpin>,
    limit: u64,
) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    if let Some(pipe) = pipe {
        pipe.take(limit).read_to_end(&mut bytes).await?;
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|arg| (*arg).to_owned()).collect()
    }

    const DEADLINE: Duration = Duration::from_secs(5);

    #[tokio::test]
    async fn collects_output_and_exit_status() {
        let done = run(
            "sh",
            &args(&["-c", "printf out; printf err >&2; exit 3"]),
            DEADLINE,
            64,
        )
        .await
        .unwrap();
        assert_eq!(done.stdout, b"out");
        assert_eq!(done.stderr, "err");
        assert_eq!(done.status.code(), Some(3));
        assert_eq!(
            done.failure("sh"),
            ToolError::new(
                ErrorName::UpstreamError,
                "sh exited with exit status: 3: err"
            )
        );
    }

    /// A child that writes far more stderr than is kept still runs to its own exit.
    #[tokio::test]
    async fn long_stderr_is_drained_and_truncated() {
        for (code, status) in [("0", Some(0)), ("4", Some(4))] {
            let script = format!("yes e | head -c 200000 >&2 && printf ok; exit {code}");
            let done = run("sh", &args(&["-c", &script]), DEADLINE, 64)
                .await
                .unwrap();
            assert_eq!(done.status.code(), status);
            assert_eq!(done.stdout, b"ok");
            assert_eq!(done.stderr.len(), 16 * 1024);
            assert!(done.stderr.starts_with("e\ne\n"));
        }
    }

    #[tokio::test]
    async fn too_much_stdout_is_an_error() {
        let error = run("sh", &args(&["-c", "printf 12345"]), DEADLINE, 4)
            .await
            .unwrap_err();
        assert_eq!(
            error,
            ToolError::new(ErrorName::UpstreamError, "sh wrote more than 4 bytes")
        );
    }

    #[tokio::test]
    async fn a_missing_program_keeps_the_os_error() {
        let error = run("niri-computer-use-no-such-program", &[], DEADLINE, 64)
            .await
            .unwrap_err();
        assert_eq!(error.name, ErrorName::UpstreamError);
        assert!(error.detail.contains("No such file"), "{error:?}");
    }

    /// A shell whose background `sleep` writes its PID to a file, so a test can check that
    /// the grandchild died with its group. The `sleep` keeps the shell's stdout open.
    struct Grandchild {
        dir: std::path::PathBuf,
        args: Vec<String>,
    }

    impl Grandchild {
        /// `then` runs after the PID is written: `wait` keeps the shell, the group's leader,
        /// running; `exit 0` ends it while the `sleep` still holds stdout.
        fn new(name: &str, then: &str) -> Self {
            let dir = crate::test_support::fresh_dir(name);
            let script = format!(
                "sleep 30 & echo $! > '{}'; {then}",
                dir.join("pid").display()
            );
            Self {
                args: args(&["-c", &script]),
                dir,
            }
        }

        /// The grandchild's PID, once the shell has written all of it.
        async fn pid(&self) -> String {
            tokio::time::timeout(DEADLINE, until_written(&self.dir.join("pid")))
                .await
                .expect("the shell never started its grandchild")
        }

        async fn assert_dead(self) {
            let stat = format!("/proc/{}/stat", self.pid().await);
            let gone = tokio::time::timeout(DEADLINE, until_dead(&stat)).await;
            std::fs::remove_dir_all(&self.dir).unwrap();
            assert!(gone.is_ok(), "the grandchild outlived its group");
        }
    }

    async fn until_written(file: &std::path::Path) -> String {
        loop {
            if let Some(pid) = std::fs::read_to_string(file)
                .ok()
                .filter(|text| text.ends_with('\n'))
            {
                return pid.trim().to_owned();
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    async fn until_dead(stat: &str) {
        while alive(stat) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// Alive while its `/proc` entry exists and it isn't a zombie waiting for init.
    fn alive(stat: &str) -> bool {
        let Ok(line) = std::fs::read_to_string(stat) else {
            return false;
        };
        line.rsplit_once(") ")
            .is_some_and(|(_, rest)| !rest.starts_with('Z'))
    }

    #[tokio::test]
    async fn a_gated_child_reads_nothing_until_fed() {
        let dir = crate::test_support::fresh_dir("gated");
        let seen = dir.join("seen");
        let script = format!("cat > '{}'; printf done", seen.display());
        let gated = gated("sh", &args(&["-c", &script]), &[], DEADLINE).unwrap();
        assert!(gated.pid().is_some());
        tokio::time::sleep(Duration::from_millis(100)).await;
        // `cat` waits at the open pipe, so nothing is written yet.
        assert_eq!(std::fs::read_to_string(&seen).unwrap(), "");
        let done = gated.feed(b"typed", 64).await.unwrap();
        assert_eq!(done.stdout, b"done");
        assert_eq!(std::fs::read_to_string(&seen).unwrap(), "typed");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn a_gated_child_that_exits_unread_reports_its_own_exit() {
        let script = "echo 'no display' >&2; exit 1";
        let gated = gated(
            "sh",
            &args(&["-c", script]),
            &[("LC_ALL", "C.UTF-8")],
            DEADLINE,
        )
        .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        // A large write meets the closed pipe.
        let done = gated.feed(&vec![b'x'; 1 << 20], 64).await.unwrap();
        assert_eq!(done.status.code(), Some(1));
        assert_eq!(done.stderr.trim(), "no display");
    }

    #[tokio::test]
    async fn a_gated_child_past_its_deadline_is_killed_with_its_group() {
        let grandchild = Grandchild::new("gated-timeout", "wait");
        let gated = gated("sh", &grandchild.args, &[], Duration::from_secs(1)).unwrap();
        let error = gated.feed(b"", 64).await.unwrap_err();
        assert_eq!(error.name, ErrorName::DeadlineExceeded);
        grandchild.assert_dead().await;
    }

    #[tokio::test]
    async fn a_timeout_kills_the_whole_group() {
        let grandchild = Grandchild::new("timeout", "wait");
        let error = run("sh", &grandchild.args, Duration::from_secs(1), 64)
            .await
            .unwrap_err();
        assert_eq!(error.name, ErrorName::DeadlineExceeded);
        grandchild.assert_dead().await;
    }

    /// The leader has exited, but the result isn't served while a descendant holds a pipe,
    /// and the leader stays unreaped, so its group can still be killed.
    #[tokio::test]
    async fn an_exited_leader_with_a_descendant_holding_stdout_times_out() {
        let grandchild = Grandchild::new("leader", "exit 0");
        let error = run("sh", &grandchild.args, Duration::from_secs(1), 64)
            .await
            .unwrap_err();
        assert_eq!(error.name, ErrorName::DeadlineExceeded);
        grandchild.assert_dead().await;
    }

    #[tokio::test]
    async fn dropping_a_call_kills_the_whole_group() {
        let grandchild = Grandchild::new("drop", "wait");
        let args = grandchild.args.clone();
        let mut call = Box::pin(run("sh", &args, DEADLINE, 64));
        tokio::select! {
            _ = &mut call => panic!("the call finished before it was dropped"),
            _ = grandchild.pid() => {}
        }
        drop(call);
        grandchild.assert_dead().await;
    }
}
