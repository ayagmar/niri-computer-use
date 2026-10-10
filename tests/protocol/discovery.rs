//! A client that passes neither `NIRI_SOCKET` nor `WAYLAND_DISPLAY`, as Codex doesn't: the
//! server finds both in the fixture's runtime directory, by niri's own socket naming.

use std::os::unix::net::UnixListener;

use serde_json::json;

use crate::client::Server;
use crate::fixture::{DISPLAY, Fixture, fake};
use crate::niri::Niri;

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
    let _display = UnixListener::bind(fixture.path(&format!("run/{DISPLAY}"))).unwrap();

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
