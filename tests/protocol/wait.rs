//! `wait_for` over stdio: waiting on the events the test sends, as niri would, and on the
//! screen through a fake grim.

use serde_json::{Value, json};

use crate::client::{Server, mistake};
use crate::fixture::{Fixture, jpeg};
use crate::niri::{Niri, Stream, window_on};

/// A server on a fake niri with windows 1 (focused, `a`, titled `t`) and 2 (`b`), and a
/// grim that always captures the same 1280-wide image. It doesn't take the lease.
async fn start(name: &str) -> (Fixture, Niri, Stream, Server) {
    let fixture = Fixture::new(name);
    fixture.grim(&jpeg(1280, 720, b"still"));
    let mut niri = Niri::start(&fixture);
    let server = Server::start(&fixture).await;
    let stream = niri.stream().await;
    stream.workspaces(1);
    stream.send(&json!({"WindowsChanged": {"windows": [
        window_on(1, Some("a"), 1, true),
        window_on(2, Some("b"), 1, false),
    ]}}));
    stream.send(&json!({"OverviewOpenedOrClosed": {"is_open": false}}));
    (fixture, niri, stream, server)
}

/// A window `id` with `app_id` and `title`, focused or not, as niri sends it when it opens
/// or changes.
fn opened(id: u64, app_id: &str, title: &str, focused: bool) -> Value {
    let mut window = window_on(id, Some(app_id), 1, focused);
    window["title"] = json!(title);
    json!({"WindowOpenedOrChanged": {"window": window}})
}

/// Starts `wait_for`, sends `events` a moment later, and returns the structured result
/// without `waited_ms`.
async fn waited(server: &mut Server, stream: &Stream, arguments: Value, events: &[Value]) -> Value {
    let id = server.start_call("wait_for", arguments).await;
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    for event in events {
        stream.send(event);
    }
    let result = server.response(id).await["result"].clone();
    assert_eq!(result["isError"], false, "{result}");
    let mut report = result["structuredContent"].clone();
    assert!(report["waited_ms"].is_u64(), "{report}");
    report.as_object_mut().unwrap().remove("waited_ms");
    report
}

#[tokio::test]
async fn a_window_that_appears_ends_the_wait_and_its_title_is_never_logged() {
    let (fixture, _niri, stream, mut server) = start("wait-window").await;
    let until = json!({"until": {"window": {"app_id": "dialog", "title": "Save"}}});
    let events = [
        opened(3, "dialog", "Open File", false),
        opened(4, "dialog", "Save As", false),
    ];
    assert_eq!(
        waited(&mut server, &stream, until, &events).await,
        json!({"observed": "met", "windows": [4], "focused_window": 1})
    );
    let logged = fixture
        .audit_lines()
        .into_iter()
        .find(|line| line["tool"] == "wait_for")
        .unwrap();
    assert_eq!(
        logged["args"],
        json!({"until": {"window": {"app_id": "dialog", "title_len": 4}}, "timeout_ms": 10000})
    );
}

#[tokio::test]
async fn closes_and_titles_end_the_wait_and_nothing_else_does() {
    let (_fixture, _niri, stream, mut server) = start("wait-follow").await;
    let closed = json!({"until": {"closed": 2}});
    assert_eq!(
        waited(
            &mut server,
            &stream,
            closed,
            &[json!({"WindowClosed": {"id": 2}})]
        )
        .await,
        json!({"observed": "met", "windows": [2], "focused_window": 1})
    );
    let title = json!({"until": {"title": {"window_id": 1, "contains": "done"}}});
    let events = [
        opened(1, "a", "make: running", true),
        opened(1, "a", "make: done", true),
    ];
    assert_eq!(
        waited(&mut server, &stream, title, &events).await["observed"],
        "met"
    );
    let never = json!({"until": {"window": {"app_id": "nope"}}, "timeout_ms": 200});
    assert_eq!(
        waited(&mut server, &stream, never, &[]).await,
        json!({"observed": "timeout", "focused_window": 1})
    );
}

#[tokio::test]
async fn conditions_that_cant_be_met_are_argument_mistakes() {
    let (_fixture, _niri, _stream, mut server) = start("wait-mistakes").await;
    for (until, says) in [
        (json!({"window": {}}), "`window` needs"),
        (
            json!({"title": {"window_id": 9, "contains": "x"}}),
            "no window with id 9",
        ),
        (
            json!({"title": {"window_id": 1, "contains": ""}}),
            "must not be empty",
        ),
    ] {
        let result = server.call("wait_for", json!({ "until": until })).await;
        assert!(mistake(&result).contains(says), "{result}");
    }
    let long = server
        .call(
            "wait_for",
            json!({"until": "screen_stable", "timeout_ms": 60000}),
        )
        .await;
    assert!(mistake(&long).contains("100 to 30000"), "{long}");
}

#[tokio::test]
async fn a_still_screen_is_met_with_its_screenshot() {
    let (_fixture, _niri, _stream, mut server) = start("wait-screen").await;
    let result = server
        .call(
            "wait_for",
            json!({"until": "screen_stable", "screenshot": true}),
        )
        .await;
    assert_eq!(result["isError"], false, "{result}");
    let report = &result["structuredContent"];
    assert_eq!(report["observed"], "met", "{report}");
    assert_eq!(report["screenshot"]["settled"], true, "{report}");
    assert_eq!(
        report["screenshot"]["screenshot_ref"],
        Value::Null,
        "{report}"
    );
    let content = result["content"].as_array().unwrap();
    assert_eq!(content.len(), 2, "{result}");
    assert_eq!(content[1]["type"], "image", "{result}");
}
