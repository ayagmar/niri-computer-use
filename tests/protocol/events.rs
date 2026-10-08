//! niri's event stream through `desktop_state` and `status`: reconnects never serve the
//! old desktop, one malformed event reconnects, and a second stops the stream.

use std::time::Duration;

use serde_json::{Value, json};

use crate::client::{Server, tool_error};
use crate::fixture::Fixture;
use crate::niri::{Niri, window};

fn titles(state: &Value) -> Vec<&str> {
    state["windows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|window| window["title"].as_str().unwrap())
        .collect()
}

#[tokio::test]
async fn desktop_state_is_one_snapshot_of_the_stream() {
    let fixture = Fixture::new("snapshot");
    let mut niri = Niri::start(&fixture);
    let mut server = Server::start(&fixture).await;
    let stream = niri.stream().await;
    stream.initial(&[window(4, "four"), window(2, "two")]);
    let state = server.structured("desktop_state").await;
    assert_eq!(titles(&state), ["two", "four"]);
    assert_eq!(state["workspaces"][0]["id"], 1);
    assert_eq!(state["overview_open"], false);
    assert_eq!(state["keyboard_layouts"]["names"], json!(["English (US)"]));
    let status = server.structured("status").await;
    assert_eq!(status["niri"]["event_stream"], "connected");
    assert_eq!(status["niri"]["version"], "26.04 (protocol-test)");
    assert_eq!(status["niri"]["compat"], "ok");
    stream.send(&json!({"WindowClosed": {"id": 4}}));
    let mut closed = false;
    for _ in 0..50 {
        if titles(&server.structured("desktop_state").await) == ["two"] {
            closed = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(closed, "the closed window stayed in the snapshot");
}

#[tokio::test]
async fn a_reconnect_never_serves_the_old_desktop() {
    let fixture = Fixture::new("reconnect");
    let mut niri = Niri::start(&fixture);
    let mut server = Server::start(&fixture).await;
    let first = niri.stream().await;
    first.initial(&[window(1, "before")]);
    assert_eq!(
        titles(&server.structured("desktop_state").await),
        ["before"]
    );
    drop(first);
    // The server reconnects after a second; this stream sends nothing yet.
    let second = niri.stream().await;
    let (name, detail) = tool_error(&server.call("desktop_state", json!({})).await);
    assert_eq!(name, "deadline_exceeded", "{detail}");
    assert_eq!(
        server.structured("status").await["niri"]["event_stream"],
        "disconnected"
    );
    second.initial(&[window(2, "after")]);
    assert_eq!(titles(&server.structured("desktop_state").await), ["after"]);
}

#[tokio::test]
async fn one_malformed_event_reconnects_at_once_and_a_second_stops_the_stream() {
    let fixture = Fixture::new("malformed");
    let mut niri = Niri::start(&fixture);
    let mut server = Server::start(&fixture).await;
    let first = niri.stream().await;
    first.initial(&[window(1, "a")]);
    first.send(&json!({"NotARealEvent": {}}));
    // No reconnect delay after a malformed event.
    let second = niri
        .stream_within(Duration::from_millis(500))
        .await
        .expect("no immediate reconnect");
    second.initial(&[window(2, "b")]);
    assert_eq!(titles(&server.structured("desktop_state").await), ["b"]);
    // The count is for the server's lifetime, so a fresh connection doesn't reset it.
    second.send(&json!({"AlsoNotReal": {}}));
    let mut stopped = false;
    for _ in 0..50 {
        let status = server.structured("status").await;
        if status["niri"]["event_stream"] == "schema_incompatible" {
            stopped = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(stopped);
    let (name, detail) = tool_error(&server.call("desktop_state", json!({})).await);
    assert_eq!(name, "upstream_error");
    assert!(detail.contains("AlsoNotReal"), "{detail}");
    // Longer than the reconnect delay: the stream stays stopped.
    assert!(
        niri.stream_within(Duration::from_millis(1500))
            .await
            .is_none()
    );
}
