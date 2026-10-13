//! The audit log as a client's calls leave it: one private line per call, metadata only.

use std::os::unix::fs::PermissionsExt as _;

use base64::Engine as _;
use serde_json::{Value, json};

use crate::client::{CLIENT, Server};
use crate::fixture::{Fixture, jpeg};
use crate::niri::{Niri, window};
use crate::noctalia::{self, UNLOCKED};

#[tokio::test]
async fn every_call_is_one_line_of_metadata_without_content() {
    let fixture = Fixture::new("audit");
    let mut niri = Niri::start(&fixture);
    fixture.program("noctalia", "exit 0");
    fixture.program("loginctl", "echo no");
    noctalia::start(&fixture, UNLOCKED);
    let image = jpeg(1280, 720, b"pixel-secret");
    fixture.grim(&image);
    let mut server = Server::start(&fixture).await;
    let stream = niri.stream().await;
    stream.initial(&[window(1, "title-secret")]);
    let tools = ["status", "desktop_state"];
    for tool in tools {
        server.structured(tool).await;
    }
    let shot = server
        .call("screenshot", json!({"target": "focused_output"}))
        .await;
    assert_eq!(shot["isError"], false);

    let lines = fixture.audit_lines();
    let logged: Vec<&str> = lines
        .iter()
        .map(|line| line["tool"].as_str().unwrap())
        .collect();
    assert_eq!(logged, [&tools[..], &["screenshot"]].concat());
    let session = format!("{CLIENT}/{}", server.pid);
    for line in &lines {
        assert_eq!(line["session"], session.as_str());
        assert_eq!(line["instance"], "niri.test.sock");
        assert_eq!(line["error"], Value::Null, "{line}");
        assert_eq!(line["accepted"], Value::Null);
        assert!(line["duration_ms"].is_u64());
        assert!(line["ts"].as_str().unwrap().ends_with('Z'));
    }
    assert_eq!(lines[2]["args"]["target"], "focused_output");

    let text = std::fs::read_to_string(fixture.audit_log()).unwrap();
    let encoded = base64::engine::general_purpose::STANDARD.encode(&image);
    for secret in ["title-secret", "pixel-secret", &encoded] {
        assert!(!text.contains(secret), "the log holds {secret}");
    }
    let mode = |path: &std::path::Path| path.metadata().unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&fixture.audit_log()), 0o600);
    assert_eq!(mode(fixture.audit_log().parent().unwrap()), 0o700);
}

#[tokio::test]
async fn a_failed_write_shows_in_status_and_the_call_still_succeeds() {
    let fixture = Fixture::new("audit-fails");
    let _niri = Niri::start(&fixture);
    // A file where the log's directory should be.
    std::fs::write(fixture.path("state/niri-computer-use"), "").unwrap();
    let mut server = Server::start(&fixture).await;
    let status = server.structured("status").await;
    assert_eq!(status["outputs"]["pointer_supported"], true);
    let audit = &server.structured("status").await["audit"];
    assert_eq!(audit["path"], fixture.audit_log().to_str().unwrap());
    // The directory that couldn't be made, named.
    let error = audit["last_error"].as_str().unwrap();
    let dir = fixture.path("state/niri-computer-use");
    assert!(
        error.starts_with(&format!("{}: ", dir.display())),
        "{error}"
    );
}

#[tokio::test]
async fn without_a_state_directory_calls_go_unlogged_and_status_says_so() {
    let mut fixture = Fixture::new("audit-none");
    fixture.unset("XDG_STATE_HOME");
    let mut server = Server::start(&fixture).await;
    let audit = &server.structured("status").await["audit"];
    assert_eq!(
        audit,
        &json!({"path": null, "last_error": "neither XDG_STATE_HOME nor HOME is set"})
    );
}

/// Claude Code 2.1.296 speaks MCP 2026-07-28: no `initialize`, and its name in every
/// request's `_meta`. Its sessions were logged as `unknown`.
#[tokio::test]
async fn a_client_named_only_in_each_requests_meta_is_logged_by_that_name() {
    let fixture = Fixture::new("audit-meta");
    let _niri = Niri::start(&fixture);
    let mut server = Server::spawn(&fixture);
    let meta = json!({"_meta": {
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientInfo": {"name": "claude-code", "version": "2.1.296"},
        "io.modelcontextprotocol/clientCapabilities": {},
    }});
    let discover = server.request("server/discover", meta.clone()).await;
    let discovered = server.response(discover).await;
    assert!(discovered.get("result").is_some(), "{discovered}");
    let mut call = meta;
    call["name"] = json!("status");
    call["arguments"] = json!({});
    let status = server.request("tools/call", call).await;
    let called = server.response(status).await;
    assert_eq!(called["result"]["isError"], false, "{called}");

    let lines = fixture.audit_lines();
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert_eq!(lines[0]["session"], format!("claude-code/{}", server.pid));
}
