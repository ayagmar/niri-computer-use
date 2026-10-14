//! The only place the server starts processes. Each child runs in a process group of its
//! own with a deadline. If the call times out or is cancelled before the child has been
//! reaped, the whole group is killed. A child that is still unreaped keeps its process ID, and with it the group
//! ID, from being reused, so the kill can't reach another group.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::sync::OnceLock;
use std::time::Duration;

use rustix::process::{Pid, Signal, kill_process_group};
use tokio::io::{AsyncRead, AsyncReadExt as _};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

use crate::error::{ErrorName, ToolError};

/// The session variables as `main` resolved them, given to every child on top of the
/// environment it inherits, without `WAYLAND_SOCKET`. A client such as Codex starts the server without them, and
/// grim and wl-clipboard need them to reach the display. Setting them in the
/// server's own environment would take `unsafe`.
static SESSION: OnceLock<Vec<(&'static str, OsString)>> = OnceLock::new();

/// Gives `session` to every child started from now on. Only the first call counts: `main`
/// makes it once, before any child starts.
pub(crate) fn pass_on(session: Vec<(&'static str, OsString)>) {
    SESSION.get_or_init(|| session);
}

/// Where a child is looked up, and the `PATH` and working directory it gets.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Search<'a> {
    /// This process's own `PATH` and directory, for what an engine runs for every client.
    Own,
    /// A client's, for what runs for that client alone.
    Client(Place<'a>),
}

/// A client's `PATH`, `None` when its environment had none, and its working directory,
/// where the relative entries are. A child started there finds what the client's
/// standalone server would, whatever the engine's own working directory.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Place<'a> {
    pub(crate) path: Option<&'a OsStr>,
    pub(crate) cwd: Option<&'a Path>,
}

/// Where `execvp` looks when `PATH` is unset: glibc's `_CS_PATH`.
const DEFAULT_PATH: &str = "/bin:/usr/bin";

/// Where the desktop's programs are looked for unless `NIRI_COMPUTER_USE_DESKTOP_PATH`
/// names other directories: the system's own, whatever `PATH` the process got.
pub(crate) const DESKTOP_PATH: &str = "/usr/local/bin:/usr/bin:/bin:/run/current-system/sw/bin";

/// Whether `place` has a file called `program` that this process may execute. Only for
/// reporting: the start itself searches, as `execvp` does.
pub(crate) fn finds(place: Place<'_>, program: &str) -> bool {
    let path = place.path.unwrap_or_else(|| OsStr::new(DEFAULT_PATH));
    std::env::split_paths(path).any(|dir| {
        let file = match place.cwd {
            Some(cwd) => cwd.join(dir).join(program),
            None => dir.join(program),
        };
        executable(&file)
    })
}

fn executable(file: &Path) -> bool {
    file.is_file() && rustix::fs::access(file, rustix::fs::Access::EXEC_OK).is_ok()
}

/// The programs that describe the desktop rather than one client, `loginctl` for the lock
/// state and `noctalia` for the shell, found once at startup in `search`. A client's
/// `PATH` never decides them, since in shared mode they serve every client.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub(crate) struct Desktop {
    /// The directories searched, in order, as `find` reads them: absolute and UTF-8, each
    /// once, without trailing or repeated slashes. Two spellings of one search compare
    /// equal.
    pub(crate) search: String,
    pub(crate) loginctl: Option<PathBuf>,
    pub(crate) noctalia: Option<PathBuf>,
}

impl Desktop {
    /// Looks in `search`'s absolute directories for the first file of each name that this
    /// process may execute. A relative directory would depend on the working directory,
    /// which is the first client's in an engine it started. A directory whose name isn't
    /// UTF-8 is skipped too, since the hello compares the search as text.
    pub(crate) fn find(search: &OsStr) -> Self {
        let mut dirs: Vec<String> = Vec::new();
        for dir in std::env::split_paths(search).filter(|dir| dir.is_absolute()) {
            let dir: PathBuf = dir.components().collect();
            if let Some(dir) = dir
                .to_str()
                .filter(|dir| !dirs.iter().any(|seen| seen == dir))
            {
                dirs.push(dir.to_owned());
            }
        }
        let locate = |program| {
            dirs.iter()
                .map(|dir| Path::new(dir).join(program))
                .find(|file| executable(file))
        };
        Self {
            loginctl: locate("loginctl"),
            noctalia: locate("noctalia"),
            search: dirs.join(":"),
        }
    }
}

