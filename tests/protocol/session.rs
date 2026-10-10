//! A fake niri in a process of its own, so its environment and command line are exactly
//! what a test chooses. The server asks logind about the session in niri's
//! `XDG_SESSION_ID`, which it reads from `/proc/<niri pid>/environ`, and only when niri's
//! `/proc/<niri pid>/cmdline` has `--session`; the in-process fake niri would show the
//! test runner's instead. The test binary runs itself with `--ignored --exact` to become
//! this process. `--session` goes after `--`, where libtest takes it as one more test name
//! filter, which matches no test.
//!
//! It answers `Version` itself, or relays every connection to the in-process fake niri, so
//! a test has the whole fake while niri's socket is served by a process of its own. Started
//! through a link called `niri`, it can also serve a Wayland display, as discovery expects
//! of a running niri.

use std::io::{BufRead as _, BufReader, Write as _};
use std::net::Shutdown;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use crate::fixture::{Fixture, eventually};

/// Where it listens; `<pid>` stands for its process ID.
const SOCKET: &str = "NCU_FAKE_NIRI_SOCKET";
const RELAY: &str = "NCU_FAKE_NIRI_RELAY";
/// A display socket it listens on too, accepting nothing.
const DISPLAY_SOCKET: &str = "NCU_FAKE_NIRI_DISPLAY";
const NAME: &str = "session::fake_niri_process";

/// A running fake niri. Dropping it kills the process.
#[derive(Debug)]
pub(crate) struct NiriProcess {
    _process: tokio::process::Child,
}

impl NiriProcess {
    /// Starts it as `niri --session` on the fixture's `NIRI_SOCKET` with `XDG_SESSION_ID`
    /// set to `session`, or unset for `None`.
    pub(crate) async fn start(fixture: &Fixture, session: Option<&str>) -> Self {
        Self::launch(fixture, session, &["--session"]).await
    }

    /// Starts it as a plain `niri`, which sets no logind locked hint.
    pub(crate) async fn plain(fixture: &Fixture, session: Option<&str>) -> Self {
        Self::launch(fixture, session, &[]).await
    }

    async fn launch(fixture: &Fixture, session: Option<&str>, niri_args: &[&str]) -> Self {
        // The fixture's socket file, or one an earlier fake niri in the same test left.
        std::fs::remove_file(fixture.niri_socket()).ok();
        let program = std::env::current_exe().unwrap();
        let mut command = command(&program);
        command.env(SOCKET, fixture.niri_socket());
        Self::spawn(command, &fixture.niri_socket(), session, niri_args)
            .await
            .0
    }

    /// Starts `command` and waits for its socket, `listen` with `<pid>` replaced by its
    /// process ID, which it returns.
    async fn spawn(
        mut command: tokio::process::Command,
        listen: &Path,
        session: Option<&str>,
        niri_args: &[&str],
    ) -> (Self, PathBuf) {
        command
            .args([
                "--exact",
                "--ignored",
                "--test-threads=1",
                "--quiet",
                "--",
                NAME,
            ])
            .args(niri_args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .kill_on_drop(true);
        if let Some(session) = session {
            command.env("XDG_SESSION_ID", session);
        }
        let child = command.spawn().unwrap();
        let pid = child.id().unwrap().to_string();
        let socket = PathBuf::from(listen.to_str().unwrap().replace("<pid>", &pid));
        assert!(eventually(Duration::from_secs(10), || socket.exists()).await);
        (Self { _process: child }, socket)
    }

    /// A plain `niri` on the fixture's `NIRI_SOCKET` that relays every connection to the
    /// socket `to`.
    pub(crate) async fn relaying(fixture: &Fixture, to: &Path) -> Self {
        std::fs::remove_file(fixture.niri_socket()).ok();
        let program = std::env::current_exe().unwrap();
        let mut command = command(&program);
        command.env(SOCKET, fixture.niri_socket()).env(RELAY, to);
        Self::spawn(command, &fixture.niri_socket(), None, &[])
            .await
            .0
    }

    /// A process called `niri` that serves `run/<display>` and relays every connection on
    /// `run/niri.<display>.<pid>.sock` to the socket `to`: a running niri as discovery
    /// finds it. Returns the socket's name too.
    pub(crate) async fn discoverable(
        fixture: &Fixture,
        display: &str,
        to: &Path,
    ) -> (Self, String) {
        let program = fixture.path("niri");
        std::os::unix::fs::symlink(std::env::current_exe().unwrap(), &program).unwrap();
        let display_socket = fixture.path(&format!("run/{display}"));
        // The fixture's own display, served by the test's process instead.
        std::fs::remove_file(&display_socket).ok();
        let listen = fixture.path(&format!("run/niri.{display}.<pid>.sock"));
        let mut command = command(&program);
        command
            .env(SOCKET, &listen)
            .env(RELAY, to)
            .env(DISPLAY_SOCKET, &display_socket);
        let (process, socket) = Self::spawn(command, &listen, None, &[]).await;
        assert!(eventually(Duration::from_secs(10), || display_socket.exists()).await);
        let name = socket.file_name().unwrap().to_str().unwrap().to_owned();
        (process, name)
    }

    /// A fake niri in session `c4`, with a `loginctl` that says it is unlocked, as the
    /// lease needs.
    pub(crate) async fn unlocked(fixture: &Fixture) -> Self {
        fixture.program("loginctl", "echo no");
        Self::start(fixture, Some("c4")).await
    }
}

/// The test binary at `program`, with an empty environment.
#[expect(
    clippy::disallowed_methods,
    reason = "the test binary starts itself as the fake niri"
)]
fn command(program: &Path) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(program);
    command.env_clear();
    command
}

/// The fake niri: answers `Version` and nothing else, until killed. Ignored, so it runs only
/// when `NiriProcess` starts it.
#[test]
#[ignore = "runs only as the fake niri process a test starts"]
fn fake_niri_process() {
    let Some(path) = std::env::var(SOCKET).ok() else {
        return;
    };
    let _display =
        std::env::var_os(DISPLAY_SOCKET).map(|display| UnixListener::bind(display).unwrap());
    let path = path.replace("<pid>", &std::process::id().to_string());
    let listener = UnixListener::bind(path).unwrap();
    let relay = std::env::var_os(RELAY);
    for connection in listener.incoming() {
        let Ok(connection) = connection else { continue };
        if let Some(to) = &relay {
            let to = to.clone();
            std::thread::spawn(move || relay_to(&connection, &to));
            continue;
        }
        std::thread::spawn(move || {
            let mut line = String::new();
            let mut reader = BufReader::new(&connection);
            if reader.read_line(&mut line).unwrap_or(0) > 0 && line.trim() == "\"Version\"" {
                (&connection)
                    .write_all(b"{\"Ok\":{\"Version\":\"26.04 (protocol-test)\"}}\n")
                    .ok();
            }
        });
    }
}

/// Copies each way between `connection` and a new connection to `to` until either side
/// closes.
fn relay_to(connection: &UnixStream, to: &std::ffi::OsStr) {
    let Ok(upstream) = UnixStream::connect(to) else {
        return;
    };
    let (Ok(mut from_client), Ok(mut to_upstream)) = (connection.try_clone(), upstream.try_clone())
    else {
        return;
    };
    let requests = std::thread::spawn(move || {
        std::io::copy(&mut from_client, &mut to_upstream).ok();
        to_upstream.shutdown(Shutdown::Write).ok();
    });
    std::io::copy(&mut &upstream, &mut &*connection).ok();
    connection.shutdown(Shutdown::Both).ok();
    requests.join().ok();
}
