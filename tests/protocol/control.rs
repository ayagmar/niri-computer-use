//! The human-only control subcommands and what `status` reports about them.

use crate::client::{Server, run};
use crate::fixture::{DISPLAY, Fixture};

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
