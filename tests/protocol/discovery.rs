//! A client that passes neither `NIRI_SOCKET` nor `WAYLAND_DISPLAY`, as Codex doesn't: the
//! server finds both in the fixture's runtime directory, by niri's own socket naming. And
//! variables that name two compositors: nothing that reaches the display may run.

use std::os::unix::net::UnixListener;

use serde_json::{Value, json};

use crate::client::{Server, tool_error};
use crate::fixture::{DISPLAY, Fixture};
use crate::niri::{Niri, window_on};
use crate::noctalia::{self, UNLOCKED};
use crate::session::NiriProcess;

#[tokio::test]
async fn without_the_session_variables_the_server_finds_the_running_niri() {
    let mut fixture = Fixture::new("discover");
    fixture.unset("NIRI_SOCKET");
    fixture.unset("WAYLAND_DISPLAY");
    let backend = fixture.path("run/backend.sock");
    let _niri = Niri::listen(&backend);
    let (_process, name) = NiriProcess::discoverable(&fixture, DISPLAY, &backend).await;

    let mut server = Server::start(&fixture).await;
    let status = server.structured("status").await;
    assert_eq!(
        status["discovery"],
        json!({
            "runtime_dir": {"source": "environment"},
            "niri_socket": {"source": "discovered"},
            "wayland_display": {"source": "discovered"},
            "warning": null
        })
    );
    assert_eq!(status["instance"], name);
    assert_eq!(status["niri"]["version"], "26.04 (protocol-test)");
    assert_eq!(status["outputs"]["pointer_supported"], true);

    // Programs such as grim, wtype and wl-paste reach the display through these.
    fixture.program(
        "wl-paste",
        r#"printf '%s %s %s' "$XDG_RUNTIME_DIR" "$WAYLAND_DISPLAY" "$NIRI_SOCKET""#,
    );
    let run = fixture.path("run");
    let passed = format!("{} {DISPLAY} {}", run.display(), run.join(&name).display());
    assert_eq!(
        server.structured("clipboard_read").await,
        json!({"text": passed, "reason": null})
    );
}

#[tokio::test]
async fn without_a_running_niri_the_error_says_where_the_server_looked() {
    let mut fixture = Fixture::new("discover-none");
    fixture.unset("NIRI_SOCKET");
    let mut server = Server::start(&fixture).await;
    let status = server.structured("status").await;
    let detail = format!(
        "NIRI_SOCKET is not set and {} has no socket of a running niri on WAYLAND_DISPLAY \
         {DISPLAY}",
        fixture.path("run").display()
    );
    assert_eq!(
        status["niri"]["error"],
        json!({"error": "niri_unavailable", "detail": detail})
    );
    assert_eq!(status["discovery"]["niri_socket"]["detail"], detail);
}

/// libwayland connects to an inherited `WAYLAND_SOCKET` before it looks at
/// `WAYLAND_DISPLAY`, so a child given one would skip the display the server checked.
#[tokio::test]
async fn children_never_inherit_a_wayland_socket() {
    let mut fixture = Fixture::new("wayland-socket");
    fixture.set("WAYLAND_SOCKET", "3");
    let _niri = Niri::start(&fixture);
    fixture.program(
        "wl-paste",
        r#"printf '%s %s' "${WAYLAND_SOCKET-unset}" "$WAYLAND_DISPLAY""#,
    );
    let mut server = Server::start(&fixture).await;
    assert_eq!(
        server.structured("clipboard_read").await,
        json!({"text": format!("unset {DISPLAY}"), "reason": null})
    );
}

