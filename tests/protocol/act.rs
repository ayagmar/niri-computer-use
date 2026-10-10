//! The action tools over stdio: the gate before each action, what niri is asked to do, and
//! the outcome read from the events the test sends back, as niri would after the action.

use std::os::unix::net::UnixListener;

use niri_ipc::{Action, WorkspaceReferenceArg};
use serde_json::{Value, json};

use crate::client::{Server, mistake, run, tool_error};
use crate::fixture::{DISPLAY, Fixture, jpeg, shared_mode};
use crate::niri::{Niri, Stream, output, window_on};
use crate::noctalia::{self, LOCKED, UNLOCKED};

/// A server holding the lease on a fake niri, with windows 1 (focused, `a`), 2 (`b`) and
/// 3 (`c`) on workspace 1 of two, Noctalia saying the screen is unlocked, and a grim that
/// captures the focused 2560x1440 output at the default 1280 pixels wide.
///
/// Fields drop in this order: the server first, so it can't react to niri or Noctalia
/// going away, and the fixture last.
struct Desk {
    server: Server,
    niri: Niri,
    stream: Stream,
    noctalia: noctalia::Reply,
    fixture: Fixture,
}

impl Desk {
    async fn start(name: &str, policy: &str) -> Self {
        Self::start_with(name, policy, &[]).await
    }

    async fn start_backend(name: &str, policy: &str, backend: &str) -> Self {
        Self::start_with(name, policy, &[("NIRI_COMPUTER_USE_KEYBOARD", backend)]).await
    }

