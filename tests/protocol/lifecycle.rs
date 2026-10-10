//! The session from start to shutdown: the handshake, the tool list, requests out of
//! order, and closing stdin.

use serde_json::{Value, json};

use crate::client::{Server, WAIT};
use crate::fixture::{Fixture, eventually, shared_mode};
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
        "acquire_desktop" => (json!([false, false, true]), json!(null)),
        "release_desktop" => (json!([false, false, true]), json!(["restore_focus"])),
        "wait_for" => (json!([true, null, null]), json!(["until"])),
        "focus_window" | "focus_workspace" => (json!([false, false, true]), json!(["id"])),
        "close_window" => (json!([false, true, false]), json!(["id"])),
        "launch" => (json!([false, false, false]), json!(["preset"])),
        "niri_action" => (json!([false, true, false]), json!(["action"])),
        "screenshot" => (json!([true, null, null]), json!(["target"])),
        "pointer_move" => (json!([false, false, true]), json!(["screenshot_ref"])),
        "click" => (json!([false, true, false]), json!(["screenshot_ref"])),
        "drag" => (
            json!([false, true, false]),
            json!(["screenshot_ref", "from", "to"]),
        ),
        "scroll" => (
            json!([false, false, false]),
            json!(["screenshot_ref", "x", "y"]),
        ),
        "key" => (json!([false, true, false]), json!(["keys", "expect"])),
        "paste" => (
            json!([false, true, false]),
            json!(["text", "keys", "expect"]),
        ),
        "shell_open" | "shell_close" => (json!([false, false, true]), json!(["panel"])),
        "type_text" => (json!([false, true, false]), json!(["text", "expect"])),
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
            "key",
            "launch",
            "niri_action",
            "outputs",
            "paste",
            "pointer_move",
            "release_desktop",
            "screenshot",
            "scroll",
            "shell_close",
            "shell_open",
            "shell_status",
            "status",
            "type_text",
            "wait_for"
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
        properties_and_defaults(name, tool);
    }
}

/// How many arguments each tool takes; tools not listed take none.
const PROPERTIES: [(&str, usize); 17] = [
    ("screenshot", 5),
    ("launch", 3),
    ("niri_action", 2),
    ("focus_window", 2),
    ("focus_workspace", 2),
    ("close_window", 2),
    ("pointer_move", 5),
    ("click", 8),
    ("drag", 6),
    ("scroll", 7),
    ("key", 3),
    ("type_text", 4),
    ("paste", 4),
    ("shell_open", 2),
    ("shell_close", 2),
    ("release_desktop", 1),
    ("wait_for", 3),
];

/// `(tool, JSON pointer into its properties, expected value)`: advertised defaults and
/// bounds. Null means the field has no default.
fn advertised() -> [(&'static str, &'static str, Value); 18] {
    [
        ("screenshot", "/max_width/default", json!(1280)),
        ("screenshot", "/format/default", json!("jpeg")),
        ("screenshot", "/region/default", Value::Null),
        ("screenshot", "/save_path/default", Value::Null),
        ("launch", "/reuse/default", json!(false)),
        ("click", "/button/default", json!("left")),
        ("click", "/count/default", json!(1)),
        ("click", "/count/minimum", json!(1)),
        ("click", "/count/maximum", json!(3)),
        ("drag", "/button/default", json!("left")),
        ("click", "/keys/maxItems", json!(5)),
        ("drag", "/keys/maxItems", json!(5)),
        ("scroll", "/keys/maxItems", json!(5)),
        ("scroll", "/notches_y/default", json!(0)),
        ("key", "/keys/minItems", json!(1)),
        ("key", "/keys/maxItems", json!(16)),
        ("type_text", "/submit/default", json!(false)),
        ("wait_for", "/timeout_ms/default", json!(10000)),
    ]
}

/// How many arguments `tool` takes, and the defaults it advertises.
fn properties_and_defaults(name: &str, tool: &Value) {
    let fields = &tool["inputSchema"]["properties"];
    let properties = fields.as_object().map_or(0, serde_json::Map::len);
    let expected = PROPERTIES
        .iter()
        .find(|(listed, _)| *listed == name)
        .map_or(0, |(_, count)| *count);
    assert_eq!(properties, expected, "{tool}");
    for (_, pointer, value) in advertised()
        .into_iter()
        .filter(|(listed, ..)| *listed == name)
    {
        assert_eq!(
            fields.pointer(pointer).unwrap_or(&Value::Null),
            &value,
            "{name}{pointer}"
        );
    }
}

#[tokio::test]
async fn without_noctalia_on_path_there_are_no_shell_tools() {
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
            "key",
            "launch",
            "niri_action",
            "outputs",
            "paste",
            "pointer_move",
            "release_desktop",
            "screenshot",
            "scroll",
            "status",
            "type_text",
            "wait_for"
        ]
    );
    for tool in ["shell_status", "shell_open", "shell_close"] {
        let id = server
            .start_call(tool, json!({"panel": "control-center"}))
            .await;
        let response = server.response(id).await;
        assert!(response["error"]["code"].is_i64(), "{response}");
        assert_eq!(response.get("result"), None);
    }
}

#[tokio::test]
async fn without_an_accessibility_bus_there_are_no_element_tools_and_status_says_why() {
    let fixture = Fixture::new("no-a11y");
    let mut server = Server::start(&fixture).await;
    let tools = server.tools().await;
    for tool in ["elements", "activate_element", "set_element_text"] {
        assert!(!names(&tools).contains(&tool), "{tool}");
    }
    // Clients such as Codex forward XDG_RUNTIME_DIR but not DBUS_SESSION_BUS_ADDRESS, so
    // the server looks for the user bus in the runtime directory, which has none here.
    let accessibility = server.structured("status").await["accessibility"].clone();
    assert_eq!(accessibility["available"], false);
    let reason = accessibility["reason"].as_str().unwrap();
    let bus = fixture.dir.join("run/bus");
    assert!(reason.contains(bus.to_str().unwrap()), "{reason}");
    let id = server.start_call("elements", json!({"window_id": 1})).await;
    let response = server.response(id).await;
    assert!(response["error"]["code"].is_i64(), "{response}");
}

#[tokio::test]
async fn an_unknown_tool_is_a_protocol_error() {
    let fixture = Fixture::new("unknown");
    let mut server = Server::start(&fixture).await;
    let id = server
        .start_call("run_command", json!({"command": "x"}))
        .await;
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
    assert_eq!(unread, Vec::<Value>::new());
    failed_to_start(&fixture, status, &stderr).await;
}

/// The MCP session failed to start: the server exited with an error, or in shared mode the
/// engine logged it, while its bridge only relays.
async fn failed_to_start(fixture: &Fixture, status: std::process::ExitStatus, stderr: &str) {
    if shared_mode() {
        let logged = || fixture.engine_log().contains("start MCP session");
        assert!(eventually(WAIT, logged).await, "{}", fixture.engine_log());
    } else {
        assert!(!status.success());
        assert!(stderr.contains("start MCP session"), "{stderr}");
    }
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
    assert_eq!(unread, Vec::<Value>::new());
    failed_to_start(&fixture, status, &stderr).await;
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