/// A command for `program`, to be looked up as `search` says. A client's program gets the
/// client's `PATH`, or none when it has none, and starts in the client's directory, so the
/// system's search runs as it would for the client.
fn searched(program: impl AsRef<OsStr>, search: Search<'_>) -> Command {
    let mut command = command(program);
    let Search::Client(place) = search else {
        return command;
    };
    match place.path {
        Some(path) => command.env("PATH", path),
        None => command.env_remove("PATH"),
    };
    if let Some(cwd) = place.cwd {
        command.current_dir(cwd);
    }
    command
}

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
/// `deadline`. Stdout over `max_stdout` bytes is an error that keeps the exit status and
/// stderr. A non-zero exit is returned, not treated as an error, because some programs
/// report ordinary outcomes that way.
pub(crate) async fn run(
    program: impl AsRef<Path>,
    search: Search<'_>,
    args: &[String],
    deadline: Duration,
    max_stdout: u64,
) -> Result<Finished, ToolError> {
    let program = program.as_ref();
    let name = program.display();
    let child = searched(program, search)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| {
            ToolError::new(ErrorName::UpstreamError, format!("start {name}: {error}"))
        })?;
    let mut running = Running::new(child);
    tokio::time::timeout(deadline, running.finish(&name.to_string(), max_stdout))
        .await
        .unwrap_or_else(|_| {
            Err(ToolError::new(
                ErrorName::DeadlineExceeded,
                format!("{name} didn't finish within {deadline:?}"),
            ))
        })
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

/// Starts a child that outlives the server: in a process group of its own, with stdin and
/// stdout closed and `stderr` as its stderr. The server never kills or waits for it: the
/// handle is dropped at once, which hands it to tokio, which reaps it once it exits. It
/// gets the server's environment without the session's variables `main` discovered, so it
/// finds the session as the server did.
pub(crate) fn daemon(
    program: &str,
    args: &[String],
    stderr: std::fs::File,
) -> Result<(), ToolError> {
    detached_child(program, args, &[], stderr).map(drop)
}

/// Starts `program` as `daemon` does, plus `env`, to become a child that outlives the
/// server by executing the file `becomes`, as `systemd-run --scope` does once its scope
/// exists. Until it has, the server owns it: dropping the handle, even in a cancelled
/// call, kills it unless it has become `becomes` or exited.
pub(crate) fn launcher(
    program: &str,
    args: &[String],
    env: &[(&str, OsString)],
    stderr: std::fs::File,
    becomes: &Path,
) -> Result<Launcher, ToolError> {
    detached_child(program, args, env, stderr).map(|child| Launcher {
        child,
        becomes: becomes.to_path_buf(),
    })
}

fn detached_child(
    program: &str,
    args: &[String],
    env: &[(&str, OsString)],
    stderr: std::fs::File,
) -> Result<Child, ToolError> {
    own_command(program)
        .args(args)
        .envs(env.iter().map(|(name, value)| (name, value)))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(stderr)
        .process_group(0)
        .spawn()
        .map_err(|error| {
            ToolError::new(
                ErrorName::UpstreamError,
                format!("start {program}: {error}"),
            )
        })
}

/// A child `launcher` started.
#[derive(Debug)]
pub(crate) struct Launcher {
    child: Child,
    becomes: PathBuf,
}

impl Launcher {
    /// How it exited, once it has; one that has is reaped. A failed wait counts as an exit,
    /// with its error.
    pub(crate) fn exited(&mut self) -> Option<Result<ExitStatus, String>> {
        match self.child.try_wait() {
            Ok(None) => None,
            Ok(Some(status)) => Some(Ok(status)),
            Err(error) => Some(Err(format!("wait: {error}"))),
        }
    }

