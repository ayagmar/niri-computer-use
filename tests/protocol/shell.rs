//! Noctalia running, stopped and absent; the lock state from logind and Noctalia; and the
//! clipboard as one lease sees it.

use std::time::Duration;

use niri_ipc::Action;
use serde_json::{Value, json};

use crate::client::{Server, run, tool_error};
use crate::clipboard::{Contents, HINT, Leased};
use crate::fixture::Fixture;
use crate::niri::{Niri, Stream, window_on};
use crate::noctalia::{self, LOCKED, UNLOCKED};
use crate::session::{self, NiriProcess};

/// A loginctl that records its arguments and prints `hint`.
fn loginctl(fixture: &Fixture, hint: &str) {
    fixture.program(
        "loginctl",
        &format!(r#"printf '%s\n' "$@" > "$DIR/loginctl.args"; echo {hint}"#),
    );
}

#[tokio::test]
async fn noctalia_running_is_reported_running() {
    let fixture = Fixture::new("noctalia-up");
    fixture.program("noctalia", "exit 0");
    loginctl(&fixture, "no");
    noctalia::start(&fixture, UNLOCKED);
    let mut server = Server::start(&fixture).await;
    let status = server.structured("status").await;
    assert_eq!(status["noctalia"], "running");
    assert_eq!(status["noctalia_error"], Value::Null);
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
}

#[tokio::test]
async fn noctalia_absent_from_path_is_not_installed() {
    let fixture = Fixture::new("noctalia-none");
    loginctl(&fixture, "no");
    // A Noctalia socket alone doesn't count: the tool list depends on `PATH`.
    noctalia::start(&fixture, UNLOCKED);
    let _niri = NiriProcess::start(&fixture, Some("c4")).await;
    let mut server = Server::start(&fixture).await;
    let status = server.structured("status").await;
    assert_eq!(status["noctalia"], "not_installed");
    assert_eq!(status["noctalia_error"], Value::Null);
    assert_eq!(status["lock"]["source"], "logind");
}

/// The `lock` block of `status`, with niri running in the session `session`.
async fn lock(fixture: &Fixture, session: Option<&str>) -> Value {
    let _niri = NiriProcess::start(fixture, session).await;
    let mut server = Server::start(fixture).await;
    server.structured("status").await["lock"].clone()
}

#[tokio::test]
async fn logind_is_asked_about_niris_session_and_locked_wins() {
    let fixture = Fixture::new("lock-logind");
    fixture.program("noctalia", "exit 0");
    noctalia::start(&fixture, LOCKED);
    loginctl(&fixture, "yes");
    assert_eq!(
        lock(&fixture, Some("c4")).await,
        json!({"state": "locked", "source": "logind", "session": "c4", "logind_error": null})
    );
    // The server's own XDG_SESSION_ID plays no part.
    assert_eq!(
        fixture.args("loginctl"),
        ["show-session", "c4", "-p", "LockedHint", "--value"]
    );
    loginctl(&fixture, "no");
    assert_eq!(
        lock(&fixture, Some("c4")).await,
        json!({"state": "locked", "source": "noctalia", "session": "c4", "logind_error": null})
    );
}

#[tokio::test]
async fn without_logind_noctalia_decides_and_otherwise_the_state_is_unknown() {
    let mut fixture = Fixture::new("lock-fallback");
    fixture.program("noctalia", "exit 0");
    noctalia::start(&fixture, UNLOCKED);
    loginctl(&fixture, "yes");
    assert_eq!(
        lock(&fixture, None).await,
        json!({
            "state": "unlocked", "source": "noctalia", "session": null,
            "logind_error": "niri has no XDG_SESSION_ID, so it sets no logind locked hint"
        })
    );
    let invalid = lock(&fixture, Some("-H")).await;
    assert_eq!(invalid["state"], "unlocked");
    assert_eq!(
        invalid["logind_error"],
        "XDG_SESSION_ID \"-H\" isn't a logind session ID"
    );
    fixture.program(
        "loginctl",
        "echo 'Failed to get session: No such session' >&2; exit 1",
    );
    // Noctalia's socket is named after the display, so this one has no Noctalia.
    fixture.set("WAYLAND_DISPLAY", "wayland-none");
    let unknown = lock(&fixture, Some("c4")).await;
    assert_eq!(unknown["state"], "unknown");
    assert_eq!(unknown["source"], "none");
    let error = unknown["logind_error"].as_str().unwrap();
    assert!(error.contains("No such session"), "{error}");
}

#[tokio::test]
async fn a_niri_without_session_is_not_asked_about_through_logind() {
    let fixture = Fixture::new("lock-plain");
    // logind would say `no` even on a locked screen: a plain niri never sets the hint.
    loginctl(&fixture, "no");
    let _niri = NiriProcess::plain(&fixture, Some("c4")).await;
    let mut server = Server::start(&fixture).await;
    assert_eq!(
        server.structured("status").await["lock"],
        json!({
            "state": "unknown", "source": "none", "session": null,
            "logind_error": "niri runs without --session, so it sets no logind locked hint"
        })
    );
    assert!(!fixture.path("loginctl.args").exists());
    let (name, _) = tool_error(&server.call("acquire_desktop", json!({})).await);
    assert_eq!(name, "screen_locked");
}

/// Moves the focus back to window 1, and waits until the server has seen it.
async fn focus_back(server: &mut Server, stream: &Stream) {
    stream.send(&json!({"WindowFocusChanged": {"id": 1}}));
    while server.structured("desktop_state").await["focused_window"] != 1 {
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
}

/// Noctalia says unlocked, then logind's answer, unlocked too, is held while the screen
/// locks and released past the last gate. The stale answer doesn't let `focus` or
/// `key` go out: the lock is read again and the call is refused.
#[tokio::test]
async fn a_lock_while_loginds_answer_is_held_refuses_the_call() {
    let mut fixture = Fixture::new("lock-held-logind");
    fixture.set("NIRI_COMPUTER_USE_KEYBOARD", "wtype");
    fixture.program("noctalia", "exit 0");
    fixture.program("wtype", r#"echo "$*" >> "$DIR/wtype.calls""#);
    session::held_loginctl(&fixture);
    let noctalia = noctalia::start(&fixture, UNLOCKED);
    let backend = fixture.path("run/backend.sock");
    let mut niri = Niri::listen(&backend);
    niri.effects(|action| {
        let Action::FocusWindow { id } = action else {
            return Vec::new();
        };
        vec![json!({"WindowFocusChanged": {"id": id}})]
    });
    let _relay = NiriProcess::in_session(&fixture, &backend).await;
    let mut server = Server::start(&fixture).await;
    let stream = niri.stream().await;
    stream.initial(&[
        window_on(1, Some("a"), 1, true),
        window_on(2, Some("b"), 1, false),
    ]);
    fixture.serve_programs();
    let points = fixture.points();
    // `key` first, as `focus` moves the focus `key` expects.
    for (tool, arguments) in [
        ("key", json!({"keys": ["Down"], "expect": {"window_id": 1}})),
        ("focus", json!({"window_id": 2})),
    ] {
        noctalia.set(UNLOCKED);
        std::fs::remove_file(fixture.path("locked")).ok();
        server.structured("acquire_desktop").await;
        // The last lock read before the call acts, as a call with nothing held reads it.
        points.start();
        let result = server.call(tool, arguments.clone()).await;
        assert_eq!(result["isError"], false, "{tool}: {result}");
        points.idle().await;
        let reads = points
            .trace()
            .iter()
            .filter(|passed| passed.point == "loginctl")
            .count();
        if tool == "focus" {
            niri.action().await;
            focus_back(&mut server, &stream).await;
        }
        std::fs::remove_file(fixture.path("wtype.calls")).ok();
        points.start();
        let held = points.hold("loginctl", reads);
        let id = server.start_call(tool, arguments).await;
        held.arrived().await;
        noctalia.set(LOCKED);
        std::fs::write(fixture.path("locked"), "").unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        held.release();
        let response = server.response(id).await;
        let (name, detail) = tool_error(&response["result"]);
        assert_eq!(name, "screen_locked", "{tool}: {detail}");
        assert!(!niri.sent_action(), "{tool}");
        assert!(!fixture.path("wtype.calls").exists(), "{tool}");
    }
}

/// Noctalia's status and logind's hint are read side by side, in `acquire_desktop`'s
/// readiness report and in its lock read after it, so a slow source doesn't add its time
/// to the other's: both replies are held at once.
#[tokio::test]
async fn noctalia_and_logind_are_asked_about_the_lock_side_by_side() {
    let fixture = Fixture::new("lock-side-by-side");
    fixture.program("noctalia", "exit 0");
    session::held_loginctl(&fixture);
    let _noctalia = noctalia::start(&fixture, UNLOCKED);
    let _niri = NiriProcess::start(&fixture, Some("c4")).await;
    let mut server = Server::start(&fixture).await;
    fixture.serve_programs();
    let points = fixture.points();
    points.start();
    let held: Vec<_> = (1..=2)
        .map(|nth| {
            (
                points.hold("noctalia:status", nth),
                points.hold("loginctl", nth),
            )
        })
        .collect();
    let id = server.start_call("acquire_desktop", json!({})).await;
    for (nth, (noctalia, logind)) in (1..).zip(held) {
        let both = Duration::from_secs(2);
        assert!(noctalia.arrived_within(both).await, "Noctalia's read {nth}");
        assert!(logind.arrived_within(both).await, "logind's read {nth}");
        noctalia.release();
        logind.release();
    }
    let response = server.response(id).await;
    assert_eq!(response["result"]["isError"], false, "{response}");
}

/// What the user copied: markup, then text, then an image, as a browser offers them.
fn copied(text: &str) -> Contents {
    vec![
        ("text/html", b"<b>markup</b>".to_vec()),
        ("text/plain;charset=utf-8", text.as_bytes().to_vec()),
        ("image/png", vec![0x89, b'P', b'N', b'G']),
    ]
}

/// The clipboard the lease began with is never read, nor is anything without the lease:
/// only what was copied since, such as a link an app's button copied.
#[tokio::test]
async fn only_what_was_copied_during_the_lease_is_read() {
    let mut leased = Leased::start("clipboard", Some(copied("the user's earlier copy"))).await;
    let before = json!({"text": null, "reason": "copied_before_lease"});
    assert_eq!(leased.read().await, before);

    leased.owner().copy(Some(copied("héllo\nworld")));
    assert_eq!(
        leased.read().await,
        json!({"text": "héllo\nworld", "reason": null})
    );
    for nothing in [None, Some(Vec::new())] {
        leased.owner().copy(nothing);
        assert_eq!(
            leased.read().await,
            json!({"text": null, "reason": "nothing_copied"})
        );
    }
    leased
        .owner()
        .copy(Some(vec![("image/png", vec![0x89, b'P', b'N', b'G'])]));
    assert_eq!(
        leased.read().await,
        json!({"text": null, "reason": "no_text"})
    );
    assert_eq!(leased.owner().reads(), ["text/plain;charset=utf-8"]);

    leased
        .owner()
        .copy(Some(copied("copied during the first lease")));
    let release = json!({"restore_focus": false});
    leased
        .server
        .structured_with("release_desktop", release)
        .await;
    let (name, _) = tool_error(&leased.server.call("clipboard_read", json!({})).await);
    assert_eq!(name, "lease_required");
    leased.server.structured("acquire_desktop").await;
    assert_eq!(leased.read().await, before, "a new lease starts afresh");
    assert_eq!(leased.owner().reads(), Vec::<String>::new());
}

/// The clipboard watcher a lease starts ends with the lease: at its release, and soon after
/// a stop takes the lease back.
#[tokio::test]
async fn the_clipboard_watcher_ends_with_the_lease() {
    let mut leased = Leased::start("clipboard-ends", None).await;
    let release = json!({"restore_focus": false});
    leased
        .server
        .structured_with("release_desktop", release)
        .await;
    assert!(leased.owner().closed_within(Duration::from_secs(1)).await);
    leased.server.structured("acquire_desktop").await;
    assert!(run(&leased.fixture, "stop").await.status.success());
    assert!(leased.owner().closed_within(Duration::from_secs(1)).await);
}

/// What a password manager copies is withheld without its text ever being read, and the
/// audit log holds neither. A hint of any other value hides nothing.
#[tokio::test]
async fn a_copy_marked_secret_is_withheld_unread() {
    let mut leased = Leased::start("clipboard-secret", None).await;
    let mut secret = copied("synthetic-password-7f3k");
    secret.push((HINT, b"secret".to_vec()));
    leased.owner().copy(Some(secret));
    assert_eq!(
        leased.read().await,
        json!({"text": null, "reason": "secret"})
    );
    assert_eq!(leased.owner().reads(), [HINT]);

    let mut public = copied("a public note");
    public.push((HINT, b"public".to_vec()));
    leased.owner().copy(Some(public));
    assert_eq!(
        leased.read().await,
        json!({"text": "a public note", "reason": null})
    );

    // A hint that can't be read is no answer: the call fails rather than read the text.
    let mut unreadable = copied("synthetic-password-7f3k");
    unreadable.push((HINT, [b"secret".as_slice(), &[b' '; 64]].concat()));
    leased.owner().copy(Some(unreadable));
    leased.owner().reads();
    let (name, _) = tool_error(&leased.server.call("clipboard_read", json!({})).await);
    assert_eq!(name, "upstream_error");
    assert_eq!(leased.owner().reads(), [HINT]);

    let audit = std::fs::read_to_string(leased.fixture.audit_log()).unwrap();
    assert!(!audit.contains("synthetic-password-7f3k"), "{audit}");
    assert!(!audit.contains("a public note"), "{audit}");
}
