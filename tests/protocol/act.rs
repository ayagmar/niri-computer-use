//! The action tools over stdio: the gate before each action, what niri is asked to do, and
//! the outcome read from the events the test sends back, as niri would after the action.

use niri_ipc::{Action, WorkspaceReferenceArg};
use serde_json::{Value, json};

use crate::client::{Server, mistake, run, tool_error};
use crate::fixture::Fixture;
use crate::niri::{Niri, Stream, window_on};
use crate::noctalia::{self, LOCKED, UNLOCKED};

/// A server holding the lease on a fake niri, with windows 1 (focused, `a`), 2 (`b`) and
/// 3 (`c`) on workspace 1 of two, and Noctalia saying the screen is unlocked.
struct Desk {
    fixture: Fixture,
    niri: Niri,
    stream: Stream,
    server: Server,
    noctalia: noctalia::Reply,
}

impl Desk {
    async fn start(name: &str, policy: &str) -> Self {
        let fixture = Fixture::new(name);
        std::fs::create_dir_all(fixture.path("config/niri-computer-use")).unwrap();
        std::fs::write(fixture.path("config/niri-computer-use/policy.toml"), policy).unwrap();
        fixture.program("noctalia", "exit 0");
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
            fixture,
            niri,
            stream,
            server,
            noctalia,
        }
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
            .filter(|line| !["status", "acquire_desktop"].contains(&line["tool"].as_str().unwrap()))
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

fn outcome(result: &Value) -> Value {
    assert_eq!(result["isError"], false, "{result}");
    result["structuredContent"].clone()
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
    desk.server.structured("release_desktop").await;
    let (unheld, detail) = tool_error(&desk.server.call("focus_window", json!({"id": 2})).await);
    assert_eq!(unheld, "lease_required");
    assert!(detail.contains("acquire_desktop"), "{detail}");
    assert!(!desk.niri.sent_action());
    assert_eq!(
        desk.audited(),
        [
            json!(["focus_window", {"id": 2}, null, null, "screen_locked"]),
            json!(["release_desktop", null, null, null, null]),
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
async fn focus_without_its_event_times_out() {
    let mut desk = Desk::start("act-timeout", "").await;
    let result = desk.act("focus_window", json!({"id": 2}), |_, _| {}).await;
    assert_eq!(
        outcome(&result),
        json!({"accepted": true, "observed": "timeout", "focused_window": 1})
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
            stream.send(&json!({"WindowFocusChanged": {"id": null}}));
            stream.send(&json!({"WorkspaceActivated": {"id": 2, "focused": true}}));
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
        // Not yet: it focused window 7, which already had focus.
        desk.niri.action().await;
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
