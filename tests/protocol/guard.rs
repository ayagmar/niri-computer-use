//! The crash guardian `serve` starts: it outlives its server only long enough to release
//! what that server's marker names, and sends nothing without one. In shared mode the
//! engine is the server, and its log has what it and its guardian wrote to stderr.

use serde_json::json;

use crate::client::{Server, WAIT};
use crate::fixture::{Fixture, eventually, exited, kill, shared_mode};

const MARKER: &str = "run/niri-computer-use/niri.test/input-dirty";

/// The serving process and its guardian: its one child, running `guard <its PID>`.
async fn serving(server: &mut Server) -> (u32, i32) {
    let serving = server.serving_pid().await;
    (serving, guardian(serving))
}

fn guardian(server: u32) -> i32 {
    let children =
        std::fs::read_to_string(format!("/proc/{server}/task/{server}/children")).unwrap();
    let pid: i32 = children.split_whitespace().next().unwrap().parse().unwrap();
    let cmdline = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap();
    assert!(
        cmdline.ends_with(format!("guard\0{server}\0").as_bytes()),
        "{cmdline:?}"
    );
    pid
}

/// `SIGKILL`s the serving process. Returns what it and its guardian wrote to stderr, once
/// the guardian has exited.
async fn kill_serving(
    fixture: &Fixture,
    server: &mut Server,
    serving: u32,
    guardian: i32,
) -> String {
    let stderr = if shared_mode() {
        kill(serving);
        String::new()
    } else {
        server.kill().await
    };
    assert!(eventually(WAIT, || exited(guardian)).await);
    if shared_mode() {
        fixture.engine_log()
    } else {
        stderr
    }
}

#[tokio::test]
async fn a_server_that_exits_leaves_no_guardian_and_no_marker() {
    let fixture = Fixture::new("guard-exit");
    let mut server = Server::start(&fixture).await;
    let (_, pid) = serving(&mut server).await;
    let (status, _, stderr) = server.stop().await;
    assert!(status.success(), "{stderr}");
    assert!(eventually(WAIT, || exited(pid)).await);
    assert!(stderr.is_empty(), "{stderr}");
    assert_eq!(fixture.engine_log(), "");
    assert!(!fixture.path(MARKER).exists());
}

/// Without niri's Wayland display nothing can be released: the guardian says why and
/// leaves the marker as the server wrote it, unreleased, for `recover`.
#[tokio::test]
async fn a_killed_servers_marker_stays_unreleased_when_releasing_fails() {
    let fixture = Fixture::new("guard-kill");
    let mut server = Server::start(&fixture).await;
    let (holder, pid) = serving(&mut server).await;
    let marker = json!({
        "operation": "drag", "phase": "pending", "server_pid": holder,
        "since": "2026-10-10T00:00:00.000Z", "buttons": [272]
    });
    std::fs::create_dir_all(fixture.path(MARKER).parent().unwrap()).unwrap();
    std::fs::write(fixture.path(MARKER), marker.to_string()).unwrap();
    let stderr = kill_serving(&fixture, &mut server, holder, pid).await;
    assert!(stderr.contains("niri-computer-use: "), "{stderr}");
    let left: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(fixture.path(MARKER)).unwrap()).unwrap();
    assert_eq!(left, marker);
}

#[tokio::test]
async fn another_servers_marker_is_left_alone() {
    let fixture = Fixture::new("guard-other");
    let mut server = Server::start(&fixture).await;
    let (holder, pid) = serving(&mut server).await;
    let marker = json!({
        "operation": "drag", "phase": "pending", "server_pid": 1,
        "since": "2026-10-10T00:00:00.000Z", "buttons": [272]
    });
    std::fs::create_dir_all(fixture.path(MARKER).parent().unwrap()).unwrap();
    std::fs::write(fixture.path(MARKER), marker.to_string()).unwrap();
    let stderr = kill_serving(&fixture, &mut server, holder, pid).await;
    assert!(stderr.is_empty(), "{stderr}");
    let kept: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(fixture.path(MARKER)).unwrap()).unwrap();
    assert_eq!(kept, marker);
}
