//! The human-only control subcommands and what `status` reports about them.

use std::os::unix::net::UnixListener;

use crate::client::{Server, run};
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