    /// Kills its process group and reaps it within `deadline`, unless it has become what it
    /// becomes. Returns whether it was killed. Between the check and the kill it could
    /// still exec; a child that hasn't by now is taken as stuck.
    pub(crate) async fn stop(&mut self, deadline: Duration) -> Result<bool, String> {
        if !self.kill() {
            return Ok(false);
        }
        match tokio::time::timeout(deadline, self.child.wait()).await {
            Ok(Ok(_)) => Ok(true),
            Ok(Err(error)) => Err(format!("reap it: {error}")),
            Err(_) => Err(format!("it didn't exit within {deadline:?} of SIGKILL")),
        }
    }

    /// Sends its process group `SIGKILL` while it runs and hasn't become what it becomes.
    /// Returns whether it did.
    fn kill(&mut self) -> bool {
        if !matches!(self.child.try_wait(), Ok(None)) {
            return false;
        }
        let Some(pid) = self.child.id() else {
            return false;
        };
        if same_file(&PathBuf::from(format!("/proc/{pid}/exe")), &self.becomes) {
            return false;
        }
        let Some(group) = Pid::from_raw(pid.cast_signed()) else {
            return false;
        };
        // It may have exited since, so a missing group is fine.
        kill_process_group(group, Signal::KILL).ok();
        true
    }
}

impl Drop for Launcher {
    /// A call that owned it was cancelled or returned: tokio reaps a killed child once it
    /// exits.
    fn drop(&mut self) {
        self.kill();
    }
}

