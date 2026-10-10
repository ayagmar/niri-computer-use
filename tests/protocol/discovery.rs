//! A client that passes neither `NIRI_SOCKET` nor `WAYLAND_DISPLAY`, as Codex doesn't: the
//! server finds both in the fixture's runtime directory, by niri's own socket naming. And
//! variables that name two compositors: nothing that reaches the display may run.

use serde_json::{Value, json};

use crate::client::{Server, tool_error};
use crate::fixture::{DISPLAY, Fixture, fake};
use crate::niri::{Niri, window_on};
use crate::noctalia::{self, UNLOCKED};
use crate::session::NiriProcess;

#[tokio::test]
async fn without_the_session_variables_the_server_finds_the_running_niri() {
    let mut fixture = Fixture::new("discover");
    fixture.unset("NIRI_SOCKET");
    fixture.unset("WAYLAND_DISPLAY");
    // A process called `niri`, whose PID names the socket the way niri's does.
    fixture.program("niri", "await_file never");
    let process = fake(&fixture, "niri");
    let name = format!("niri.{DISPLAY}.{}.sock", process.id().unwrap());
    let _niri = Niri::listen(&fixture.path(&format!("run/{name}")));

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
        "NIRI_SOCKET is not set and {} has no socket of a running niri",
        fixture.path("run").display()
    );
    assert_eq!(
        status["niri"]["error"],
        json!({"error": "niri_unavailable", "detail": detail})
    );
    assert_eq!(status["discovery"]["niri_socket"]["detail"], detail);
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
