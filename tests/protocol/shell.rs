//! Noctalia running, stopped and absent; the lock state from logind and Noctalia; and the
//! clipboard's three outcomes.

use serde_json::{Value, json};

use crate::client::{Server, tool_error};
use crate::fixture::{Fixture, SESSION};
use crate::noctalia::{self, LOCKED, UNLOCKED};

/// A loginctl that records its arguments and prints `hint`.
fn loginctl(fixture: &Fixture, hint: &str) {
    fixture.program(
        "loginctl",
        &format!(r#"printf '%s\n' "$@" > "$DIR/loginctl.args"; echo {hint}"#),
    );
}

#[tokio::test]
async fn noctalia_running_answers_shell_status() {
    let fixture = Fixture::new("noctalia-up");
    fixture.program("noctalia", "exit 0");
    loginctl(&fixture, "no");
    noctalia::start(&fixture, UNLOCKED);
    let mut server = Server::start(&fixture).await;
    let status = server.structured("status").await;
    assert_eq!(status["noctalia"], "running");
    assert_eq!(status["noctalia_error"], Value::Null);
    let shell = server.structured("shell_status").await;
    assert_eq!(shell, serde_json::from_str::<Value>(UNLOCKED).unwrap());
}

#[tokio::test]
async fn noctalia_stopped_is_not_running_with_the_reason() {
    let fixture = Fixture::new("noctalia-down");
    fixture.program("noctalia", "exit 0");
    loginctl(&fixture, "no");
    let mut server = Server::start(&fixture).await;
    let status = server.structured("status").await;
    assert_eq!(status["noctalia"], "not_running");
    assert_eq!(status["noctalia_error"]["error"], "noctalia_unavailable");
    let detail = status["noctalia_error"]["detail"].as_str().unwrap();
    assert!(detail.contains("connect"), "{detail}");
    let (name, _) = tool_error(&server.call("shell_status", json!({})).await);
    assert_eq!(name, "noctalia_unavailable");
}

#[tokio::test]
async fn noctalia_absent_from_path_is_not_installed() {
    let fixture = Fixture::new("noctalia-none");
    loginctl(&fixture, "no");
    // A Noctalia socket alone doesn't count: the tool list depends on `PATH`.
    noctalia::start(&fixture, UNLOCKED);
    let mut server = Server::start(&fixture).await;
    let status = server.structured("status").await;
    assert_eq!(status["noctalia"], "not_installed");
    assert_eq!(status["noctalia_error"], Value::Null);
    assert_eq!(status["lock"]["source"], "logind");
}

/// The `lock` block of `status` for one environment.
async fn lock(fixture: &Fixture) -> Value {
    let mut server = Server::start(fixture).await;
    server.structured("status").await["lock"].clone()
}

#[tokio::test]
async fn logind_answers_first_and_locked_wins() {
    let fixture = Fixture::new("lock-logind");
    fixture.program("noctalia", "exit 0");
    noctalia::start(&fixture, LOCKED);
    loginctl(&fixture, "yes");
    assert_eq!(
        lock(&fixture).await,
        json!({"state": "locked", "source": "logind", "logind_error": null})
    );
    assert_eq!(
        fixture.args("loginctl"),
        ["show-session", SESSION, "-p", "LockedHint", "--value"]
    );
    loginctl(&fixture, "no");
    assert_eq!(
        lock(&fixture).await,
        json!({"state": "locked", "source": "noctalia", "logind_error": null})
    );
}

#[tokio::test]
async fn without_logind_noctalia_decides_and_otherwise_the_state_is_unknown() {
    let mut fixture = Fixture::new("lock-fallback");
    fixture.program("noctalia", "exit 0");
    noctalia::start(&fixture, UNLOCKED);
    fixture.unset("XDG_SESSION_ID");
    assert_eq!(
        lock(&fixture).await,
        json!({"state": "unlocked", "source": "noctalia", "logind_error": "XDG_SESSION_ID is not set"})
    );
    fixture.set("XDG_SESSION_ID", "-H");
    let invalid = lock(&fixture).await;
    assert_eq!(invalid["state"], "unlocked");
    assert_eq!(
        invalid["logind_error"],
        "XDG_SESSION_ID \"-H\" isn't a logind session ID"
    );
    fixture.set("XDG_SESSION_ID", SESSION);
    fixture.program(
        "loginctl",
        "echo 'Failed to get session: No such session' >&2; exit 1",
    );
    // Noctalia's socket is named after the display, so this one has no Noctalia.
    fixture.set("WAYLAND_DISPLAY", "wayland-none");
    let unknown = lock(&fixture).await;
    assert_eq!(unknown["state"], "unknown");
    assert_eq!(unknown["source"], "none");
    let error = unknown["logind_error"].as_str().unwrap();
    assert!(error.contains("No such session"), "{error}");
}

#[tokio::test]
async fn the_clipboard_is_text_nothing_copied_or_no_text() {
    let fixture = Fixture::new("clipboard");
    let mut server = Server::start(&fixture).await;
    fixture.program(
        "wl-paste",
        r#"printf '%s\n' "$@" > "$DIR/wl-paste.args"; printf 'héllo\nworld'"#,
    );
    assert_eq!(
        server.structured("clipboard_read").await,
        json!({"text": "héllo\nworld", "reason": null})
    );
    assert_eq!(fixture.args("wl-paste"), ["--no-newline", "--type", "text"]);
    fixture.program("wl-paste", "echo 'Nothing is copied' >&2; exit 1");
    assert_eq!(
        server.structured("clipboard_read").await,
        json!({"text": null, "reason": "nothing_copied"})
    );
    fixture.program(
        "wl-paste",
        r#"echo 'Clipboard content is not available as requested type "text"' >&2; exit 1"#,
    );
    assert_eq!(
        server.structured("clipboard_read").await,
        json!({"text": null, "reason": "no_text"})
    );
}