/// Sol's case: the only running niri is on another display than the one the client gave,
/// which wtype would reach while focus and policy were checked on that niri.
#[tokio::test]
async fn a_given_display_rules_out_a_niri_on_another_display() {
    let mut fixture = Fixture::new("discover-other");
    fixture.unset("NIRI_SOCKET");
    let backend = fixture.path("run/backend.sock");
    let _niri = Niri::listen(&backend);
    let _process = NiriProcess::discoverable(&fixture, "wayland-other", &backend).await;
    fixture.program("noctalia", "exit 0");
    let _noctalia = noctalia::start(&fixture, UNLOCKED);
    fixture.program("wtype", r#": > "$DIR/wtype.ran""#);
    let mut server = Server::start(&fixture).await;
    let detail = format!(
        "NIRI_SOCKET is not set and {} has no socket of a running niri on WAYLAND_DISPLAY \
         {DISPLAY}",
        fixture.path("run").display()
    );
    let unavailable = json!({"error": "niri_unavailable", "detail": detail});
    assert_eq!(
        server.structured("status").await["niri"]["error"],
        unavailable
    );
    for (tool, arguments) in [
        ("acquire_desktop", json!({})),
        (
            "type_text",
            json!({"text": "secret", "expect": {"app_id": "a"}}),
        ),
    ] {
        let result = server.call(tool, arguments).await;
        assert_eq!(result["structuredContent"], unavailable, "{tool}");
    }
    assert!(!fixture.path("wtype.ran").exists());
}

/// niri's socket is served by a process of its own, which relays to the fake niri, while
/// the test's process serves the fixture's display, as another compositor would. Focus,
/// policy, lock and lease all pass; only the display's peer is wrong.
#[tokio::test]
async fn a_display_niri_doesnt_serve_refuses_input_screenshots_and_the_clipboard() {
    let fixture = Fixture::new("discover-mismatch");
    let backend = fixture.path("run/backend.sock");
    let mut niri = Niri::listen(&backend);
    let _relay = NiriProcess::relaying(&fixture, &backend).await;
    fixture.program("noctalia", "exit 0");
    let _noctalia = noctalia::start(&fixture, UNLOCKED);
    for program in ["wtype", "grim", "wl-paste"] {
        fixture.program(program, &format!(": > \"$DIR/{program}.ran\""));
    }
    let mut server = Server::start(&fixture).await;
    let stream = niri.stream().await;
    stream.initial(&[window_on(1, Some("a"), 1, true)]);
    let status = server.structured("status").await;
    assert_eq!(status["niri"]["error"], Value::Null);
    assert_eq!(status["display_error"]["error"], "session_mismatch");
    let detail = status["display_error"]["detail"].as_str().unwrap();
    assert!(
        detail.starts_with(&format!(
            "the Wayland display {} is served by PID ",
            fixture.path(&format!("run/{DISPLAY}")).display()
        )),
        "{detail}"
    );
    assert!(
        detail.ends_with("set WAYLAND_DISPLAY and NIRI_SOCKET to the same niri's"),
        "{detail}"
    );

    server.structured("acquire_desktop").await;
    for (tool, arguments) in [
        (
            "type_text",
            json!({"text": "secret", "expect": {"app_id": "a"}}),
        ),
        ("screenshot", json!({"target": "focused_output"})),
        ("clipboard_read", json!({})),
    ] {
        let (name, refused) = tool_error(&server.call(tool, arguments).await);
        assert_eq!(
            (name.as_str(), refused.as_str()),
            ("session_mismatch", detail)
        );
    }
    for program in ["wtype", "grim", "wl-paste"] {
        assert!(
            !fixture.path(&format!("{program}.ran")).exists(),
            "{program} ran"
        );
    }
}

/// Sol's lifetime case: the display passes the check at startup, then is served by another
/// process, first while niri still runs and then after niri is gone. The server checks at
/// each use, so nothing that reaches the display runs.
#[tokio::test]
async fn a_display_replaced_after_startup_refuses_input_screenshots_and_the_clipboard() {
    let mut fixture = Fixture::new("display-replaced");
    fixture.unset("NIRI_SOCKET");
    let backend = fixture.path("run/backend.sock");
    let mut niri = Niri::listen(&backend);
    let (process, name) = NiriProcess::discoverable(&fixture, DISPLAY, &backend).await;
    fixture.program("noctalia", "exit 0");
    let _noctalia = noctalia::start(&fixture, UNLOCKED);
    for program in ["wtype", "grim", "wl-paste"] {
        fixture.program(program, &format!(": > \"$DIR/{program}.ran\""));
    }
    let mut server = Server::start(&fixture).await;
    let stream = niri.stream().await;
    stream.initial(&[window_on(1, Some("a"), 1, true)]);
    assert_eq!(
        server.structured("status").await["display_error"],
        Value::Null
    );
    server.structured("acquire_desktop").await;

    let display = fixture.path(&format!("run/{DISPLAY}"));
    std::fs::remove_file(&display).unwrap();
    let _replacement = UnixListener::bind(&display).unwrap();
    refuses_what_reaches_the_display(&mut server, "session_mismatch").await;

    drop(process);
    std::fs::remove_file(fixture.path(&format!("run/{name}"))).unwrap();
    refuses_what_reaches_the_display(&mut server, "niri_unavailable").await;
    for program in ["wtype", "grim", "wl-paste"] {
        assert!(
            !fixture.path(&format!("{program}.ran")).exists(),
            "{program} ran"
        );
    }
}

/// `status` shows `expected` under `display_error`, and input, a screenshot and a clipboard
/// read are refused with it.
async fn refuses_what_reaches_the_display(server: &mut Server, expected: &str) {
    assert_eq!(
        server.structured("status").await["display_error"]["error"],
        expected
    );
    for (tool, arguments) in [
        (
            "type_text",
            json!({"text": "secret", "expect": {"app_id": "a"}}),
        ),
        ("screenshot", json!({"target": "focused_output"})),
        ("clipboard_read", json!({})),
    ] {
        let (refused, detail) = tool_error(&server.call(tool, arguments).await);
        assert_eq!(refused, expected, "{tool}: {detail}");
    }
}