    /// As `start`, with these variables in the server's environment; the keyboard backend
    /// is wtype unless they choose another.
    async fn start_with(name: &str, policy: &str, vars: &[(&'static str, &str)]) -> Self {
        let mut fixture = Fixture::new(name);
        fixture.set("NIRI_COMPUTER_USE_KEYBOARD", "wtype");
        for (variable, value) in vars {
            fixture.set(variable, value);
        }
        std::fs::create_dir_all(fixture.path("config/niri-computer-use")).unwrap();
        std::fs::write(fixture.path("config/niri-computer-use/policy.toml"), policy).unwrap();
        fixture.program("noctalia", "exit 0");
        fixture.grim(&jpeg(1280, 720, b"evidence"));
        let noctalia = noctalia::start(&fixture, UNLOCKED);
        let mut niri = Niri::start(&fixture);
        let mut server = Server::start(&fixture).await;
        let stream = niri.stream().await;
        stream.workspaces(2);
        stream.send(&json!({"WindowsChanged": {"windows": [
            window_on(1, Some("a"), 1, true),
            window_on(2, Some("b"), 1, false),
            window_on(3, Some("c"), 1, false),
        ]}}));
        stream.send(&json!({"OverviewOpenedOrClosed": {"is_open": false}}));
        server.structured("acquire_desktop").await;
        Self {
            server,
            niri,
            stream,
            noctalia,
            fixture,
        }
    }

    /// Removes the Wayland display's socket, so input finds nothing to connect to.
    fn unplug_display(&self) {
        std::fs::remove_file(self.fixture.path(&format!("run/{DISPLAY}"))).unwrap();
    }

    /// Serves the display from the test's process again, as the fixture does.
    fn replug_display(&self) -> UnixListener {
        UnixListener::bind(self.fixture.path(&format!("run/{DISPLAY}"))).unwrap()
    }

    /// Calls `tool`, hands its action to `respond`, and returns the result.
    async fn act(
        &mut self,
        tool: &str,
        arguments: Value,
        respond: impl FnOnce(&Stream, Action),
    ) -> Value {
        let id = self.server.start_call(tool, arguments).await;
        let action = self.niri.action().await;
        respond(&self.stream, action);
        let response = self.server.response(id).await;
        response["result"].clone()
    }

    /// The audit log's `[tool, args, accepted, observed, error]` for the action tools.
    fn audited(&self) -> Vec<Value> {
        self.fixture
            .audit_lines()
            .into_iter()
            .filter(|line| {
                !["status", "acquire_desktop", "screenshot", "desktop_state"]
                    .contains(&line["tool"].as_str().unwrap())
            })
            .map(|line| {
                json!([
                    line["tool"],
                    line["args"],
                    line["accepted"],
                    line["observed"],
                    line["error"]
                ])
            })
            .collect()
    }
}

const PRESETS: &str = r#"
[[preset]]
name = "foot"
argv = ["foot"]
app_id = "foot"
"#;

/// The outcome, after checking that an outcome in doubt comes with a screenshot of the
/// focused output, the image after the text, and that no other outcome does. The
/// screenshot's metadata is left out of what it returns.
fn outcome(result: &Value) -> Value {
    assert_eq!(result["isError"], false, "{result}");
    let mut outcome = result["structuredContent"].clone();
    let content = result["content"].as_array().unwrap();
    let doubt = ["timeout", "pending", "none", "interrupted", "uncertain"];
    if doubt.contains(&outcome["observed"].as_str().unwrap()) {
        assert_eq!(content.len(), 2, "{result}");
        assert_eq!(content[1]["type"], "image", "{result}");
        assert_eq!(content[1]["mimeType"], "image/jpeg", "{result}");
        let screenshot = outcome
            .as_object_mut()
            .unwrap()
            .remove("screenshot")
            .unwrap();
        assert_eq!(screenshot["output"], "DP-1", "{screenshot}");
        assert_eq!(screenshot["width"], 1280, "{screenshot}");
        let id = screenshot["screenshot_ref"].as_str().unwrap();
        assert!(id.starts_with("shot-"), "{screenshot}");
    } else {
        assert_eq!(content.len(), 1, "{result}");
        assert_eq!(outcome.get("screenshot"), None, "{result}");
    }
    outcome
}

fn focus_changed(stream: &Stream, id: u64) {
    stream.send(&json!({"WindowFocusChanged": {"id": id}}));
}

#[tokio::test]
async fn actions_need_the_lease_and_check_the_lock_each_time() {
    let mut desk = Desk::start("act-gate", "").await;
    desk.noctalia.set(LOCKED);
    let (name, _) = tool_error(&desk.server.call("focus_window", json!({"id": 2})).await);
    assert_eq!(name, "screen_locked");
    desk.noctalia.set(UNLOCKED);
    desk.server
        .structured_with("release_desktop", json!({"restore_focus": false}))
        .await;
    let (unheld, detail) = tool_error(&desk.server.call("focus_window", json!({"id": 2})).await);
    assert_eq!(unheld, "lease_required");
    assert!(detail.contains("acquire_desktop"), "{detail}");
    assert!(!desk.niri.sent_action());
    assert_eq!(
        desk.audited(),
        [
            json!(["focus_window", {"id": 2}, null, null, "screen_locked"]),
            json!(["release_desktop", {"restore_focus": false}, null, null, null]),
            json!(["focus_window", {"id": 2}, null, null, "lease_required"]),
        ]
    );
}

#[tokio::test]
async fn focus_window_is_accepted_then_observed() {
    let mut desk = Desk::start("act-focus", "").await;
    let result = desk
        .act("focus_window", json!({"id": 2}), |stream, action| {
            assert!(
                matches!(action, Action::FocusWindow { id: 2 }),
                "{action:?}"
            );
            focus_changed(stream, 2);
        })
        .await;
    assert_eq!(
        outcome(&result),
        json!({"accepted": true, "observed": "focused", "focused_window": 2})
    );
    let unknown = mistake(&desk.server.call("focus_window", json!({"id": 9})).await);
    assert_eq!(
        unknown,
        "invalid arguments: no window with id 9; desktop_state lists them"
    );
    assert!(!desk.niri.sent_action());
    assert_eq!(
        desk.audited(),
        [
            json!(["focus_window", {"id": 2}, true, "focused", null]),
            json!(["focus_window", {"id": 9}, null, null, "invalid_arguments"]),
        ]
    );
}

#[tokio::test]
async fn a_target_that_already_has_focus_sends_nothing() {
    let mut desk = Desk::start("act-focused", "").await;
    let window = desk.server.call("focus_window", json!({"id": 1})).await;
    assert_eq!(
        outcome(&window),
        json!({"accepted": false, "observed": "focused", "focused_window": 1})
    );
    // niri's `workspace-auto-back-and-forth` would switch away from it.
    let workspace = desk.server.call("focus_workspace", json!({"id": 1})).await;
    assert_eq!(
        outcome(&workspace),
        json!({"accepted": false, "observed": "focused", "focused_window": 1})
    );
    assert!(!desk.niri.sent_action());
}

#[tokio::test]
async fn focus_moving_elsewhere_during_the_wait_is_interrupted() {
    let mut desk = Desk::start("act-interrupt", "").await;
    let result = desk
        .act("focus_window", json!({"id": 2}), |stream, _| {
            focus_changed(stream, 3);
            focus_changed(stream, 2);
        })
        .await;
    assert_eq!(
        outcome(&result),
        json!({"accepted": true, "observed": "interrupted", "focused_window": 3})
    );
}

#[tokio::test]
async fn focus_leaving_the_target_workspace_or_a_launch_is_interrupted() {
    let mut desk = Desk::start("act-interrupt-more", PRESETS).await;
    let window = window_on(4, Some("d"), 2, false);
    desk.stream
        .send(&json!({"WindowOpenedOrChanged": {"window": window}}));
    // A window on workspace 2 is where focus may go; window 3, on workspace 1, isn't.
    let result = desk
        .act("focus_workspace", json!({"id": 2}), |stream, _| {
            focus_changed(stream, 4);
            focus_changed(stream, 3);
        })
        .await;
    assert_eq!(
        outcome(&result),
        json!({"accepted": true, "observed": "interrupted", "focused_window": 3})
    );
    let launch = desk
        .act("launch", json!({"preset": "foot"}), |stream, _| {
            focus_changed(stream, 2);
        })
        .await;
    assert_eq!(
        outcome(&launch),
        json!({"accepted": true, "observed": "interrupted", "focused_window": 2})
    );
}

#[tokio::test]
async fn a_single_instance_app_answers_a_launch_with_its_window() {
    let mut desk = Desk::start("act-single", PRESETS).await;
    let existing = window_on(5, Some("foot"), 1, false);
    desk.stream
        .send(&json!({"WindowOpenedOrChanged": {"window": existing}}));
    let mut answered = Value::Null;
    // The new window may not have reached the server yet: ask until it has.
    for _ in 0..50 {
        let result = desk
            .act("launch", json!({"preset": "foot"}), |stream, _| {
                focus_changed(stream, 5);
            })
            .await;
        if outcome(&result)["observed"] == "focused" {
            answered = outcome(&result);
            break;
        }
        focus_changed(&desk.stream, 1);
    }
    assert_eq!(
        answered,
        json!({"accepted": true, "observed": "focused", "focused_window": 5, "windows": [5]})
    );
}

#[tokio::test]
async fn focus_without_its_event_times_out() {
    let mut desk = Desk::start("act-timeout", "").await;
    let result = desk.act("focus_window", json!({"id": 2}), |_, _| {}).await;
    assert_eq!(
        outcome(&result),
        json!({"accepted": true, "observed": "timeout", "focused_window": 1})
    );
}

#[tokio::test]
async fn screenshots_under_the_lease_are_refs_of_that_lease() {
    let mut desk = Desk::start("act-refs", "").await;
    let shot = json!({"target": "focused_output"});
    let screenshot_ref = |result: Value| result["structuredContent"]["screenshot_ref"].clone();
    let first = screenshot_ref(desk.server.call("screenshot", shot.clone()).await);
    // `shot-<the server's tag>-<number>`.
    let tag = first
        .as_str()
        .and_then(|id| id.strip_prefix("shot-")?.strip_suffix("-1"))
        .unwrap()
        .to_owned();
    let evidence = desk.act("focus_window", json!({"id": 2}), |_, _| {}).await;
    assert_eq!(
        evidence["structuredContent"]["screenshot"]["screenshot_ref"],
        format!("shot-{tag}-2")
    );
    desk.server
        .structured_with("release_desktop", json!({"restore_focus": false}))
        .await;
    let unleased = desk.server.call("screenshot", shot.clone()).await;
    assert_eq!(screenshot_ref(unleased), Value::Null);
    desk.server.structured("acquire_desktop").await;
    let again = desk.server.call("screenshot", shot).await;
    assert_eq!(screenshot_ref(again), format!("shot-{tag}-3"));
}

/// A client whose server was restarted, or a bridge reconnected to a new engine, may still
/// hold a ref the earlier process issued: it must not name the new process's capture.
#[tokio::test]
async fn a_ref_from_another_server_process_is_unknown() {
    let mut earlier = Desk::start("act-refs-a", "").await;
    let mut now = Desk::start("act-refs-b", "").await;
    let old = screenshot_ref(&mut earlier).await;
    let new = screenshot_ref(&mut now).await;
    assert_ne!(old, new);
    let at = |id: &str| json!({"screenshot_ref": id, "x": 10, "y": 10});
    let moved = now.server.call("pointer_move", at(&old)).await;
    assert_eq!(reason(&moved), ("ref_invalid", "unknown_ref".to_owned()));
}

#[tokio::test]
async fn a_failed_capture_keeps_the_outcome_and_says_why() {
    let mut desk = Desk::start("act-no-shot", "").await;
    desk.fixture
        .program("grim", "echo 'compositor gone' >&2; exit 1");
    let result = desk.act("focus_window", json!({"id": 2}), |_, _| {}).await;
    assert_eq!(result["isError"], false, "{result}");
    assert_eq!(result["content"].as_array().unwrap().len(), 1, "{result}");
    let outcome = &result["structuredContent"];
    assert_eq!(outcome["observed"], "timeout");
    assert_eq!(outcome["screenshot_error"]["error"], "upstream_error");
    let detail = outcome["screenshot_error"]["detail"].as_str().unwrap();
    assert!(detail.contains("compositor gone"), "{detail}");
    assert_eq!(
        desk.audited(),
        [json!(["focus_window", {"id": 2}, true, "timeout", null])]
    );
}

#[tokio::test]
async fn a_lost_reply_is_uncertain() {
    let mut desk = Desk::start("act-lost", "").await;
    desk.niri.hold_actions(true);
    let result = desk.act("focus_window", json!({"id": 2}), |_, _| {}).await;
    let outcome = outcome(&result);
    assert_eq!(outcome["accepted"], Value::Null);
    assert_eq!(outcome["observed"], "uncertain");
    let detail = outcome["detail"].as_str().unwrap();
    assert!(detail.contains("no reply within 2s"), "{detail}");
}

#[tokio::test]
async fn focus_workspace_waits_for_the_workspace() {
    let mut desk = Desk::start("act-workspace", "").await;
    let result = desk
        .act("focus_workspace", json!({"id": 2}), |stream, action| {
            assert!(
                matches!(
                    action,
                    Action::FocusWorkspace {
                        reference: WorkspaceReferenceArg::Id(2)
                    }
                ),
                "{action:?}"
            );
            // niri reports the workspace first; focus is still on window 1 until the
            // next event moves it off, as workspace 2 is empty.
            stream.send(&json!({"WorkspaceActivated": {"id": 2, "focused": true}}));
            stream.send(&json!({"WindowFocusChanged": {"id": null}}));
        })
        .await;
    assert_eq!(
        outcome(&result),
        json!({"accepted": true, "observed": "focused", "focused_window": null})
    );
    let unknown = mistake(&desk.server.call("focus_workspace", json!({"id": 7})).await);
    assert!(unknown.contains("no workspace with id 7"), "{unknown}");
}

#[tokio::test]
async fn close_window_is_closed_or_pending() {
    let mut desk = Desk::start("act-close", "").await;
    let result = desk
        .act("close_window", json!({"id": 2}), |stream, action| {
            assert!(
                matches!(action, Action::CloseWindow { id: Some(2) }),
                "{action:?}"
            );
            stream.send(&json!({"WindowClosed": {"id": 2}}));
        })
        .await;
    assert_eq!(
        outcome(&result),
        json!({"accepted": true, "observed": "closed", "focused_window": 1, "windows": [2]})
    );
    // An app asking about unsaved changes keeps its window open.
    let pending = desk.act("close_window", json!({"id": 3}), |_, _| {}).await;
    assert_eq!(
        outcome(&pending),
        json!({"accepted": true, "observed": "pending", "focused_window": 1, "windows": [3]})
    );
    assert_eq!(
        desk.audited(),
        [
            json!(["close_window", {"id": 2}, true, "closed", null]),
            json!(["close_window", {"id": 3}, true, "pending", null]),
        ]
    );
}

#[tokio::test]
async fn launch_spawns_the_presets_argv_and_counts_new_windows() {
    let mut desk = Desk::start("act-launch", PRESETS).await;
    let status = desk.server.structured("status").await;
    assert_eq!(status["policy"]["preset_names"], json!(["foot"]));
    // The app maps its window first and sets its app_id later.
    let result = desk
        .act("launch", json!({"preset": "foot"}), |stream, action| {
            let Action::Spawn { command } = action else {
                panic!("{action:?}");
            };
            assert_eq!(command, ["foot"]);
            let opened = window_on(7, None, 1, true);
            stream.send(&json!({"WindowOpenedOrChanged": {"window": opened}}));
            let named = window_on(7, Some("foot"), 1, true);
            stream.send(&json!({"WindowOpenedOrChanged": {"window": named}}));
        })
        .await;
    assert_eq!(
        outcome(&result),
        json!({"accepted": true, "observed": "one", "focused_window": 7, "windows": [7]})
    );
    let several = desk
        .act("launch", json!({"preset": "foot"}), |stream, _| {
            for id in [8, 9] {
                let window = window_on(id, Some("foot"), 1, false);
                stream.send(&json!({"WindowOpenedOrChanged": {"window": window}}));
            }
        })
        .await;
    assert_eq!(
        outcome(&several),
        json!({"accepted": true, "observed": "ambiguous", "focused_window": 7, "windows": [8, 9]})
    );
    let (name, detail) = tool_error(&desk.server.call("launch", json!({"preset": "sh"})).await);
    assert_eq!(name, "unknown_preset");
    assert_eq!(
        detail,
        "no preset named \"sh\"; the policy file has [\"foot\"]"
    );
    assert_eq!(
        desk.audited(),
        [
            json!(["launch", {"preset": "foot", "reuse": false}, true, "one", null]),
            json!(["launch", {"preset": "foot", "reuse": false}, true, "ambiguous", null]),
            json!(["launch", {"preset": "sh", "reuse": false}, null, null, "unknown_preset"]),
        ]
    );
}

#[tokio::test]
async fn a_launch_without_a_matching_window_observes_none() {
    let mut desk = Desk::start("act-launch-none", PRESETS).await;
    let result = desk
        .act("launch", json!({"preset": "foot"}), |stream, _| {
            let other = window_on(7, Some("other"), 1, false);
            stream.send(&json!({"WindowOpenedOrChanged": {"window": other}}));
        })
        .await;
    assert_eq!(
        outcome(&result),
        json!({"accepted": true, "observed": "none", "focused_window": 1})
    );
}

#[tokio::test]
async fn reuse_spawns_for_none_focuses_one_and_starts_nothing_for_two() {
    let mut desk = Desk::start("act-reuse", PRESETS).await;
    let reuse = json!({"preset": "foot", "reuse": true});
    let result = desk
        .act("launch", reuse.clone(), |stream, action| {
            assert!(matches!(action, Action::Spawn { .. }), "{action:?}");
            let window = window_on(7, Some("foot"), 1, true);
            stream.send(&json!({"WindowOpenedOrChanged": {"window": window}}));
        })
        .await;
    assert_eq!(outcome(&result)["observed"], "one");
    focus_changed(&desk.stream, 1);
    let focused = desk
        .act("launch", reuse.clone(), |stream, action| {
            assert!(
                matches!(action, Action::FocusWindow { id: 7 }),
                "{action:?}"
            );
            focus_changed(stream, 7);
        })
        .await;
    assert_eq!(
        outcome(&focused),
        json!({"accepted": true, "observed": "focused", "focused_window": 7, "windows": [7]})
    );
    let second = window_on(8, Some("foot"), 1, false);
    desk.stream
        .send(&json!({"WindowOpenedOrChanged": {"window": second}}));
    // The new window may not have reached the server yet: ask until it has.
    let mut ambiguous = Value::Null;
    for _ in 0..50 {
        let again = desk.server.call("launch", reuse.clone()).await;
        if outcome(&again)["observed"] == "ambiguous" {
            ambiguous = outcome(&again);
            break;
        }
        // Not yet: window 7 already had focus, so nothing was sent.
        assert_eq!(outcome(&again)["accepted"], false, "{again}");
    }
    assert_eq!(
        ambiguous,
        json!({"accepted": false, "observed": "ambiguous", "focused_window": 7, "windows": [7, 8]})
    );
    assert!(!desk.niri.sent_action());
}

#[tokio::test]
async fn a_stop_cancels_the_running_action_and_takes_the_lease_back() {
    let mut desk = Desk::start("act-stop", "").await;
    let id = desk
        .server
        .start_call("focus_window", json!({"id": 2}))
        .await;
    desk.niri.action().await;
    assert!(run(&desk.fixture, "stop").await.status.success());
    let response = desk.server.response(id).await;
    let (name, detail) = tool_error(&response["result"]);
    assert_eq!(name, "stopped");
    assert!(detail.contains("cancelled this action"), "{detail}");
    let mut released = false;
    for _ in 0..250 {
        if desk.server.structured("status").await["lease"]["held_by_me"] == false {
            released = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(released, "the stop didn't take the lease back");
    let (after, _) = tool_error(&desk.server.call("focus_window", json!({"id": 2})).await);
    assert_eq!(after, "stopped");
}

#[tokio::test]
async fn a_stop_ends_the_owners_queued_actions_and_frees_the_lease_within_a_second() {
    use std::time::{Duration, Instant};

    let mut desk = Desk::start("act-stop-queue", "").await;
    recording_wtype(&desk.fixture, "await_file go");
    let key = json!({"keys": ["Down"], "expect": "none"});
    let mut calls = Vec::new();
    for _ in 0..16 {
        calls.push(desk.server.start_call("key", key.clone()).await);
    }
    // The first runs, blocked in wtype, and the other fifteen wait for it.
    let calls_file = desk.fixture.path("wtype.calls");
    assert!(crate::fixture::eventually(Duration::from_secs(2), || calls_file.exists()).await);
    tokio::time::sleep(Duration::from_millis(200)).await;
    let stopped = Instant::now();
    assert!(run(&desk.fixture, "stop").await.status.success());
    for id in calls {
        let (name, detail) = tool_error(&desk.server.response(id).await["result"]);
        assert_eq!(name, "stopped", "{detail}");
    }
    let mut free = false;
    while !free && stopped.elapsed() < Duration::from_secs(1) {
        free = desk.server.structured("status").await["lease"]["holder"].is_null();
    }
    assert!(
        free,
        "the lease was still held {:?} after the stop",
        stopped.elapsed()
    );
    std::fs::write(desk.fixture.path("go"), "").unwrap();
    let (refused, _) = tool_error(&desk.server.call("acquire_desktop", json!({})).await);
    assert_eq!(refused, "stopped");
}

/// A `ref_invalid`'s name and its reason, the detail's first word.
fn reason(result: &Value) -> (&'static str, String) {
    let (name, detail) = tool_error(result);
    assert_eq!(name, "ref_invalid", "{detail}");
    let reason = detail.split(':').next().unwrap().to_owned();
    ("ref_invalid", reason)
}

/// Takes a screenshot under the lease and returns its ref.
async fn screenshot_ref(desk: &mut Desk) -> String {
    let shot = desk
        .server
        .call("screenshot", json!({"target": "focused_output"}))
        .await;
    shot["structuredContent"]["screenshot_ref"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[tokio::test]
async fn input_to_a_denied_app_is_refused() {
    let mut desk = Desk::start("act-denied", r#"deny_input_app_ids = ["a"]"#).await;
    let id = screenshot_ref(&mut desk).await;
    let click = desk
        .server
        .call("click", json!({"screenshot_ref": id, "x": 10, "y": 10}))
        .await;
    let (name, detail) = tool_error(&click);
    assert_eq!(name, "app_denied");
    assert!(detail.contains("\"a\""), "{detail}");
}

#[tokio::test]
async fn denial_is_focus_based_and_unchecked_expect_does_not_bypass_it() {
    let mut desk = Desk::start("deny-focus-only", r#"deny_input_app_ids = ["b"]"#).await;
    let id = screenshot_ref(&mut desk).await;
    // b is visible, but a is focused. Policy does not hit-test these coordinates.
    // With no fake Wayland server the call reaches connect, not an app_denied refusal.
    desk.unplug_display();
    let click = desk
        .server
        .call("click", json!({"screenshot_ref": id, "x": 10, "y": 10}))
        .await;
    let (name, detail) = tool_error(&click);
    assert_eq!(name, "upstream_error");
    assert!(detail.starts_with("connect to"), "{detail}");
    let _display = desk.replug_display();
    fake_wtype(&desk.fixture, "cat >/dev/null");
    let allowed = desk
        .server
        .call("type_text", json!({"text": "x", "expect": "none"}))
        .await;
    assert_eq!(allowed["isError"], false);
    let focused = desk
        .act("focus_window", json!({"id": 2}), |stream, _| {
            focus_changed(stream, 2);
        })
        .await;
    assert_eq!(focused["isError"], false);
    let denied = desk
        .server
        .call("type_text", json!({"text": "x", "expect": "none"}))
        .await;
    assert_eq!(tool_error(&denied).0, "app_denied");
}

#[tokio::test]
async fn pointer_tools_check_the_ref_the_outputs_and_their_arguments_first() {
    let mut desk = Desk::start("act-pointer", "").await;
    let at = |id: &str, x: u32| json!({"screenshot_ref": id, "x": x, "y": 10});
    let unknown = desk.server.call("pointer_move", at("shot-9", 10)).await;
    assert_eq!(reason(&unknown), ("ref_invalid", "unknown_ref".to_owned()));

    let id = screenshot_ref(&mut desk).await;
    let outside = desk.server.call("pointer_move", at(&id, 1280)).await;
    assert_eq!(
        reason(&outside),
        ("ref_invalid", "out_of_bounds".to_owned())
    );

    let mut quadruple = at(&id, 10);
    quadruple["count"] = json!(4);
    let mistaken = desk.server.call("click", quadruple).await;
    assert!(mistake(&mistaken).contains("`count`"));

    // The fixture's Wayland display has no socket: everything checked, nothing sent.
    desk.unplug_display();
    let click = desk.server.call("click", at(&id, 10)).await;
    let (name, detail) = tool_error(&click);
    assert_eq!(name, "upstream_error");
    assert!(detail.starts_with("connect to"), "{detail}");
    let marker = desk
        .fixture
        .path("run/niri-computer-use/niri.test/input-dirty");
    assert!(!marker.exists());

    let scaled = output("DP-1", Some((0, 0, 2560, 1440, 2.0)));
    desk.niri.set_outputs(vec![scaled.clone()], Some("DP-1"));
    let changed = desk.server.call("pointer_move", at(&id, 10)).await;
    assert_eq!(
        reason(&changed),
        ("ref_invalid", "output_changed".to_owned())
    );

    let clicked = |count: u8| {
        let mut args = at(&id, 10);
        args["button"] = json!("left");
        args["count"] = json!(count);
        args
    };
    let second = output("HDMI-A-1", Some((2560, 0, 1920, 1080, 1.0)));
    desk.niri.set_outputs(vec![scaled, second], Some("DP-1"));
    let two = desk.server.call("pointer_move", at(&id, 10)).await;
    assert_eq!(tool_error(&two).0, "untested_output_config");
    assert_eq!(
        desk.audited(),
        [
            json!(["pointer_move", at("shot-9", 10), null, null, "ref_invalid"]),
            json!(["pointer_move", at(&id, 1280), null, null, "ref_invalid"]),
            json!(["click", clicked(4), null, null, "invalid_arguments"]),
            json!(["click", clicked(1), null, null, "upstream_error"]),
            json!(["pointer_move", at(&id, 10), null, null, "ref_invalid"]),
            json!([
                "pointer_move",
                at(&id, 10),
                null,
                null,
                "untested_output_config"
            ]),
        ]
    );
}

#[tokio::test]
async fn scroll_refuses_notches_past_the_cap_at_the_signed_extremes() {
    let mut desk = Desk::start("act-notches", "").await;
    let id = screenshot_ref(&mut desk).await;
    for (x, y) in [(i32::MIN, 0), (0, i32::MIN), (i32::MAX, 0), (-11, 0)] {
        let arguments =
            json!({"screenshot_ref": id, "x": 10, "y": 10, "notches_x": x, "notches_y": y});
        let result = desk.server.call("scroll", arguments).await;
        assert!(mistake(&result).contains("notches"), "{x} {y}");
    }
}

#[tokio::test]
async fn a_point_is_a_pixel_or_an_element_ref_of_this_lease() {
    let mut desk = Desk::start("act-element", "").await;
    let id = screenshot_ref(&mut desk).await;
    for arguments in [
        json!({"screenshot_ref": id, "x": 10, "y": 10, "element": "elem-1"}),
        json!({"screenshot_ref": id, "x": 10}),
        json!({"screenshot_ref": id}),
    ] {
        let result = desk.server.call("click", arguments.clone()).await;
        assert!(mistake(&result).contains("`element`"), "{arguments}");
    }
    let half = json!({"screenshot_ref": id, "from": {"x": 1, "y": 1}, "to": {"y": 5}});
    assert!(mistake(&desk.server.call("drag", half).await).contains("`element`"));

    let unknown = json!({"screenshot_ref": id, "element": "elem-7"});
    let stale = desk.server.call("pointer_move", unknown.clone()).await;
    let (name, detail) = tool_error(&stale);
    assert_eq!(name, "element_stale");
    assert!(detail.contains("elements"), "{detail}");
    let marker = desk
        .fixture
        .path("run/niri-computer-use/niri.test/input-dirty");
    assert!(!marker.exists());
    let logged = desk.audited();
    assert_eq!(
        logged.last(),
        Some(&json!([
            "pointer_move",
            unknown,
            null,
            null,
            "element_stale"
        ]))
    );
}

#[tokio::test]
async fn keyboard_tools_check_their_text_focus_and_app_before_typing() {
    let mut desk = Desk::start("act-keys", r#"deny_input_app_ids = ["b"]"#).await;
    let long = desk
        .server
        .call(
            "type_text",
            json!({"text": "→".repeat(1001), "expect": "none"}),
        )
        .await;
    let (name, detail) = tool_error(&long);
    assert_eq!(name, "text_too_long");
    assert!(detail.starts_with("1001 characters"), "{detail}");

    let combo = desk
        .server
        .call("key", json!({"keys": ["hyper+a"], "expect": "none"}))
        .await;
    assert!(mistake(&combo).contains("unknown modifier"));

    let elsewhere = desk
        .server
        .call(
            "key",
            json!({"keys": ["ctrl+s"], "expect": {"window_id": 2}}),
        )
        .await;
    let (mismatch, seen) = tool_error(&elsewhere);
    assert_eq!(mismatch, "focus_mismatch");
    assert!(seen.contains("window 1"), "{seen}");

    focus_changed(&desk.stream, 2);
    let mut focused = Value::Null;
    for _ in 0..100 {
        focused = desk.server.structured("desktop_state").await["focused_window"].clone();
        if focused == 2 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(focused, 2);
    let denied = desk
        .server
        .call(
            "type_text",
            json!({"text": "secret", "expect": {"app_id": "b"}}),
        )
        .await;
    assert_eq!(tool_error(&denied).0, "app_denied");
    assert_eq!(
        desk.audited(),
        [
            json!(["type_text", {"text_len": 1001, "expect": "none"}, null, null, "text_too_long"]),
            json!(["key", {"keys": ["hyper+a"], "expect": "none"}, null, null, "invalid_arguments"]),
            json!(["key", {"keys": ["ctrl+s"], "expect": {"window_id": 2}}, null, null, "focus_mismatch"]),
            json!(["type_text", {"text_len": 6, "expect": {"app_id": "b"}}, null, null, "app_denied"]),
        ]
    );
}

#[tokio::test]
async fn paste_checks_its_text_focus_app_and_clipboard_before_the_key() {
    let mut desk = Desk::start("act-paste", r#"deny_input_app_ids = ["b"]"#).await;
    fake_wtype(&desk.fixture, "exit 0");
    let paste =
        |text: &str, expect: Value| json!({"text": text, "keys": "ctrl+v", "expect": expect});
    let long = desk
        .server
        .call("paste", paste(&"a".repeat(1024 * 1024 + 1), json!("none")))
        .await;
    assert_eq!(tool_error(&long).0, "text_too_long");
    let elsewhere = desk
        .server
        .call("paste", paste("x", json!({"window_id": 2})))
        .await;
    assert_eq!(tool_error(&elsewhere).0, "focus_mismatch");
    // Nothing serves the fixture's Wayland display, so the clipboard isn't touched.
    desk.unplug_display();
    let unserved = desk
        .server
        .call("paste", paste("pasted words", json!({"app_id": "a"})))
        .await;
    let (name, detail) = tool_error(&unserved);
    assert_eq!(name, "upstream_error");
    assert!(detail.starts_with("connect to"), "{detail}");
    assert!(!desk.fixture.path("wtype.args").exists());

    focus_changed(&desk.stream, 2);
    for _ in 0..100 {
        if desk.server.structured("desktop_state").await["focused_window"] == 2 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let denied = desk
        .server
        .call("paste", paste("x", json!({"app_id": "b"})))
        .await;
    assert_eq!(tool_error(&denied).0, "app_denied");
    let audit = std::fs::read_to_string(desk.fixture.audit_log()).unwrap();
    assert!(!audit.contains("pasted words"), "{audit}");
    assert_eq!(
        desk.audited(),
        [
            json!(["paste", {"text_len": 1024 * 1024 + 1, "keys": "ctrl+v", "expect": "none"}, null, null, "text_too_long"]),
            json!(["paste", {"text_len": 1, "keys": "ctrl+v", "expect": {"window_id": 2}}, null, null, "focus_mismatch"]),
            json!(["paste", {"text_len": 12, "keys": "ctrl+v", "expect": {"app_id": "a"}}, null, null, "upstream_error"]),
            json!(["paste", {"text_len": 1, "keys": "ctrl+v", "expect": {"app_id": "b"}}, null, null, "app_denied"]),
        ]
    );
}

#[tokio::test]
async fn held_pointer_keys_validate_before_any_input() {
    let mut desk = Desk::start_backend("held-invalid", "", "native").await;
    let id = screenshot_ref(&mut desk).await;
    let result = desk
        .server
        .call(
            "click",
            json!({"screenshot_ref": id, "x": 10, "y": 10, "keys": ["hyper"]}),
        )
        .await;
    assert!(mistake(&result).contains("unknown modifier"));
    assert!(
        !desk
            .fixture
            .path("run/niri-computer-use/niri.test/input-dirty")
            .exists()
    );
}

#[tokio::test]
async fn native_selection_never_falls_back_to_wtype() {
    let mut desk = Desk::start_backend("native-no-fallback", "", "native").await;
    desk.unplug_display();
    fake_wtype(&desk.fixture, "cat >/dev/null");
    let result = desk
        .server
        .call("type_text", json!({"text": "hi", "expect": "none"}))
        .await;
    let (name, detail) = tool_error(&result);
    assert_eq!(name, "upstream_error");
    assert!(detail.starts_with("connect to"), "{detail}");
    assert!(!desk.fixture.path("wtype.args").exists());
    assert!(
        !desk
            .fixture
            .path("run/niri-computer-use/niri.test/input-dirty")
            .exists()
    );
}

#[tokio::test]
async fn invalid_keyboard_selection_refuses_input() {
    let mut desk = Desk::start_backend("native-invalid", "", "typo").await;
    fake_wtype(&desk.fixture, "cat >/dev/null");
    let result = desk
        .server
        .call("type_text", json!({"text": "hi", "expect": "none"}))
        .await;
    let (name, detail) = tool_error(&result);
    assert_eq!(name, "refused");
    assert!(detail.contains("NIRI_COMPUTER_USE_KEYBOARD"), "{detail}");
    assert!(!desk.fixture.path("wtype.args").exists());
}

#[tokio::test]
async fn a_wtype_that_fails_to_start_leaves_no_marker() {
    let mut desk = Desk::start("act-no-wtype", "").await;
    let typed = desk
        .server
        .call(
            "type_text",
            json!({"text": "hi", "expect": {"app_id": "a"}}),
        )
        .await;
    let (name, detail) = tool_error(&typed);
    assert_eq!(name, "upstream_error");
    assert!(detail.starts_with("start wtype"), "{detail}");
    let marker = desk
        .fixture
        .path("run/niri-computer-use/niri.test/input-dirty");
    assert!(!marker.exists());
}

#[tokio::test]
async fn input_tools_refuse_on_a_locked_screen_before_anything_else() {
    let mut desk = Desk::start("act-locked", "").await;
    let id = screenshot_ref(&mut desk).await;
    desk.noctalia.set(LOCKED);
    for (tool, arguments) in [
        ("click", json!({"screenshot_ref": id, "x": 10, "y": 10})),
        (
            "scroll",
            json!({"screenshot_ref": id, "x": 10, "y": 10, "notches_y": 1}),
        ),
        ("key", json!({"keys": ["ctrl+s"], "expect": "none"})),
        ("type_text", json!({"text": "x", "expect": "none"})),
    ] {
        let refused = desk.server.call(tool, arguments).await;
        assert_eq!(tool_error(&refused).0, "screen_locked", "{tool}");
    }
    let marker = desk
        .fixture
        .path("run/niri-computer-use/niri.test/input-dirty");
    assert!(!marker.exists());
}

const MARKER: &str = "run/niri-computer-use/niri.test/input-dirty";

/// A fake wtype that records its arguments, its locale, all of stdin, and the marker as it
/// stands once stdin is closed, then runs `then`.
fn fake_wtype(fixture: &Fixture, then: &str) {
    fixture.program(
        "wtype",
        &format!(
            "printf '%s\\n' \"$@\" > \"$DIR/wtype.args\"\nprintf '%s' \"$LC_ALL\" > \"$DIR/wtype.locale\"\ncat > \"$DIR/wtype.in\"\ncat \"$DIR/{MARKER}\" > \"$DIR/wtype.marker\"\n{then}"
        ),
    );
}

#[tokio::test]
async fn type_text_feeds_wtype_behind_the_gate_and_clears_the_marker() {
    let mut desk = Desk::start("act-wtype", "").await;
    fake_wtype(&desk.fixture, "exit 0");
    let typed = desk
        .server
        .call(
            "type_text",
            json!({"text": "héllo → x", "expect": {"app_id": "a"}}),
        )
        .await;
    assert_eq!(
        outcome(&typed),
        json!({"accepted": true, "observed": "sent", "focused_window": 1, "focus": "matched"})
    );
    assert_eq!(desk.fixture.args("wtype"), ["-"]);
    let read = |name: &str| std::fs::read_to_string(desk.fixture.path(name)).unwrap();
    assert_eq!(read("wtype.in"), "héllo → x");
    assert_eq!(read("wtype.locale"), "C.UTF-8");
    // While wtype ran, the marker named it.
    let marker: Value = serde_json::from_str(&read("wtype.marker")).unwrap();
    assert_eq!(
        (&marker["operation"], &marker["phase"]),
        (&json!("type_text"), &json!("running"))
    );
    assert!(marker["child"]["pid"].as_u64().is_some(), "{marker}");
    assert!(!desk.fixture.path(MARKER).exists());
    // The audit log has the length, never the text.
    let audit = std::fs::read_to_string(desk.fixture.audit_log()).unwrap();
    assert!(!audit.contains("héllo"), "{audit}");
    assert_eq!(
        desk.audited(),
        [json!(["type_text", {"text_len": 9, "expect": {"app_id": "a"}}, true, "sent", null])]
    );

    let key = desk
        .server
        .call(
            "key",
            json!({"keys": ["ctrl+shift+t"], "expect": {"window_id": 1}}),
        )
        .await;
    assert_eq!(outcome(&key)["observed"], "sent");
    assert_eq!(
        desk.fixture.args("wtype"),
        [
            "-", "-M", "ctrl", "-M", "shift", "-k", "t", "-m", "shift", "-m", "ctrl"
        ]
    );
    assert_eq!(read("wtype.in"), "");
}

/// A fake wtype that appends stdin to `wtype.in` and its arguments, one line per call, to
/// `wtype.calls`, then runs `then`.
fn recording_wtype(fixture: &Fixture, then: &str) {
    fixture.program(
        "wtype",
        &format!("cat >> \"$DIR/wtype.in\"\necho \"$*\" >> \"$DIR/wtype.calls\"\n{then}"),
    );
}

/// The arguments of each `wtype` call so far.
fn wtype_calls(fixture: &Fixture) -> Vec<String> {
    std::fs::read_to_string(fixture.path("wtype.calls"))
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

/// A wtype whose first call waits until the test creates `go`, so the test can move focus
/// while it types.
const FIRST_CALL_WAITS: &str =
    "if [ ! -e \"$DIR/first\" ]; then : > \"$DIR/first\"; await_file go; fi";

/// Starts `tool`, moves focus to window 2 while wtype's first call runs, and returns the
/// result.
async fn focus_moved_during_the_first_call(desk: &mut Desk, tool: &str, arguments: Value) -> Value {
    recording_wtype(&desk.fixture, FIRST_CALL_WAITS);
    let id = desk.server.start_call(tool, arguments).await;
    while !desk.fixture.path("first").exists() {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    focus_changed(&desk.stream, 2);
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    std::fs::write(desk.fixture.path("go"), "").unwrap();
    desk.server.response(id).await["result"].clone()
}

#[tokio::test]
async fn long_text_is_typed_in_parts_of_a_hundred_characters() {
    let mut desk = Desk::start("act-wtype-parts", "").await;
    recording_wtype(&desk.fixture, "exit 0");
    let text = "é→x".repeat(83) + "y";
    let typed = desk
        .server
        .call(
            "type_text",
            json!({"text": text, "expect": {"app_id": "a"}}),
        )
        .await;
    assert_eq!(
        outcome(&typed),
        json!({"accepted": true, "observed": "sent", "focused_window": 1, "focus": "matched"})
    );
    let read = |name: &str| std::fs::read_to_string(desk.fixture.path(name)).unwrap();
    assert_eq!(read("wtype.in"), text);
    assert_eq!(wtype_calls(&desk.fixture), ["-", "-", "-"]);
    assert_eq!(
        desk.audited(),
        [json!(["type_text", {"text_len": 250, "expect": {"app_id": "a"}}, true, "sent", null])]
    );
}

#[tokio::test]
async fn focus_moving_during_a_part_stops_the_rest_of_the_text_and_its_return() {
    let mut desk = Desk::start("act-wtype-moved", "").await;
    let text = json!({"text": "a".repeat(250), "expect": {"window_id": 1}, "submit": true});
    let result = focus_moved_during_the_first_call(&mut desk, "type_text", text).await;
    assert_eq!(
        outcome(&result),
        json!({
            "accepted": true, "observed": "interrupted", "focused_window": 2, "focus": "matched",
            "typed": 100, "submitted": false
        })
    );
    let read = |name: &str| std::fs::read_to_string(desk.fixture.path(name)).unwrap();
    assert_eq!(read("wtype.in"), "a".repeat(100));
    assert_eq!(wtype_calls(&desk.fixture), ["-"]);
}

#[tokio::test]
async fn submit_presses_return_once_all_of_the_text_went_out() {
    let mut desk = Desk::start("act-submit", "").await;
    recording_wtype(&desk.fixture, "exit 0");
    let message = json!({"text": "a".repeat(150), "expect": {"window_id": 1}, "submit": true});
    let sent = desk.server.call("type_text", message).await;
    assert_eq!(
        outcome(&sent),
        json!({
            "accepted": true, "observed": "sent", "focused_window": 1, "focus": "matched",
            "submitted": true
        })
    );
    assert_eq!(wtype_calls(&desk.fixture), ["-", "-", "- -k Return"]);
    assert_eq!(
        desk.audited(),
        [
            json!(["type_text", {"text_len": 150, "expect": {"window_id": 1}, "submit": true}, true, "sent", null])
        ]
    );
}

#[tokio::test]
async fn key_presses_each_combination_in_order() {
    let mut desk = Desk::start("act-keys", "").await;
    recording_wtype(&desk.fixture, "exit 0");
    let keys = json!({"keys": ["ctrl+l", "Down", "Return"], "expect": {"app_id": "a"}});
    let pressed = desk.server.call("key", keys).await;
    assert_eq!(
        outcome(&pressed),
        json!({"accepted": true, "observed": "sent", "focused_window": 1, "focus": "matched"})
    );
    assert_eq!(
        wtype_calls(&desk.fixture),
        ["- -M ctrl -k l -m ctrl", "- -k Down", "- -k Return"]
    );
}

#[tokio::test]
async fn focus_moving_stops_the_rest_of_the_keys() {
    let mut desk = Desk::start("act-keys-moved", "").await;
    let keys = json!({"keys": ["Down", "Down", "Return"], "expect": {"window_id": 1}});
    let result = focus_moved_during_the_first_call(&mut desk, "key", keys).await;
    assert_eq!(
        outcome(&result),
        json!({
            "accepted": true, "observed": "interrupted", "focused_window": 2, "focus": "matched",
            "pressed": 1
        })
    );
    assert_eq!(wtype_calls(&desk.fixture), ["- -k Down"]);
}

#[tokio::test]
async fn a_screenshot_waits_for_the_running_action() {
    let mut desk = Desk::start("act-settled", "").await;
    recording_wtype(&desk.fixture, "await_file go; : > \"$DIR/wtype.done\"");
    desk.fixture.program(
        "grim",
        r#"if [ -e "$DIR/wtype.done" ]; then echo after; else echo during; fi > "$DIR/grim.when"; cat "$DIR/grim.out""#,
    );
    let typing = desk
        .server
        .start_call("type_text", json!({"text": "x", "expect": "none"}))
        .await;
    while !desk.fixture.path("wtype.calls").exists() {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let shot = desk
        .server
        .start_call("screenshot", json!({"target": "focused_output"}))
        .await;
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    std::fs::write(desk.fixture.path("go"), "").unwrap();
    desk.server.response(typing).await;
    desk.server.response(shot).await;
    let when = std::fs::read_to_string(desk.fixture.path("grim.when")).unwrap();
    assert_eq!(when.trim(), "after");
}

#[tokio::test]
async fn captures_exclude_the_next_action_until_completion_or_cancellation() {
    use std::time::Duration;

    for (tool, args) in [
        ("screenshot", json!({"target": "focused_output"})),
        (
            "wait_for",
            json!({"until": "screen_stable", "screenshot": true}),
        ),
        (
            "wait_for",
            json!({"until": {"window": {"app_id": "a"}}, "screenshot": true}),
        ),
    ] {
        let mut desk = Desk::start("shot-exclusive", "").await;
        desk.fixture.program(
            "grim",
            r#"
            : > "$DIR/grim.started"
            await_file grim.go
            cat "$DIR/grim.out"
        "#,
        );
        let capture = desk.server.start_call(tool, args).await;
        assert!(
            crate::fixture::eventually(Duration::from_secs(2), || desk
                .fixture
                .path("grim.started")
                .exists())
            .await
        );
        let action = desk
            .server
            .start_call("focus_window", json!({"id": 2}))
            .await;
        assert!(
            tokio::time::timeout(Duration::from_millis(200), desk.niri.action())
                .await
                .is_err(),
            "action ran inside {tool} capture"
        );
        std::fs::write(desk.fixture.path("grim.go"), "").unwrap();
        assert!(matches!(
            desk.niri.action().await,
            Action::FocusWindow { id: 2 }
        ));
        focus_changed(&desk.stream, 2);
        assert_eq!(
            desk.server.response(action).await["result"]["isError"],
            false
        );
        assert_eq!(
            desk.server.response(capture).await["result"]["isError"],
            false
        );
    }

    let mut desk = Desk::start("shot-cancel", "").await;
    desk.fixture
        .program("grim", r#": > "$DIR/grim.started"; exec sleep 30"#);
    let capture = desk
        .server
        .start_call("screenshot", json!({"target": "focused_output"}))
        .await;
    assert!(
        crate::fixture::eventually(Duration::from_secs(2), || desk
            .fixture
            .path("grim.started")
            .exists())
        .await
    );
    desk.server.cancel(capture).await;
    let result = desk
        .act("focus_window", json!({"id": 2}), |stream, _| {
            focus_changed(stream, 2);
        })
        .await;
    assert_eq!(result["isError"], false);
}

#[tokio::test]
async fn unsettled_read_only_waits_allow_actions_and_release_between_captures() {
    use std::time::Duration;

    for until in [json!("screen_stable"), json!({"window": {"app_id": "a"}})] {
        let mut desk = Desk::start("wait-yields-seat", "").await;
        desk.fixture.program(
            "grim",
            r#"
            n=$(cat "$DIR/grim.n" 2>/dev/null)x
            printf %s "$n" > "$DIR/grim.n"
            cat "$DIR/grim.out"
            printf %s "$n"
        "#,
        );
        let waiting = desk
            .server
            .start_call(
                "wait_for",
                json!({"until": until, "timeout_ms": 5000, "screenshot": true}),
            )
            .await;
        assert!(
            crate::fixture::eventually(Duration::from_secs(2), || desk
                .fixture
                .path("grim.n")
                .exists())
            .await
        );
        let action = desk
            .server
            .start_call("focus_window", json!({"id": 2}))
            .await;
        let sent = tokio::time::timeout(Duration::from_millis(500), desk.niri.action())
            .await
            .expect("visual settlement blocked the action for more than one capture");
        assert!(matches!(sent, Action::FocusWindow { id: 2 }));
        focus_changed(&desk.stream, 2);
        let focused = desk.server.response(action).await;
        assert_eq!(focused["result"]["isError"], false);
        let released = tokio::time::timeout(
            Duration::from_millis(500),
            desk.server
                .call("release_desktop", json!({"restore_focus": false})),
        )
        .await
        .expect("visual settlement blocked release");
        assert_eq!(released["structuredContent"]["released"], true);
        assert!(
            !desk.server.answered().contains(&waiting),
            "the wait ended before action/release completed"
        );
        desk.server.cancel(waiting).await;
    }
}

#[tokio::test]
async fn a_wtype_that_fails_by_itself_clears_the_marker_and_one_killed_leaves_it() {
    let mut desk = Desk::start("act-wtype-fail", "").await;
    let type_x = json!({"text": "x", "expect": "none"});
    fake_wtype(&desk.fixture, "echo 'unknown key' >&2; exit 1");
    let failed = desk.server.call("type_text", type_x.clone()).await;
    let (name, detail) = tool_error(&failed);
    assert_eq!(name, "upstream_error");
    assert!(detail.contains("unknown key"), "{detail}");
    assert!(!desk.fixture.path(MARKER).exists());

    fake_wtype(&desk.fixture, "kill -KILL $$");
    let killed = desk.server.call("type_text", type_x.clone()).await;
    let (killed_name, killed_detail) = tool_error(&killed);
    assert_eq!(killed_name, "upstream_error");
    assert!(killed_detail.contains("marker stays"), "{killed_detail}");
    assert!(desk.fixture.path(MARKER).exists());
    let blocked = desk.server.call("type_text", type_x).await;
    assert_eq!(tool_error(&blocked).0, "recovery_required");
}

#[tokio::test]
async fn a_wtype_past_its_deadline_is_killed_and_leaves_the_marker() {
    let mut desk = Desk::start("act-wtype-slow", "").await;
    fake_wtype(&desk.fixture, "exec sleep 10");
    let slow = desk
        .server
        .call("type_text", json!({"text": "x", "expect": "none"}))
        .await;
    let (name, detail) = tool_error(&slow);
    assert_eq!(name, "deadline_exceeded");
    assert!(detail.contains("keys may be held"), "{detail}");
    assert!(desk.fixture.path(MARKER).exists());
}

#[tokio::test]
async fn shell_open_and_close_watch_noctalias_active_panel() {
    let mut desk = Desk::start("act-shell", "").await;
    let opened = desk
        .server
        .call("shell_open", json!({"panel": "control-center"}))
        .await;
    assert_eq!(
        outcome(&opened),
        json!({
            "accepted": true, "observed": "opened", "focused_window": 1,
            "shell": {"active_panel": "control-center"}
        })
    );
    // Already open: nothing is sent.
    let again = desk
        .server
        .call("shell_open", json!({"panel": "control-center"}))
        .await;
    assert_eq!(outcome(&again)["accepted"], false);
    let closed = desk
        .server
        .call("shell_close", json!({"panel": "control-center"}))
        .await;
    assert_eq!(
        outcome(&closed),
        json!({
            "accepted": true, "observed": "closed", "focused_window": 1,
            "shell": {"active_panel": null}
        })
    );
    assert_eq!(
        desk.noctalia.panel_commands(),
        ["panel-open control-center", "panel-close control-center"]
    );
    assert_eq!(
        desk.audited(),
        [
            json!(["shell_open", {"panel": "control-center"}, true, "opened", null]),
            json!(["shell_open", {"panel": "control-center"}, false, "opened", null]),
            json!(["shell_close", {"panel": "control-center"}, true, "closed", null]),
        ]
    );
}

#[tokio::test]
async fn panels_outside_the_allowlist_are_refused_without_asking_noctalia() {
    let mut desk = Desk::start("act-shell-refused", "").await;
    for panel in ["launcher", "session", "polkit", "clipboard", "nope"] {
        for tool in ["shell_open", "shell_close"] {
            let refused = desk.server.call(tool, json!({"panel": panel})).await;
            let (name, detail) = tool_error(&refused);
            assert_eq!(name, "panel_not_allowed", "{tool} {panel}");
            assert!(detail.contains(panel), "{detail}");
        }
    }
    assert_eq!(desk.noctalia.panel_commands(), Vec::<String>::new());
}

#[tokio::test]
async fn a_panel_that_never_opens_times_out_with_a_screenshot() {
    let mut desk = Desk::start("act-shell-timeout", "").await;
    desk.noctalia.panels_follow(false);
    let started = std::time::Instant::now();
    let stuck = desk
        .server
        .call("shell_open", json!({"panel": "wallpaper"}))
        .await;
    assert_eq!(
        outcome(&stuck),
        json!({
            "accepted": true, "observed": "timeout", "focused_window": 1,
            "shell": {"active_panel": null}
        })
    );
    assert!(started.elapsed() >= std::time::Duration::from_secs(2));
    assert_eq!(desk.noctalia.panel_commands(), ["panel-open wallpaper"]);
}

#[tokio::test]
async fn shell_tools_need_the_lease_and_an_unlocked_screen() {
    let mut desk = Desk::start("act-shell-gate", "").await;
    desk.noctalia.set(LOCKED);
    let locked = desk
        .server
        .call("shell_open", json!({"panel": "control-center"}))
        .await;
    assert_eq!(tool_error(&locked).0, "screen_locked");
    desk.noctalia.set(UNLOCKED);
    desk.server
        .structured_with("release_desktop", json!({"restore_focus": false}))
        .await;
    let unheld = desk
        .server
        .call("shell_close", json!({"panel": "control-center"}))
        .await;
    assert_eq!(tool_error(&unheld).0, "lease_required");
    assert_eq!(desk.noctalia.panel_commands(), Vec::<String>::new());
}

#[tokio::test]
async fn a_lost_panel_reply_is_uncertain_with_a_screenshot() {
    let mut desk = Desk::start("act-shell-lost", "").await;
    desk.noctalia.panel_reply("");
    let lost = desk
        .server
        .call("shell_open", json!({"panel": "control-center"}))
        .await;
    assert_eq!(
        outcome(&lost),
        json!({
            "accepted": null, "observed": "uncertain", "focused_window": 1,
            "detail": "Noctalia closed the connection without a reply"
        })
    );
}

#[tokio::test]
async fn only_one_panel_is_open_at_a_time() {
    let mut desk = Desk::start("act-shell-other", "").await;
    let open = |panel: &str| json!({"panel": panel});
    outcome(&desk.server.call("shell_open", open("wallpaper")).await);
    // Opening another panel replaces it.
    let replaced = desk.server.call("shell_open", open("tray-drawer")).await;
    assert_eq!(
        outcome(&replaced)["shell"],
        json!({"active_panel": "tray-drawer"})
    );
    // Closing a panel that another one replaced sends nothing.
    let closed = desk.server.call("shell_close", open("wallpaper")).await;
    assert_eq!(
        outcome(&closed),
        json!({
            "accepted": false, "observed": "closed", "focused_window": 1,
            "shell": {"active_panel": "tray-drawer"}
        })
    );
    assert_eq!(
        desk.noctalia.panel_commands(),
        ["panel-open wallpaper", "panel-open tray-drawer"]
    );
}

#[tokio::test]
async fn an_action_asked_for_a_screenshot_returns_the_screen_once_it_stopped_changing() {
    let mut desk = Desk::start("act-shot-after", "").await;
    let shown = |result: &Value| {
        let content = result["content"].as_array().unwrap();
        assert_eq!(content.len(), 2, "{result}");
        assert_eq!(content[1]["type"], "image", "{result}");
        assert_eq!(
            result["structuredContent"]["observed"], "focused",
            "{result}"
        );
        result["structuredContent"]["screenshot"]["settled"].clone()
    };
    let still = desk
        .act(
            "focus_window",
            json!({"id": 2, "screenshot": true}),
            |stream, _| {
                focus_changed(stream, 2);
            },
        )
        .await;
    assert_eq!(shown(&still), true);

    // A screen that changes between every two captures never settles; the wait ends.
    desk.fixture.program(
        "grim",
        r#"n=$(cat "$DIR/grim.n" 2>/dev/null)x; printf %s "$n" > "$DIR/grim.n"; cat "$DIR/grim.out"; printf %s "$n""#,
    );
    let started = std::time::Instant::now();
    let moving = desk
        .act(
            "focus_window",
            json!({"id": 1, "screenshot": true}),
            |stream, _| {
                focus_changed(stream, 1);
            },
        )
        .await;
    assert_eq!(shown(&moving), false);
    let waited = started.elapsed();
    assert!((1500..4000).contains(&waited.as_millis()), "{waited:?}");
}

#[tokio::test]
async fn release_desktop_can_give_focus_back_to_the_users_window() {
    let mut desk = Desk::start("act-restore", "").await;
    let release = |restore: bool| json!({ "restore_focus": restore });
    // Take the lease again once the desktop shows window 1 focused.
    desk.server
        .structured_with("release_desktop", release(false))
        .await;
    while desk.server.structured("desktop_state").await["focused_window"] != 1 {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let acquired = desk.server.structured("acquire_desktop").await;
    assert_eq!(acquired["users_window"], 1, "{acquired}");
    desk.act("focus_window", json!({"id": 3}), |stream, _| {
        focus_changed(stream, 3);
    })
    .await;
    let restored = desk
        .act("release_desktop", release(true), |stream, action| {
            assert!(
                matches!(action, Action::FocusWindow { id: 1 }),
                "{action:?}"
            );
            focus_changed(stream, 1);
        })
        .await;
    assert_eq!(
        restored["structuredContent"],
        json!({
            "users_window": 1, "released": true,
            "restored": {"accepted": true, "observed": "focused", "focused_window": 1, "windows": [1]}
        })
    );

    // The user's window closed meanwhile: nothing is sent.
    desk.server.structured("acquire_desktop").await;
    desk.stream.send(&json!({"WindowClosed": {"id": 1}}));
    focus_changed(&desk.stream, 2);
    while desk.server.structured("desktop_state").await["focused_window"] != 2 {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let gone = desk
        .server
        .structured_with("release_desktop", release(true))
        .await;
    assert_eq!(gone["restored"]["observed"], "closed", "{gone}");
    assert_eq!(gone["restored"]["accepted"], false, "{gone}");
    assert!(!desk.niri.sent_action());
}

#[tokio::test]
async fn a_client_that_goes_mid_action_frees_the_lease_at_once() {
    use std::time::{Duration, Instant};

    let mut desk = Desk::start("act-client-gone", "").await;
    desk.fixture.program(
        "grim",
        r#": > "$DIR/grim.started"; await_file grim.go; cat "$DIR/grim.out""#,
    );
    desk.server
        .start_call("focus_window", json!({"id": 2, "screenshot": true}))
        .await;
    assert!(matches!(
        desk.niri.action().await,
        Action::FocusWindow { id: 2 }
    ));
    focus_changed(&desk.stream, 2);
    // The action's work is done and its evidence waits on grim.
    assert!(
        crate::fixture::eventually(Duration::from_secs(2), || desk
            .fixture
            .path("grim.started")
            .exists())
        .await
    );
    let went = Instant::now();
    desk.server.stop().await;
    assert!(
        went.elapsed() < Duration::from_millis(500),
        "{:?}",
        went.elapsed()
    );
    let mut next = Server::start(&desk.fixture).await;
    // A new server opens its own event stream; the shared engine keeps its one.
    let stream = if shared_mode() {
        desk.stream
    } else {
        desk.niri.stream().await
    };
    stream.workspaces(2);
    stream.send(&json!({"WindowsChanged": {"windows": [window_on(2, Some("b"), 1, true)]}}));
    let taken = next.structured("acquire_desktop").await;
    assert_eq!(taken["holder"]["pid"], next.serving_pid().await);
}

#[tokio::test]
async fn a_client_that_goes_mid_key_leaves_no_marker_behind() {
    use std::time::Duration;

    let mut desk = Desk::start("act-client-gone-typing", "").await;
    recording_wtype(&desk.fixture, "await_file go");
    desk.server
        .start_call("key", json!({"keys": ["Down"], "expect": "none"}))
        .await;
    let marker = desk
        .fixture
        .path("run/niri-computer-use/niri.test/input-dirty");
    assert!(
        crate::fixture::eventually(Duration::from_secs(2), || desk
            .fixture
            .path("wtype.calls")
            .exists())
        .await
    );
    // The call is dropped with the client, but the server waits for wtype to finish. A
    // bridge exits at once, and its engine finishes the cleanup.
    let stopped = tokio::spawn(desk.server.stop());
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(stopped.is_finished(), shared_mode());
    assert!(marker.exists());
    std::fs::write(desk.fixture.path("go"), "").unwrap();
    let (status, _, stderr) = stopped.await.unwrap();
    assert!(status.success(), "{status}: {stderr}");
    if shared_mode() {
        assert!(crate::fixture::eventually(Duration::from_secs(2), || !marker.exists()).await);
    } else {
        assert!(!marker.exists());
    }
}

#[tokio::test]
async fn niri_action_reports_the_window_as_niri_shows_it_after() {
    let mut desk = Desk::start("act-niri-action", "").await;
    let fullscreen = json!({"action": {"FullscreenWindow": {"id": 2}}});
    let result = desk
        .act("niri_action", fullscreen.clone(), |stream, action| {
            assert!(
                matches!(action, Action::FullscreenWindow { id: Some(2) }),
                "{action:?}"
            );
            stream.send(&json!({"WindowLayoutsChanged": {"changes": [[2, {
                "pos_in_scrolling_layout": [1, 1], "tile_size": [2560.0, 1440.0],
                "window_size": [2560, 1440], "tile_pos_in_workspace_view": null,
                "window_offset_in_tile": [0.0, 0.0]
            }]]}}));
        })
        .await;
    assert_eq!(
        outcome(&result),
        json!({
            "accepted": true, "observed": "changed", "focused_window": 1, "windows": [2],
            "window": {
                "id": 2, "workspace_id": 1, "is_focused": false, "is_floating": false,
                "is_urgent": false, "window_size": [2560, 1440], "tile_size": [2560.0, 1440.0],
                "pos_in_scrolling_layout": [1, 1]
            }
        })
    );
    let unknown = desk
        .server
        .call(
            "niri_action",
            json!({"action": {"FullscreenWindow": {"id": 9}}}),
        )
        .await;
    assert_eq!(
        mistake(&unknown),
        "invalid arguments: no window with id 9; desktop_state lists them"
    );
    let garbled = desk
        .server
        .call("niri_action", json!({"action": {"Fullscreen": {}}}))
        .await;
    assert!(
        mistake(&garbled).starts_with(
            "invalid arguments: `action` isn't a niri action: unknown variant `Fullscreen`"
        ),
        "{garbled}"
    );
    assert!(!desk.niri.sent_action());
    assert_eq!(
        desk.audited()[0],
        json!(["niri_action", fullscreen, true, "changed", null])
    );
}

#[tokio::test]
async fn gated_niri_actions_need_unrestricted() {
    let spawn = json!({"action": {"Spawn": {"command": ["foot"]}}});
    let mut desk = Desk::start("act-gated", "").await;
    let (name, detail) = tool_error(&desk.server.call("niri_action", spawn.clone()).await);
    assert_eq!(name, "unrestricted_required");
    assert!(
        detail.starts_with("Spawn needs unrestricted = true"),
        "{detail}"
    );
    assert!(!desk.niri.sent_action());
    assert_eq!(
        desk.audited(),
        [json!([
            "niri_action",
            spawn,
            null,
            null,
            "unrestricted_required"
        ])]
    );

    let mut open = Desk::start("act-unrestricted", "unrestricted = true").await;
    let result = open
        .act("niri_action", spawn, |_, action| {
            assert!(
                matches!(&action, Action::Spawn { command } if command == &["foot"]),
                "{action:?}"
            );
        })
        .await;
    assert_eq!(
        outcome(&result),
        json!({"accepted": true, "observed": "sent", "focused_window": 1})
    );
}

#[tokio::test]
async fn the_variable_turns_unrestricted_on_for_one_client() {
    const SHOT: &str = r#"
[[preset]]
name = "shot"
argv = ["kitty", "--class", "shot"]
app_id = "shot"
env = { GDK_SCALE = "2" }
"#;
    let on = [("NIRI_COMPUTER_USE_UNRESTRICTED", "1")];
    let mut desk = Desk::start_with("act-unrestricted-env", SHOT, &on).await;
    let status = desk.server.structured("status").await;
    assert_eq!(
        status["unrestricted"],
        json!({"enabled": true, "source": "env", "error": null})
    );
    assert_eq!(status["policy"]["state"], "loaded");
    let id = desk
        .server
        .start_call("launch", json!({"preset": "shot"}))
        .await;
    let action = desk.niri.action().await;
    assert!(
        matches!(&action, Action::Spawn { command }
            if command == &["env", "--", "GDK_SCALE=2", "kitty", "--class", "shot"]),
        "{action:?}"
    );
    desk.server.response(id).await;
    let spawn = json!({"action": {"Spawn": {"command": ["foot"]}}});
    desk.act("niri_action", spawn.clone(), |_, _| {}).await;

    // Any other value is reported and leaves it off, and the env preset invalid.
    let wrong = [("NIRI_COMPUTER_USE_UNRESTRICTED", "yes")];
    let mut off = Desk::start_with("act-unrestricted-wrong", "", &wrong).await;
    let reported = off.server.structured("status").await;
    assert_eq!(reported["unrestricted"]["enabled"], false);
    assert!(
        reported["unrestricted"]["error"]
            .as_str()
            .unwrap()
            .starts_with("NIRI_COMPUTER_USE_UNRESTRICTED is \"yes\""),
        "{reported}"
    );
    let (name, _) = tool_error(&off.server.call("niri_action", spawn).await);
    assert_eq!(name, "unrestricted_required");
}

#[tokio::test]
async fn noctalia_sends_any_command_only_when_unrestricted() {
    let mut desk = Desk::start("act-noctalia-off", "").await;
    let tools = desk.server.tools().await;
    assert!(!tools.iter().any(|tool| tool["name"] == "noctalia"));

    let on = [("NIRI_COMPUTER_USE_UNRESTRICTED", "1")];
    let mut open = Desk::start_with("act-noctalia", "", &on).await;
    let listed = open.server.tools().await;
    assert!(listed.iter().any(|tool| tool["name"] == "noctalia"));
    let args = json!({"args": ["panel-open", "wallpaper"]});
    let result = open.server.call("noctalia", args.clone()).await;
    assert_eq!(
        outcome(&result),
        json!({
            "accepted": true, "observed": "sent", "focused_window": 1,
            "noctalia": {"reply": "ok\n", "truncated": false}
        })
    );
    assert_eq!(open.noctalia.panel_commands(), ["panel-open wallpaper"]);
    let (name, detail) = tool_error(
        &open
            .server
            .call("noctalia", json!({"args": ["plugin", "x:y", "all", "go"]}))
            .await,
    );
    assert_eq!(name, "upstream_error");
    assert_eq!(
        detail,
        "Noctalia replied: error: unknown command \"plugin x:y all go\""
    );
    assert_eq!(
        open.audited()[0],
        json!(["noctalia", args, true, "sent", null])
    );
}
