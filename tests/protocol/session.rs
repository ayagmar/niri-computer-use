//! A fake niri in a process of its own, so its environment and command line are exactly
//! what a test chooses. The server asks logind about the session in niri's
//! `XDG_SESSION_ID`, which it reads from `/proc/<niri pid>/environ`, and only when niri's
//! `/proc/<niri pid>/cmdline` has `--session`; the in-process fake niri would show the
//! test runner's instead. The test binary runs itself with `--ignored --exact` to become
//! this process. `--session` goes after `--`, where libtest takes it as one more test name
//! filter, which matches no test.

use std::io::{BufRead as _, BufReader, Write as _};
use std::os::unix::net::UnixListener;
use std::process::Stdio;
use std::time::Duration;

use crate::fixture::{Fixture, eventually};

const SOCKET: &str = "NCU_FAKE_NIRI_SOCKET";
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
        let mut command = command();
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
            .env_clear()
            .env(SOCKET, fixture.niri_socket())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .kill_on_drop(true);
        if let Some(session) = session {
            command.env("XDG_SESSION_ID", session);
        }
        // A socket left by an earlier fake niri in the same test.
        let socket = fixture.niri_socket();
        std::fs::remove_file(&socket).ok();
        let child = command.spawn().unwrap();
        assert!(eventually(Duration::from_secs(10), || socket.exists()).await);
        Self { _process: child }
    }

    /// A fake niri in session `c4`, with a `loginctl` that says it is unlocked, as the
    /// lease needs.
    pub(crate) async fn unlocked(fixture: &Fixture) -> Self {
        fixture.program("loginctl", "echo no");
        Self::start(fixture, Some("c4")).await
    }
}

#[expect(
    clippy::disallowed_methods,
    reason = "the test binary starts itself as the fake niri"
)]
fn command() -> tokio::process::Command {
    tokio::process::Command::new(std::env::current_exe().unwrap())
}

/// The fake niri: answers `Version` and nothing else, until killed. Ignored, so it runs only
/// when `NiriProcess` starts it.
#[test]
#[ignore = "runs only as the fake niri process a test starts"]
fn fake_niri_process() {
    let Some(path) = std::env::var_os(SOCKET) else {
        return;
    };
    let listener = UnixListener::bind(path).unwrap();
    for connection in listener.incoming() {
        let Ok(connection) = connection else { continue };
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