/// Whether `a` and `b` name one file, following symlinks such as `/proc/<pid>/exe`.
fn same_file(a: &Path, b: &Path) -> bool {
    use std::os::unix::fs::MetadataExt as _;
    match (std::fs::metadata(a), std::fs::metadata(b)) {
        (Ok(a), Ok(b)) => (a.dev(), a.ino()) == (b.dev(), b.ino()),
        _ => false,
    }
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
fn own_command(program: impl AsRef<OsStr>) -> Command {
    let mut command = Command::new(program);
    // libwayland would use an inherited connection before `WAYLAND_DISPLAY`, skipping the
    // display `main` checked is niri's.
    command.env_remove("WAYLAND_SOCKET");
    command
}

/// A command with the session's variables, as `main` resolved them.
fn command(program: impl AsRef<OsStr>) -> Command {
    let mut command = own_command(program);
    command.envs(
        SESSION
            .get()
            .into_iter()
            .flatten()
            .map(|(name, value)| (name, value)),
    );
    command
}

/// A started child. Dropping it before it has been reaped kills its process group.
#[derive(Debug)]
struct Running {
    child: Child,
    group: Option<Pid>,
}

impl Running {
    /// Kills its group while the child is unreaped, so its group ID still belongs to it. It
    /// may already have exited, so a missing group is fine.
    fn kill(&self) {
        if let Some(group) = self.group {
            kill_process_group(group, Signal::KILL).ok();
        }
    }

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
        let stderr = String::from_utf8_lossy(&stderr).into_owned();
        if u64::try_from(stdout.len()).unwrap_or(u64::MAX) > max_stdout {
            return Err(ToolError::new(
                ErrorName::UpstreamError,
                format!(
                    "{program} wrote more than {max_stdout} bytes and exited with {status}: {}",
                    stderr.trim()
                ),
            ));
        }
        Ok(Finished {
            status,
            stdout,
            stderr,
        })
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.kill();
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

    /// The desktop's programs come from the system's directories whatever `PATH` the
    /// process got, so they must be real executables found in a fixed order, and never
    /// under the working directory, which in an engine is its first client's.
    #[test]
    fn the_desktop_programs_are_the_first_executables_in_absolute_directories() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = crate::test_support::fresh_dir("desktop");
        let program = |file: PathBuf, mode| {
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(&file, "").unwrap();
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(mode)).unwrap();
        };
        std::fs::create_dir_all(dir.join("first/loginctl")).unwrap();
        // Only others may execute it, and this process is its owner.
        program(dir.join("first/noctalia"), 0o645);
        program(dir.join("relative/loginctl"), 0o755);
        program(dir.join("second/loginctl"), 0o755);
        program(dir.join("second/noctalia"), 0o755);
        program(dir.join("third/noctalia"), 0o755);
        let cwd = std::env::current_dir().unwrap();
        let up = "../".repeat(cwd.components().count() - 1);
        let relative = Path::new(&up).join(dir.join("relative").strip_prefix("/").unwrap());
        assert!(relative.join("loginctl").is_file());
        let search = std::env::join_paths([
            dir.join("missing"),
            dir.join("first"),
            relative,
            dir.join("second"),
            dir.join("third"),
        ])
        .unwrap();
        let found = Desktop::find(&search);
        assert_eq!(found.loginctl, Some(dir.join("second/loginctl")));
        assert_eq!(found.noctalia, Some(dir.join("second/noctalia")));
        // The same directories in the same order, spelled another way, are the same search.
        let respelled = format!(
            "{}/:{}//second:{}:{}:",
            dir.join("missing").display(),
            dir.display(),
            dir.join("missing").display(),
            dir.join("third").display(),
        );
        let respelled = Desktop::find(OsStr::new(&respelled));
        let plain =
            std::env::join_paths([dir.join("missing"), dir.join("second"), dir.join("third")]);
        assert_eq!(respelled.search, Desktop::find(&plain.unwrap()).search);
        assert_ne!(respelled.search, Desktop::find(&search).search);
        let empty = Desktop::find(dir.join("missing").as_os_str());
        assert_eq!((empty.loginctl, empty.noctalia), (None, None));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn collects_output_and_exit_status() {
        let done = run(
            "sh",
            Search::Own,
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
            let done = run("sh", Search::Own, &args(&["-c", &script]), DEADLINE, 64)
                .await
                .unwrap();
            assert_eq!(done.status.code(), status);
            assert_eq!(done.stdout, b"ok");
            assert_eq!(done.stderr.len(), 16 * 1024);
            assert!(done.stderr.starts_with("e\ne\n"));
        }
    }

    #[tokio::test]
    async fn too_much_stdout_is_an_error_that_keeps_the_exit_and_stderr() {
        let script = "printf 12345; echo 'no frame' >&2; exit 2";
        let error = run("sh", Search::Own, &args(&["-c", script]), DEADLINE, 4)
            .await
            .unwrap_err();
        assert_eq!(
            error,
            ToolError::new(
                ErrorName::UpstreamError,
                "sh wrote more than 4 bytes and exited with exit status: 2: no frame"
            )
        );
    }

    #[tokio::test]
    async fn a_missing_program_keeps_the_os_error() {
        let error = run(
            "niri-computer-use-no-such-program",
            Search::Own,
            &[],
            DEADLINE,
            64,
        )
        .await
        .unwrap_err();
        assert_eq!(error.name, ErrorName::UpstreamError);
        assert!(error.detail.contains("No such file"), "{error:?}");
    }

    /// A shell whose background `sleep` writes its PID to a file, so a test can check that
    /// the grandchild died with its group. The `sleep` keeps the shell's stdout open.
    struct Grandchild {
        dir: PathBuf,
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

    async fn until_written(file: &Path) -> String {
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
    async fn a_timeout_kills_the_whole_group() {
        let grandchild = Grandchild::new("timeout", "wait");
        let error = run(
            "sh",
            Search::Own,
            &grandchild.args,
            Duration::from_secs(1),
            64,
        )
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
        let error = run(
            "sh",
            Search::Own,
            &grandchild.args,
            Duration::from_secs(1),
            64,
        )
        .await
        .unwrap_err();
        assert_eq!(error.name, ErrorName::DeadlineExceeded);
        grandchild.assert_dead().await;
    }

    #[tokio::test]
    async fn dropping_a_call_kills_the_whole_group() {
        let grandchild = Grandchild::new("drop", "wait");
        let args = grandchild.args.clone();
        let mut call = Box::pin(run("sh", Search::Own, &args, DEADLINE, 64));
        tokio::select! {
            _ = &mut call => panic!("the call finished before it was dropped"),
            _ = grandchild.pid() => {}
        }
        drop(call);
        grandchild.assert_dead().await;
    }
}
