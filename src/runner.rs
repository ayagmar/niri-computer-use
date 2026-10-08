//! The only place the server starts processes. Each child runs in a process group of its
//! own with a deadline. If the call times out or is cancelled before the child has been
//! reaped, the whole group is killed. A child that is still unreaped keeps its process ID,
//! and with it the group ID, from being reused, so the kill can't reach another group.

use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use rustix::process::{Pid, Signal, kill_process_group};
use tokio::io::{AsyncRead, AsyncReadExt as _};
use tokio::process::{Child, Command};

use crate::error::{ErrorName, ToolError};

/// How much stderr an error keeps.
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

    /// Reads both pipes to the end, then reaps the child. The pipes close once the child
    /// and anything it started that kept them have exited.
    async fn finish(&mut self, program: &str, max_stdout: u64) -> Result<Finished, ToolError> {
        let (stdout, stderr) = tokio::join!(
            read_limited(self.child.stdout.take(), max_stdout + 1),
            read_limited(self.child.stderr.take(), MAX_STDERR),
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
        let error = run("niri-desktop-mcp-no-such-program", &[], DEADLINE, 64)
            .await
            .unwrap_err();
        assert_eq!(error.name, ErrorName::UpstreamError);
        assert!(error.detail.contains("No such file"), "{error:?}");
    }

    /// A shell whose background `sleep` writes its PID to a file, so a test can check that
    /// the grandchild died with its group.
    struct Grandchild {
        dir: std::path::PathBuf,
        args: Vec<String>,
    }

    impl Grandchild {
        fn new(name: &str) -> Self {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let dir = std::env::temp_dir().join(format!(
                "niri-desktop-mcp-{name}-{}-{nanos}",
                std::process::id()
            ));
            std::fs::create_dir(&dir).unwrap();
            let script = format!("sleep 30 & echo $! > '{}'; wait", dir.join("pid").display());
            Self {
                args: args(&["-c", &script]),
                dir,
            }
        }

        async fn assert_dead(self) {
            let pid = std::fs::read_to_string(self.dir.join("pid")).unwrap();
            let stat = format!("/proc/{}/stat", pid.trim());
            let gone = tokio::time::timeout(DEADLINE, until_dead(&stat)).await;
            std::fs::remove_dir_all(&self.dir).unwrap();
            assert!(gone.is_ok(), "the grandchild outlived its group");
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
    async fn a_timeout_kills_the_whole_group() {
        let grandchild = Grandchild::new("timeout");
        let error = run("sh", &grandchild.args, Duration::from_millis(300), 64)
            .await
            .unwrap_err();
        assert_eq!(error.name, ErrorName::DeadlineExceeded);
        grandchild.assert_dead().await;
    }

    #[tokio::test]
    async fn dropping_a_call_kills_the_whole_group() {
        let grandchild = Grandchild::new("drop");
        let call = run("sh", &grandchild.args, DEADLINE, 64);
        assert!(
            tokio::time::timeout(Duration::from_millis(300), call)
                .await
                .is_err()
        );
        grandchild.assert_dead().await;
    }
}
