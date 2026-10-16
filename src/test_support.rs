//! Helpers shared by unit tests.

pub(crate) mod output_mode;
pub(crate) mod wayland;

use std::path::PathBuf;

/// Settings for a property test: `cases` inputs, and no file of failing inputs written into
/// the source tree. A failure's minimised input becomes a regression test of its own.
pub(crate) fn properties(cases: u32) -> proptest::test_runner::Config {
    proptest::test_runner::Config {
        cases,
        failure_persistence: None,
        ..proptest::test_runner::Config::default()
    }
}

/// A directory the calling test creates itself, so the test reads and removes nothing
/// else. Creation fails if the name already exists.
pub(crate) fn fresh_dir(name: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "niri-computer-use-{name}-{}-{nanos}",
        std::process::id()
    ));
    std::fs::create_dir(&dir).unwrap();
    dir
}

/// Writes `body` as an executable shell script at `file`. The `install` child writes it,
/// so this process never holds the file open for writing: a test spawning a program on
/// another thread meanwhile would inherit that handle, and running the script would fail
/// with "Text file busy".
#[expect(
    clippy::disallowed_methods,
    reason = "a test fixture written by a short-lived child, not a process the server runs"
)]
pub(crate) fn script(file: &std::path::Path, body: &str) {
    use std::io::Write as _;
    let mut install = std::process::Command::new("install")
        .args(["-m", "755", "/dev/stdin"])
        .arg(file)
        .stdin(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = install.stdin.take().unwrap();
    stdin
        .write_all(format!("#!/bin/sh\n{body}\n").as_bytes())
        .unwrap();
    drop(stdin);
    assert!(install.wait().unwrap().success());
}

/// The environment of a process whose niri socket is `niri.test.sock` in `dir`, resolved
/// as at its start: `dir` is made private and gets that socket file, which nothing
/// listens on.
pub(crate) fn niri_env(dir: &std::path::Path) -> crate::Env {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    let socket = dir.join("niri.test.sock");
    if !socket.exists() {
        drop(std::os::unix::net::UnixListener::bind(&socket).unwrap());
    }
    let euid = rustix::process::geteuid().as_raw();
    crate::Env {
        niri_socket: crate::niri::Socket::at(socket.clone()),
        instance: crate::control::runtime::Instance::resolve(&socket, euid),
        runtime_dir: Some(dir.to_path_buf()),
        ..crate::Env::default()
    }
}

/// A `PATH` with one directory that can't exist, so that nothing on the host runs.
pub(crate) const NO_PROGRAMS: &str = "/dev/null/no-programs";

/// Where a test session's programs are found: nowhere.
pub(crate) fn no_programs() -> crate::runner::Place<'static> {
    crate::runner::Place {
        path: Some(std::ffi::OsStr::new(NO_PROGRAMS)),
        cwd: None,
    }
}

/// Session `id` of one process, with no policy file, nothing turned on, and `NO_PROGRAMS`
/// as its `PATH`.
pub(crate) fn session(id: u64) -> crate::session::Session {
    let given = crate::session::Given {
        unrestricted: None,
        confirm_lease: None,
        tools: None,
        shared: None,
        home: None,
        path: Some(NO_PROGRAMS.into()),
        policy: crate::policy::Source::Missing,
    };
    crate::session::Session::new(
        crate::session::SessionId(id),
        std::process::id(),
        crate::session::Settings::new(given),
    )
}

/// A screenshot ref of a whole 960x720 output at scale 1.5, flipped, taken now.
pub(crate) fn shot() -> crate::refs::Shot {
    use niri_ipc::{LogicalOutput, Transform};

    let geometry = LogicalOutput {
        x: 0,
        y: 0,
        width: 960,
        height: 720,
        scale: 1.5,
        transform: Transform::Flipped180,
    };
    crate::refs::Shot {
        output: "winit".to_owned(),
        geometry,
        motion_geometry: Some(geometry),
        captured: crate::observe::Rect {
            x: 0,
            y: 0,
            width: 960,
            height: 720,
        },
        scale: 1.5,
        width: 1440,
        height: 1080,
        taken: tokio::time::Instant::now(),
        connection: 1,
        scene: crate::refs::scene::Scene::default(),
    }
}
