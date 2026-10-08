//! The session from start to shutdown: the handshake, the tool list, requests out of
//! order, and closing stdin.

use serde_json::{Value, json};

use crate::client::{Server, WAIT};
use crate::fixture::Fixture;
use crate::niri::{Niri, window};

fn names(tools: &[Value]) -> Vec<&str> {
    let mut names: Vec<&str> = tools
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    names.sort_unstable();
    names
}

#[tokio::test]
async fn the_handshake_negotiates_and_describes_the_server() {
    let fixture = Fixture::new("hello");
    let mut server = Server::spawn(&fixture);
    let response = server.initialize("2025-06-18").await;
    let result = &response["result"];
    assert_eq!(result["protocolVersion"], "2025-06-18");
    assert_eq!(result["serverInfo"]["name"], "niri-computer-use");
    assert!(result["capabilities"]["tools"].is_object(), "{result}");
    let instructions = result["instructions"].as_str().unwrap();
    assert!(
        instructions.contains("Start with `status`"),
        "{instructions}"
    );
    let (status, unread, _) = server.stop().await;
    assert!(status.success());
    assert_eq!(unread, Vec::<Value>::new());
}

#[tokio::test]
async fn an_unknown_protocol_version_gets_the_newest_supported_one() {
    let fixture = Fixture::new("version");
    let mut server = Server::spawn(&fixture);
    let response = server.initialize("2099-01-01").await;
    assert_eq!(response["result"]["protocolVersion"], "2025-11-25");
}

/// `[readOnlyHint, destructiveHint, idempotentHint]` and the required arguments of each
/// tool, by name.
fn expected(name: &str) -> (Value, Value) {
    let read_only = (json!([true, null, null]), json!(null));
    match name {
        "acquire_desktop" | "release_desktop" => (json!([false, false, true]), json!(null)),
        "focus_window" | "focus_workspace" => (json!([false, false, true]), json!(["id"])),
        "close_window" => (json!([false, true, false]), json!(["id"])),
        "launch" => (json!([false, false, false]), json!(["preset"])),
        "screenshot" => (json!([true, null, null]), json!(["target"])),
        "pointer_move" => (
            json!([false, false, true]),
            json!(["screenshot_ref", "x", "y"]),
        ),
        "click" => (
            json!([false, true, false]),
            json!(["screenshot_ref", "x", "y"]),
        ),
        "drag" => (
            json!([false, true, false]),
            json!(["screenshot_ref", "from", "to"]),
        ),
        "scroll" => (
            json!([false, false, false]),
            json!(["screenshot_ref", "x", "y"]),
        ),
        _ => read_only,
    }
}

#[tokio::test]
async fn the_tools_say_what_they_change_and_what_they_take() {
    let fixture = Fixture::new("tools");
    fixture.program("noctalia", "exit 0");
    let mut server = Server::start(&fixture).await;
    let tools = server.tools().await;
    assert_eq!(
        names(&tools),
        [
            "acquire_desktop",
            "click",
            "clipboard_read",
            "close_window",
            "desktop_state",
            "drag",
            "focus_window",
            "focus_workspace",
            "launch",
            "outputs",
            "pointer_move",
            "release_desktop",
            "screenshot",
            "scroll",
            "shell_status",
            "status"
        ]
    );
    for tool in &tools {
        let name = tool["name"].as_str().unwrap();
        let annotations = &tool["annotations"];
        let hints = json!([
            annotations["readOnlyHint"],
            annotations["destructiveHint"],
            annotations["idempotentHint"]
        ]);
        let (want_hints, required) = expected(name);
        assert_eq!(hints, want_hints, "{name}");
        assert_eq!(tool["inputSchema"]["required"], required, "{name}");
        assert!(!tool["description"].as_str().unwrap().is_empty(), "{name}");
        assert_eq!(tool["inputSchema"]["type"], "object", "{name}");
        // Optional arguments are absent or a value, never typed as nullable, and advertise
        // their real defaults.
        let schema = tool["inputSchema"].to_string();
        assert!(!schema.contains("\"null\""), "{schema}");
        let properties = tool["inputSchema"]["properties"]
            .as_object()
            .map_or(0, serde_json::Map::len);
        let fields = &tool["inputSchema"]["properties"];
        match name {
            "screenshot" => {
                assert_eq!(properties, 4, "{tool}");
                assert_eq!(fields["max_width"]["default"], 1280);
                assert_eq!(fields["format"]["default"], "jpeg");
                assert_eq!(fields["region"].get("default"), None);
            }
            "launch" => {
                assert_eq!(properties, 2, "{tool}");
                assert_eq!(fields["reuse"]["default"], false);
            }
            "focus_window" | "focus_workspace" | "close_window" => {
                assert_eq!(properties, 1, "{tool}");
            }
            "pointer_move" => assert_eq!(properties, 3, "{tool}"),
            "click" => {
                assert_eq!(properties, 5, "{tool}");
                assert_eq!(fields["button"]["default"], "left");
                assert_eq!(fields["count"]["default"], 1);
            }
            "drag" => {
                assert_eq!(properties, 4, "{tool}");
                assert_eq!(fields["button"]["default"], "left");
            }
            "scroll" => {
                assert_eq!(properties, 5, "{tool}");
                assert_eq!(fields["notches_y"]["default"], 0);
            }
            _ => assert_eq!(properties, 0, "{tool}"),
        }
    }
}

