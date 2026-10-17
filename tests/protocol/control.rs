//! The human-only control subcommands and what `status` reports about them.

use std::io::Write as _;
use std::os::unix::net::UnixListener;
use std::process::Stdio;
use std::time::Duration;

use rustix::fs::{OFlags, fcntl_getfl, fcntl_setfl};
use tokio::io::AsyncReadExt as _;

use crate::client::{Server, run, subcommand_to};
use crate::fixture::{DISPLAY, Fixture};
use crate::session::NiriProcess;

#[tokio::test]
async fn stop_and_resume_set_and_clear_the_flag_status_reports() {
    let fixture = Fixture::new("stop");
    let mut server = Server::start(&fixture).await;
    assert_eq!(server.structured("status").await["stop"], false);

    let stop = run(&fixture, "stop").await;
    assert!(stop.status.success(), "{stop:?}");
    assert_eq!(stop.stdout, b"");
    let flag = fixture.path("run/niri-computer-use/niri.test/stop");
    assert!(flag.exists());
    assert_eq!(server.structured("status").await["stop"], true);

    let resume = run(&fixture, "resume").await;
    assert!(resume.status.success(), "{resume:?}");
    assert!(!flag.exists());
    assert_eq!(server.structured("status").await["stop"], false);
}

#[tokio::test]
async fn resume_refuses_while_input_may_be_stuck() {
    let fixture = Fixture::new("resume-dirty");
    assert!(run(&fixture, "stop").await.status.success());
    std::fs::write(
        fixture.path("run/niri-computer-use/niri.test/input-dirty"),
        "",
    )
    .unwrap();
    let resume = run(&fixture, "resume").await;
    assert!(!resume.status.success());
    let stderr = String::from_utf8(resume.stderr).unwrap();
    assert!(stderr.contains("recover"), "{stderr}");
    assert!(
        fixture
            .path("run/niri-computer-use/niri.test/stop")
            .exists()
    );
}

#[tokio::test]
async fn stop_without_a_niri_instance_fails_and_says_why() {
    let mut fixture = Fixture::new("stop-unset");
    fixture.unset("NIRI_SOCKET");
    let stop = run(&fixture, "stop").await;
    assert!(!stop.status.success());
    let stderr = String::from_utf8(stop.stderr).unwrap();
    assert_eq!(
        stderr,
        format!(
            "niri-computer-use: NIRI_SOCKET is not set and {} has no socket of a running niri \
             on WAYLAND_DISPLAY {DISPLAY}\n",
            fixture.path("run").display()
        )
    );
}

/// niri hung: its socket takes no connection, yet `stop` and `resume` find it, without
/// connecting, and set and clear its flag. `resume` still refuses while input may be stuck.
#[tokio::test]
async fn stop_and_resume_work_while_niri_takes_no_connection() {
    let mut fixture = Fixture::new("stop-hung");
    fixture.unset("NIRI_SOCKET");
    let (_niri, socket) = NiriProcess::hung(&fixture, DISPLAY).await;
    let instance = socket.file_stem().unwrap().to_str().unwrap().to_owned();
    let flag = fixture.path(&format!("run/niri-computer-use/{instance}/stop"));
    let stop = run(&fixture, "stop").await;
    assert!(stop.status.success(), "{stop:?}");
    assert!(flag.exists());
    let resume = run(&fixture, "resume").await;
    assert!(resume.status.success(), "{resume:?}");
    assert!(!flag.exists());

    assert!(run(&fixture, "stop").await.status.success());
    std::fs::write(flag.with_file_name("input-dirty"), "").unwrap();
    assert!(!run(&fixture, "resume").await.status.success());
    assert!(flag.exists());
}

/// Without connecting, a socket counts only while its PID is a running niri, and two such
/// sockets are never chosen between: `stop` sets no flag and says why.
#[tokio::test]
async fn stop_refuses_a_stale_or_ambiguous_niri_socket() {
    let mut fixture = Fixture::new("stop-stale");
    fixture.unset("NIRI_SOCKET");
    // The test's own PID, which isn't a niri's.
    let stale = fixture.path(&format!("run/niri.{DISPLAY}.{}.sock", std::process::id()));
    drop(UnixListener::bind(&stale).unwrap());
    let none = refused_stop(&fixture).await;
    assert!(none.contains("has no socket of a running niri"), "{none}");

    let _first = NiriProcess::hung(&fixture, DISPLAY).await;
    let _second = NiriProcess::hung(&fixture, DISPLAY).await;
    let several = refused_stop(&fixture).await;
    assert!(several.contains("2 running niri instances"), "{several}");
    assert!(!fixture.path("run/niri-computer-use").exists());
}

/// What `stop` says on stderr when it fails, as it must.
async fn refused_stop(fixture: &Fixture) -> String {
    let stop = run(fixture, "stop").await;
    assert!(!stop.status.success(), "{stop:?}");
    String::from_utf8(stop.stderr).unwrap()
}

/// `--version` answers without discovery: a niri that could be found sees no connection.
#[tokio::test]
async fn version_prints_the_version_without_looking_for_a_session() {
    let mut fixture = Fixture::new("version");
    fixture.unset("NIRI_SOCKET");
    fixture.unset("WAYLAND_DISPLAY");
    // A running niri that discovery would find and connect to.
    let backend = fixture.path("run/backend.sock");
    let listener = UnixListener::bind(&backend).unwrap();
    listener.set_nonblocking(true).unwrap();
    let (_niri, _) = NiriProcess::discoverable(&fixture, DISPLAY, &backend).await;

    let version = run(&fixture, "--version").await;
    assert!(version.status.success(), "{version:?}");
    assert_eq!(
        String::from_utf8(version.stdout).unwrap(),
        format!("niri-computer-use {}\n", env!("CARGO_PKG_VERSION"))
    );
    tokio::time::sleep(Duration::from_millis(200)).await;
    let accepted = listener.accept().map(|_| ()).map_err(|error| error.kind());
    assert_eq!(accepted, Err(std::io::ErrorKind::WouldBlock));

    let unknown = run(&fixture, "--help-me").await;
    assert!(!unknown.status.success());
    let usage = String::from_utf8(unknown.stderr).unwrap();
    assert!(usage.contains("usage: niri-computer-use serve"), "{usage}");
}

/// `watch` whose reader keeps the pipe open but stops reading ends once its write deadline
/// passes, rather than waiting forever for the write it started.
#[tokio::test]
async fn watch_ends_when_its_reader_stops_reading() {
    let fixture = Fixture::new("watch-stuck");
    let (reader, mut writer) = std::io::pipe().unwrap();
    let flags = fcntl_getfl(&writer).unwrap();
    fcntl_setfl(&writer, flags | OFlags::NONBLOCK).unwrap();
    while writer.write(&[b'x'; 4096]).is_ok() {}
    fcntl_setfl(&writer, flags).unwrap();
    let mut watch = subcommand_to(&fixture, "watch", Stdio::from(writer));
    let status = tokio::time::timeout(Duration::from_secs(10), watch.wait())
        .await
        .expect("watch still runs 10 s after its reader stopped")
        .unwrap();
    let mut stderr = String::new();
    watch
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .await
        .unwrap();
    assert!(!status.success());
    assert!(
        stderr.contains("stdout's reader took more than 5 s to read a line"),
        "{stderr}"
    );
    drop(reader);
}
