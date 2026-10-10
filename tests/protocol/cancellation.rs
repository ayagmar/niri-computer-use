//! Cancellation, deadlines and concurrent calls. A cancelled call gets no response, frees
//! what it held, and leaves the session usable.

use std::time::Duration;

use serde_json::json;

use crate::client::Server;
use crate::fixture::{Fixture, exited, jpeg, pid_in};
use crate::niri::Niri;

/// A program that starts a grandchild in its process group, records both process IDs and
/// then waits far longer than any deadline.
pub(crate) const STUCK: &str = r#"sleep 60 &
echo $! > "$DIR/grandchild.pid"
echo $$ > "$DIR/child.pid"
exec sleep 60"#;

/// Whether the stuck program and its grandchild have both exited within two seconds.
pub(crate) async fn stuck_program_exited(fixture: &Fixture) -> bool {
    let child = pid_in(&fixture.path("child.pid")).await;
    let grandchild = pid_in(&fixture.path("grandchild.pid")).await;
    crate::fixture::eventually(Duration::from_secs(2), || {
        exited(child) && exited(grandchild)
    })
    .await
}

/// Starts `tool`, cancels it once its program runs, and checks the program's process
/// group is gone, no response came, the log says `cancelled`, and the session goes on.
async fn cancel_while_running(name: &str, program: &str, tool: &str, arguments: serde_json::Value) {
    let fixture = Fixture::new(name);
    let _niri = Niri::start(&fixture);
    fixture.program(program, STUCK);
    let mut server = Server::start(&fixture).await;
    let id = server.start_call(tool, arguments).await;
    pid_in(&fixture.path("child.pid")).await;
    server.cancel(id).await;
    assert!(
        stuck_program_exited(&fixture).await,
        "{tool} left {program} running"
    );
    server.structured("outputs").await;
    assert!(
        !server.answered().contains(&id),
        "the cancelled {tool} was answered"
    );
    let lines = fixture.audit_lines();
    assert_eq!(lines[0]["tool"], tool);
    assert_eq!(lines[0]["error"], "cancelled");
    assert_eq!(lines[1]["tool"], "outputs");
    let (status, unread, _) = server.stop().await;
    assert!(status.success());
    assert!(unread.iter().all(|message| message["id"] != id));
}

#[tokio::test]
async fn cancelling_a_screenshot_kills_grim_and_its_group() {
    cancel_while_running(
        "cancel-grim",
        "grim",
        "screenshot",
        json!({"target": "focused_output"}),
    )
    .await;
}

#[tokio::test]
async fn cancelling_clipboard_read_kills_wl_paste_and_its_group() {
    cancel_while_running("cancel-paste", "wl-paste", "clipboard_read", json!({})).await;
}

#[tokio::test]
async fn cancelling_a_niri_request_closes_its_connection() {
    let fixture = Fixture::new("cancel-niri");
    let mut niri = Niri::start(&fixture);
    niri.set_silent(true);
    let mut server = Server::start(&fixture).await;
    let id = server.start_call("outputs", json!({})).await;
    niri.held().await;
    server.cancel(id).await;
    // Well inside the two-second request deadline.
    assert!(niri.abandoned_within(Duration::from_millis(500)).await);
    niri.set_silent(false);
    let outputs = server.structured("outputs").await;
    assert!(outputs.get("DP-1").is_some(), "{outputs}");
    assert!(!server.answered().contains(&id));
}

#[tokio::test]
async fn a_capture_past_its_deadline_is_killed_with_its_group() {
    let fixture = Fixture::new("deadline");
    let _niri = Niri::start(&fixture);
    fixture.program("grim", STUCK);
    let mut server = Server::start(&fixture).await;
    let result = server
        .call("screenshot", json!({"target": "focused_output"}))
        .await;
    let (name, detail) = crate::client::tool_error(&result);
    assert_eq!(name, "deadline_exceeded");
    assert_eq!(detail, "grim didn't finish within 5s");
    assert!(stuck_program_exited(&fixture).await);
}

#[tokio::test]
async fn a_held_niri_request_fails_at_its_deadline() {
    let fixture = Fixture::new("niri-deadline");
    let niri = Niri::start(&fixture);
    niri.set_silent(true);
    let mut server = Server::start(&fixture).await;
    let (name, _) = crate::client::tool_error(&server.call("outputs", json!({})).await);
    assert_eq!(name, "deadline_exceeded");
}

#[tokio::test]
async fn concurrent_calls_are_answered_as_each_finishes() {
    let fixture = Fixture::new("concurrent");
    let mut niri = Niri::start(&fixture);
    let image = jpeg(1280, 720, &[]);
    std::fs::write(fixture.path("grim.out"), &image).unwrap();
    fixture.program("grim", r#"sleep 1; cat "$DIR/grim.out""#);
    fixture.program("wl-paste", "printf text");
    let mut server = Server::start(&fixture).await;
    let stream = niri.stream().await;
    stream.initial(&[crate::niri::window(1, "a")]);
    let slow = server
        .start_call("screenshot", json!({"target": "focused_output"}))
        .await;
    let mut fast = Vec::new();
    for tool in ["status", "outputs", "desktop_state", "clipboard_read"] {
        fast.push(server.start_call(tool, json!({})).await);
    }
    for id in &fast {
        assert_eq!(server.response(*id).await["result"]["isError"], false);
    }
    assert!(
        !server.answered().contains(&slow),
        "the slow capture blocked the others"
    );
    assert_eq!(server.response(slow).await["result"]["isError"], false);
}