#[tokio::test]
async fn without_noctalia_on_path_there_is_no_shell_status() {
    let fixture = Fixture::new("no-shell");
    let mut server = Server::start(&fixture).await;
    let tools = server.tools().await;
    assert_eq!(
        names(&tools),
        [
            "acquire_desktop",
            "click",
            "clipboard_read",
            "close_window",
            "desktop_state",
            "drag",
            "focus_window",
            "focus_workspace",
            "launch",
            "outputs",
            "pointer_move",
            "release_desktop",
            "screenshot",
            "scroll",
            "status"
        ]
    );
    let id = server.start_call("shell_status", json!({})).await;
    let response = server.response(id).await;
    assert!(response["error"]["code"].is_i64(), "{response}");
    assert_eq!(response.get("result"), None);
}

#[tokio::test]
async fn an_unknown_tool_is_a_protocol_error() {
    let fixture = Fixture::new("unknown");
    let mut server = Server::start(&fixture).await;
    let id = server.start_call("type_text", json!({"text": "x"})).await;
    let response = server.response(id).await;
    assert!(response["error"]["code"].is_i64(), "{response}");
    assert_eq!(fixture.audit_lines(), Vec::<Value>::new());
}

#[tokio::test]
async fn a_request_before_initialize_is_refused_and_the_server_exits() {
    let fixture = Fixture::new("early");
    let mut server = Server::spawn(&fixture);
    let id = server.request("tools/list", json!({})).await;
    let response = server.response(id).await;
    assert!(response["error"]["code"].is_i64(), "{response}");
    let (status, unread, stderr) = server.stop().await;
    assert!(!status.success());
    assert_eq!(unread, Vec::<Value>::new());
    assert!(stderr.contains("start MCP session"), "{stderr}");
}

#[tokio::test]
async fn ping_works_before_initialize() {
    let fixture = Fixture::new("ping");
    let mut server = Server::spawn(&fixture);
    let id = server.request("ping", json!({})).await;
    assert_eq!(server.response(id).await["result"], json!({}));
    let response = server.initialize("2025-11-25").await;
    assert!(response.get("result").is_some(), "{response}");
}

#[tokio::test]
async fn stdin_closing_before_initialize_fails_with_nothing_on_stdout() {
    let fixture = Fixture::new("eof-early");
    let (status, unread, stderr) = Server::spawn(&fixture).stop().await;
    assert!(!status.success());
    assert_eq!(unread, Vec::<Value>::new());
    assert!(stderr.contains("start MCP session"), "{stderr}");
}

#[tokio::test]
async fn stdin_closing_shuts_the_server_down_and_closes_the_event_stream() {
    let fixture = Fixture::new("eof");
    let mut niri = Niri::start(&fixture);
    let mut server = Server::start(&fixture).await;
    let mut stream = niri.stream().await;
    stream.initial(&[window(1, "a")]);
    let state = server.structured("desktop_state").await;
    assert_eq!(state["windows"][0]["id"], 1);
    let (status, unread, stderr) = server.stop().await;
    assert!(status.success(), "{stderr}");
    assert_eq!(unread, Vec::<Value>::new());
    assert!(stream.closed_within(WAIT).await);
}
