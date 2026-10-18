//! The action tools over stdio: the gate before each action, what niri is asked to do, and
//! the outcome read from the events the test sends back, as niri would after the action.

use std::time::{Duration, Instant};

use niri_ipc::{Action, WorkspaceReferenceArg};
use serde_json::{Value, json};

use crate::client::{Server, mistake, report, run, tool_error};
use crate::fixture::{DISPLAY, Fixture, jpeg, shared_mode};
use crate::niri::{Niri, Stream, output, window_on};
use crate::noctalia::{self, LOCKED, UNLOCKED};

/// How long after niri's initial state the server counts the windows as possibly moving.
const SETTLE: Duration = Duration::from_secs(1);

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

    /// As `start`, with these variables in the server's environment.
    async fn start_with(name: &str, policy: &str, vars: &[(&'static str, &str)]) -> Self {
        let mut desk = Self::starting(name, policy, vars).await;
        // A screenshot taken before then can't be aimed at by pixel.
        desk.server.structured("desktop_state").await;
        tokio::time::sleep(SETTLE).await;
        desk
    }

    /// As `start_with`, without waiting out the server's first second.
    async fn starting(name: &str, policy: &str, vars: &[(&'static str, &str)]) -> Self {
        let mut fixture = Fixture::new(name);
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
    let mut outcome = report(result);
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
    let (name, _) = tool_error(&desk.server.call("focus", json!({"window_id": 2})).await);
    assert_eq!(name, "screen_locked");
    desk.noctalia.set(UNLOCKED);
    desk.server
        .structured_with("release_desktop", json!({"restore_focus": false}))
        .await;
    let (unheld, detail) = tool_error(&desk.server.call("focus", json!({"window_id": 2})).await);
    assert_eq!(unheld, "lease_required");
    assert!(detail.contains("acquire_desktop"), "{detail}");
    assert!(!desk.niri.sent_action());
    assert_eq!(
        desk.audited(),
        [
            json!(["focus", {"window_id": 2}, null, null, "screen_locked"]),
            json!(["release_desktop", {"restore_focus": false}, null, null, null]),
            json!(["focus", {"window_id": 2}, null, null, "lease_required"]),
        ]
    );
}

#[tokio::test]
async fn focusing_a_window_is_accepted_then_observed() {
    let mut desk = Desk::start("act-focus", "").await;
    let result = desk
        .act("focus", json!({"window_id": 2}), |stream, action| {
            assert!(
                matches!(action, Action::FocusWindow { id: 2 }),
                "{action:?}"
            );
            focus_changed(stream, 2);
        })
        .await;
    assert_eq!(
        outcome(&result),
        json!({
            "accepted": true, "observed": "focused", "focused_window": 2,
            "changes": {"focus": {"from": 1, "to": 2}}
        })
    );
    let unknown = mistake(&desk.server.call("focus", json!({"window_id": 9})).await);
    assert_eq!(
        unknown,
        "invalid arguments: no window with id 9; desktop_state lists them"
    );
    assert!(!desk.niri.sent_action());
    assert_eq!(
        desk.audited(),
        [
            json!(["focus", {"window_id": 2}, true, "focused", null]),
            json!(["focus", {"window_id": 9}, null, null, "invalid_arguments"]),
        ]
    );
}

#[tokio::test]
async fn a_target_that_already_has_focus_sends_nothing() {
    let mut desk = Desk::start("act-focused", "").await;
    let window = desk.server.call("focus", json!({"window_id": 1})).await;
    assert_eq!(
        outcome(&window),
        json!({"accepted": false, "observed": "focused", "focused_window": 1})
    );
    // niri's `workspace-auto-back-and-forth` would switch away from it.
    let workspace = desk.server.call("focus", json!({"workspace_id": 1})).await;
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
        .act("focus", json!({"window_id": 2}), |stream, _| {
            focus_changed(stream, 3);
            focus_changed(stream, 2);
        })
        .await;
    // The wait ended at the intruder; the result goes on to where focus came to rest.
    assert_eq!(
        outcome(&result),
        json!({
            "accepted": true, "observed": "interrupted", "focused_window": 2,
            "changes": {"focus": {"from": 1, "to": 2}}
        })
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
        .act("focus", json!({"workspace_id": 2}), |stream, _| {
            focus_changed(stream, 4);
            focus_changed(stream, 3);
        })
        .await;
    assert_eq!(
        outcome(&result),
        json!({
            "accepted": true, "observed": "interrupted", "focused_window": 3,
            "changes": {"focus": {"from": 1, "to": 3}}
        })
    );
    let launch = desk
        .act("launch", json!({"preset": "foot"}), |stream, _| {
            focus_changed(stream, 2);
        })
        .await;
    assert_eq!(
        outcome(&launch),
        json!({
            "accepted": true, "observed": "interrupted", "focused_window": 2,
            "changes": {"focus": {"from": 3, "to": 2}}
        })
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
        json!({
            "accepted": true, "observed": "focused", "focused_window": 5, "windows": [5],
            "changes": {"focus": {"from": 1, "to": 5}}
        })
    );
}

#[tokio::test]
async fn focus_without_its_event_times_out() {
    let mut desk = Desk::start("act-timeout", "").await;
    let result = desk.act("focus", json!({"window_id": 2}), |_, _| {}).await;
    assert_eq!(
        outcome(&result),
        json!({"accepted": true, "observed": "timeout", "focused_window": 1})
    );
}

#[tokio::test]
async fn screenshots_under_the_lease_are_refs_of_that_lease() {
    let mut desk = Desk::start("act-refs", "").await;
    let shot = json!({"target": "focused_output"});
    let screenshot_ref = |result: Value| report(&result)["screenshot_ref"].clone();
    let first = screenshot_ref(desk.server.call("screenshot", shot.clone()).await);
    // `shot-<the server's tag>-<number>`.
    let tag = first
        .as_str()
        .and_then(|id| id.strip_prefix("shot-")?.strip_suffix("-1"))
        .unwrap()
        .to_owned();
    let evidence = desk.act("focus", json!({"window_id": 2}), |_, _| {}).await;
    assert_eq!(
        report(&evidence)["screenshot"]["screenshot_ref"],
        format!("shot-{tag}-2")
    );
    desk.server
        .structured_with("release_desktop", json!({"restore_focus": false}))
        .await;
    // Without the lease, a screenshot's ref is only for zooming: taking the lease again
    // drops it, so the pointer can't aim with it.
    let unleased = desk.server.call("screenshot", shot.clone()).await;
    let view = format!("shot-{tag}-3");
    assert_eq!(screenshot_ref(unleased), view.as_str());
    desk.server.structured("acquire_desktop").await;
    let again = desk.server.call("screenshot", shot).await;
    assert_eq!(screenshot_ref(again), format!("shot-{tag}-4"));
    let at = json!({"screenshot_ref": view, "x": 10, "y": 10});
    let moved = desk.server.call("pointer_move", at).await;
    assert_eq!(reason(&moved), ("ref_invalid", "unknown_ref".to_owned()));
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
    let result = desk.act("focus", json!({"window_id": 2}), |_, _| {}).await;
    assert_eq!(result["isError"], false, "{result}");
    assert_eq!(result["content"].as_array().unwrap().len(), 1, "{result}");
    let outcome = &report(&result);
    assert_eq!(outcome["observed"], "timeout");
    assert_eq!(outcome["screenshot_error"]["error"], "upstream_error");
    let detail = outcome["screenshot_error"]["detail"].as_str().unwrap();
    assert!(detail.contains("compositor gone"), "{detail}");
    assert_eq!(
        desk.audited(),
        [json!(["focus", {"window_id": 2}, true, "timeout", null])]
    );
}

#[tokio::test]
async fn another_client_is_refused_at_once_while_the_owner_acts_and_niri_is_slow() {
    let mut desk = Desk::start("act-acquire-busy", "").await;
    let mut other = Server::start(&desk.fixture).await;
    desk.niri.hold_actions(true);
    let id = desk
        .server
        .start_call("focus", json!({"window_id": 2}))
        .await;
    desk.niri.action().await;
    // The readiness report asks niri, which now answers nothing until its deadline.
    desk.niri.set_silent(true);
    let asked = Instant::now();
    let (name, detail) = tool_error(&other.call("acquire_desktop", json!({})).await);
    let took = asked.elapsed();
    assert_eq!(name, "lease_held", "{detail}");
    assert!(took < Duration::from_secs(1), "refused after {took:?}");
    desk.niri.set_silent(false);
    desk.server.response(id).await;
}

#[tokio::test]
async fn a_lost_reply_is_uncertain() {
    let mut desk = Desk::start("act-lost", "").await;
    desk.niri.hold_actions(true);
    let result = desk.act("focus", json!({"window_id": 2}), |_, _| {}).await;
    let outcome = outcome(&result);
    assert_eq!(outcome["accepted"], Value::Null);
    assert_eq!(outcome["observed"], "uncertain");
    let detail = outcome["detail"].as_str().unwrap();
    assert!(detail.contains("no reply within 2s"), "{detail}");
    // niri's event stream still says where focus is when the change report ends.
    assert_eq!(outcome["focused_window"], 1);
}

#[tokio::test]
async fn focusing_a_workspace_waits_for_the_workspace() {
    let mut desk = Desk::start("act-workspace", "").await;
    let result = desk
        .act("focus", json!({"workspace_id": 2}), |stream, action| {
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
        json!({
            "accepted": true, "observed": "focused", "focused_window": null,
            "changes": {"focus": {"from": 1, "to": null}, "workspace": {"from": 1, "to": 2}}
        })
    );
    let unknown = mistake(&desk.server.call("focus", json!({"workspace_id": 7})).await);
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
        json!({
            "accepted": true, "observed": "closed", "focused_window": 1, "windows": [2],
            "changes": {"closed": [{"id": 2, "app_id": "b"}]}
        })
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
        json!({
            "accepted": true, "observed": "one", "focused_window": 7, "windows": [7],
            "changes": {
                "opened": [{"id": 7, "app_id": "foot", "workspace_id": 1, "title": "t"}],
                "focus": {"from": 1, "to": 7}
            }
        })
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
        json!({
            "accepted": true, "observed": "ambiguous", "focused_window": 7, "windows": [8, 9],
            "changes": {"opened": [
                {"id": 8, "app_id": "foot", "workspace_id": 1, "title": "t"},
                {"id": 9, "app_id": "foot", "workspace_id": 1, "title": "t"}
            ]}
        })
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
            json!(["launch", {"preset": {"category": "unknown_preset", "bytes": 2}, "reuse": false}, null, null, "unknown_preset"]),
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
        json!({
            "accepted": true, "observed": "none", "focused_window": 1,
            "changes": {"opened": [{"id": 7, "app_id": "other", "workspace_id": 1, "title": "t"}]}
        })
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
        json!({
            "accepted": true, "observed": "focused", "focused_window": 7, "windows": [7],
            "changes": {"focus": {"from": 1, "to": 7}}
        })
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

/// What takes the action away from the server while it runs.
#[derive(Debug, Clone, Copy)]
enum Cut {
    /// The user's `niri-computer-use stop`.
    Stop,
    /// The `lease` file replaced, as another server's cleanup could.
    Lease,
}

impl Cut {
    async fn make(self, desk: &Desk) {
        match self {
            Self::Stop => assert!(run(&desk.fixture, "stop").await.status.success()),
            Self::Lease => {
                let lease = desk.fixture.runtime_dir().join("lease");
                std::fs::remove_file(&lease).unwrap();
                std::fs::write(&lease, "").unwrap();
            }
        }
    }

    /// The error before the action went out, and what `detail` says after.
    const fn reported(self) -> (&'static str, &'static str) {
        match self {
            Self::Stop => ("stopped", "the user's stop flag"),
            Self::Lease => ("lease_required", "the lease file was removed or replaced"),
        }
    }
}

const CUTS: [Cut; 2] = [Cut::Stop, Cut::Lease];

#[tokio::test]
async fn a_stop_or_a_replaced_lease_before_the_action_goes_out_sends_nothing() {
    for cut in CUTS {
        let mut desk = Desk::start(&format!("act-cut-before-{cut:?}"), "").await;
        let report = desk.niri.hold_version();
        let id = desk
            .server
            .start_call("focus", json!({"window_id": 2}))
            .await;
        report.arrived().await;
        cut.make(&desk).await;
        report.release();
        let (name, detail) = tool_error(&desk.server.response(id).await["result"]);
        assert_eq!(name, cut.reported().0, "{cut:?}: {detail}");
        assert!(!desk.niri.sent_action(), "{cut:?}");
        assert_eq!(
            desk.audited(),
            [json!(["focus", {"window_id": 2}, null, null, cut.reported().0])]
        );
    }
}

/// Runs `focus`, makes `cut` once niri got the action, and once the server read
/// niri's answer when `answered`, and returns the outcome after checking it is `uncertain`
/// with the cut's reason, without a screenshot.
async fn cut_after_sending(desk: &mut Desk, cut: Cut, answered: bool) -> Value {
    let id = desk
        .server
        .start_call("focus", json!({"window_id": 2}))
        .await;
    desk.niri.action().await;
    if answered {
        desk.niri.answer_read().await;
    }
    cut.make(desk).await;
    let result = desk.server.response(id).await["result"].clone();
    assert_eq!(result["isError"], false, "{cut:?}: {result}");
    assert_eq!(result["content"].as_array().unwrap().len(), 1, "{result}");
    let outcome = report(&result);
    assert_eq!(outcome["observed"], "uncertain", "{outcome}");
    let detail = outcome["detail"].as_str().unwrap();
    assert!(detail.contains(cut.reported().1), "{detail}");
    outcome
}

#[tokio::test]
async fn a_stop_or_a_replaced_lease_before_niri_answers_is_uncertain() {
    for cut in CUTS {
        let mut desk = Desk::start(&format!("act-cut-unanswered-{cut:?}"), "").await;
        desk.niri.hold_actions(true);
        let outcome = cut_after_sending(&mut desk, cut, false).await;
        assert_eq!(outcome["accepted"], Value::Null, "{outcome}");
        assert_eq!(
            desk.audited(),
            [json!(["focus", {"window_id": 2}, null, "uncertain", null])]
        );
    }
}

#[tokio::test]
async fn a_replaced_lease_after_niri_accepted_is_uncertain_and_gives_the_lease_up() {
    let mut desk = Desk::start("act-cut-lease", "").await;
    let outcome = cut_after_sending(&mut desk, Cut::Lease, true).await;
    assert_eq!(outcome["accepted"], true, "{outcome}");
    assert_eq!(
        desk.audited(),
        [json!(["focus", {"window_id": 2}, true, "uncertain", null])]
    );
    assert_eq!(
        desk.server.structured("status").await["lease"]["held_by_me"],
        false
    );
    let (after, _) = tool_error(&desk.server.call("focus", json!({"window_id": 2})).await);
    assert_eq!(after, "lease_required");
    assert!(!desk.niri.sent_action());
}

#[tokio::test]
async fn a_stop_cancels_the_running_action_and_takes_the_lease_back() {
    let mut desk = Desk::start("act-stop", "").await;
    let outcome = cut_after_sending(&mut desk, Cut::Stop, true).await;
    assert_eq!(outcome["accepted"], true, "{outcome}");
    assert_eq!(
        desk.audited(),
        [json!(["focus", {"window_id": 2}, true, "uncertain", null])]
    );
    let mut released = false;
    for _ in 0..250 {
        if desk.server.structured("status").await["lease"]["held_by_me"] == false {
            released = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(released, "the stop didn't take the lease back");
    let (after, _) = tool_error(&desk.server.call("focus", json!({"window_id": 2})).await);
    assert_eq!(after, "stopped");
}

#[tokio::test]
async fn a_stop_ends_the_owners_queued_actions_and_frees_the_lease_within_a_second() {
    let mut desk = Desk::start("act-stop-queue", "").await;
    desk.niri.hold_actions(true);
    let focus = json!({"window_id": 2});
    let mut calls = Vec::new();
    for _ in 0..16 {
        calls.push(desk.server.start_call("focus", focus.clone()).await);
    }
    // The first runs, its action held by niri, and the other fifteen wait for it.
    desk.niri.action().await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let stopped = Instant::now();
    assert!(run(&desk.fixture, "stop").await.status.success());
    // The running action went out to niri, so it may have been carried out.
    let running = desk.server.response(calls[0]).await;
    let cut = &report(&running["result"]);
    assert_eq!(cut["observed"], "uncertain", "{running}");
    assert_eq!(cut["accepted"], Value::Null, "{running}");
    assert!(
        cut["detail"]
            .as_str()
            .unwrap()
            .contains("the user's stop flag")
    );
    for &id in &calls[1..] {
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
    let (refused, _) = tool_error(&desk.server.call("acquire_desktop", json!({})).await);
    assert_eq!(refused, "stopped");
    let resume = run(&desk.fixture, "resume").await;
    assert!(resume.status.success(), "{resume:?}");
    desk.server.structured("acquire_desktop").await;
    assert_eq!(
        desk.server.structured("status").await["lease"]["held_by_me"],
        true
    );
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
    report(&shot)["screenshot_ref"].as_str().unwrap().to_owned()
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
    // With no fake Wayland server each call reaches connect, not an app_denied refusal.
    desk.unplug_display();
    for (tool, arguments) in [
        ("click", json!({"screenshot_ref": id, "x": 10, "y": 10})),
        ("type_text", json!({"text": "x", "expect": "none"})),
    ] {
        let (name, detail) = tool_error(&desk.server.call(tool, arguments).await);
        assert_eq!(name, "upstream_error", "{tool}");
        assert!(detail.starts_with("connect to"), "{tool}: {detail}");
    }
    let focused = desk
        .act("focus", json!({"window_id": 2}), |stream, _| {
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
async fn closing_a_denied_apps_window_is_refused_named_or_focused() {
    let mut desk = Desk::start("act-deny-close", r#"deny_input_app_ids = ["a"]"#).await;
    for (tool, arguments) in [
        ("close_window", json!({"id": 1})),
        ("niri_action", json!({"action": {"CloseWindow": {"id": 1}}})),
        (
            "niri_action",
            json!({"action": {"CloseWindow": {"id": null}}}),
        ),
    ] {
        let (name, detail) = tool_error(&desk.server.call(tool, arguments).await);
        assert_eq!(name, "app_denied", "{tool}");
        assert_eq!(
            detail, r#"window 1's app_id "a" is on the policy's deny list"#,
            "{tool}"
        );
    }
    assert!(!desk.niri.sent_action());
    // Layout actions still reach a denied window, and other apps' windows still close.
    let floated = desk
        .act(
            "niri_action",
            json!({"action": {"ToggleWindowFloating": {"id": 1}}}),
            |_, _| {},
        )
        .await;
    assert_eq!(outcome(&floated)["observed"], "unchanged");
    let closed = desk
        .act("close_window", json!({"id": 2}), |stream, _| {
            stream.send(&json!({"WindowClosed": {"id": 2}}));
        })
        .await;
    assert_eq!(outcome(&closed)["observed"], "closed");
}

#[tokio::test]
async fn a_focused_close_names_the_window_it_checked() {
    let deny_b = r#"deny_input_app_ids = ["b"]"#;
    let mut desk = Desk::start("act-close-focused", deny_b).await;
    let close_focused = json!({"action": {"CloseWindow": {"id": null}}});
    let closed = desk
        .act("niri_action", close_focused.clone(), |stream, action| {
            assert!(
                matches!(action, Action::CloseWindow { id: Some(1) }),
                "{action:?}"
            );
            // Focus moving to the denied window now can't redirect the close.
            focus_changed(stream, 2);
            stream.send(&json!({"WindowClosed": {"id": 1}}));
        })
        .await;
    assert_eq!(outcome(&closed)["observed"], "closed");
    assert_eq!(outcome(&closed)["windows"], json!([1]));

    // With no window focused, nothing is sent.
    desk.stream
        .send(&json!({"WindowFocusChanged": {"id": null}}));
    while !desk.server.structured("desktop_state").await["focused_window"].is_null() {
        tokio::task::yield_now().await;
    }
    let unfocused = desk.server.call("niri_action", close_focused).await;
    assert_eq!(
        mistake(&unfocused),
        "invalid arguments: no window has focus; pass the id of the window to act on"
    );
    assert!(!desk.niri.sent_action());

    // unrestricted doesn't lift the deny list.
    let policy = format!("unrestricted = true\n{deny_b}");
    let mut open = Desk::start("act-close-unrestricted", &policy).await;
    for (tool, arguments) in [
        ("niri_action", json!({"action": {"CloseWindow": {"id": 2}}})),
        ("close_window", json!({"id": 2})),
    ] {
        let refused = open.server.call(tool, arguments).await;
        assert_eq!(tool_error(&refused).0, "app_denied", "{tool}");
    }
    assert!(!open.niri.sent_action());
}

/// Sends `event` and waits until `desktop_state` shows `applied` of it.
async fn applied(desk: &mut Desk, event: &Value, applied: impl Fn(&Value) -> bool + Sync) {
    desk.stream.send(event);
    while !applied(&desk.server.structured("desktop_state").await) {
        tokio::task::yield_now().await;
    }
}

/// The ref of a screenshot taken once the screen stopped changing, as an action's
/// `screenshot: true` returns it.
async fn settled_ref(desk: &mut Desk) -> String {
    let arguments = json!({"until": "screen_stable", "timeout_ms": 2000, "screenshot": true});
    let waited = report(&desk.server.call("wait_for", arguments).await);
    assert_eq!(waited["observed"], "met", "{waited}");
    waited["screenshot"]["screenshot_ref"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[tokio::test]
async fn a_pixel_aim_is_refused_once_the_windows_its_screenshot_shows_moved() {
    let mut desk = Desk::start("act-moved", "").await;
    let at = |id: &str| json!({"screenshot_ref": id, "x": 10, "y": 10});
    let before = screenshot_ref(&mut desk).await;
    // The user focuses another window beside the one shown: niri scrolls to it.
    let focused =
        json!({"WorkspaceActiveWindowChanged": {"workspace_id": 1, "active_window_id": 2}});
    applied(&mut desk, &focused, |state| {
        state["workspaces"][0]["active_window_id"] == 2
    })
    .await;
    let since = settled_ref(&mut desk).await;
    // With no Wayland socket, a click that passes every check fails to connect.
    desk.unplug_display();
    let refused = desk.server.call("click", at(&before)).await;
    assert_eq!(
        reason(&refused),
        ("ref_invalid", "layout_changed".to_owned())
    );
    let (_, why) = tool_error(&refused);
    assert!(
        why.contains("follows window 2 instead of no window"),
        "{why}"
    );

    // A new title moves nothing.
    let mut retitled = window_on(2, Some("b"), 1, false);
    retitled["title"] = json!("loaded");
    let retitle = json!({"WindowOpenedOrChanged": {"window": retitled}});
    applied(&mut desk, &retitle, |state| {
        state["windows"][1]["title"] == "loaded"
    })
    .await;
    let (name, unconnected) = tool_error(&desk.server.call("click", at(&since)).await);
    assert_eq!(name, "upstream_error", "{unconnected}");

    // The user resizes a window.
    let mut layout = window_on(1, Some("a"), 1, true)["layout"].clone();
    layout["tile_size"] = json!([400.0, 600.0]);
    let resized = json!({"WindowLayoutsChanged": {"changes": [[1, layout]]}});
    applied(&mut desk, &resized, |state| {
        state["windows"][0]["layout"]["tile_size"][0] == 400.0
    })
    .await;
    let scroll = json!({"screenshot_ref": since, "x": 10, "y": 10, "notches_y": 1});
    let (_, stale) = tool_error(&desk.server.call("scroll", scroll).await);
    let moved = "layout_changed: niri moved or resized window 1 since the screenshot";
    assert!(stale.starts_with(moved), "{stale}");
}

// The bug: the check compared the windows at the capture with those at the call, so a
// change undone in between, which scrolls niri's view and may leave it there, went unseen.
#[tokio::test]
async fn a_pixel_aim_is_refused_after_focus_moved_away_and_back() {
    let mut desk = Desk::start("act-moved-back", "").await;
    let before = screenshot_ref(&mut desk).await;
    for active in [json!(2), Value::Null] {
        let event = json!({"WorkspaceActiveWindowChanged": {"workspace_id": 1, "active_window_id": active}});
        applied(&mut desk, &event, |state| {
            state["workspaces"][0]["active_window_id"] == active
        })
        .await;
    }
    desk.unplug_display();
    let click = json!({"screenshot_ref": before, "x": 10, "y": 10});
    let (_, why) = tool_error(&desk.server.call("click", click).await);
    let moved = "layout_changed: the view now follows no window instead of window 2, as after a focus change since the screenshot";
    assert!(why.starts_with(moved), "{why}");
}

// The bug: niri reports where a window goes at once and then animates it there, so a
// screenshot taken meanwhile shows it on the way, while the windows niri reported at the
// capture and at the call were the same.
#[tokio::test]
async fn a_pixel_aim_is_refused_through_a_screenshot_taken_while_windows_may_move() {
    let mut desk = Desk::start("act-moving", "").await;
    let mut layout = window_on(1, Some("a"), 1, true)["layout"].clone();
    layout["tile_size"] = json!([400.0, 600.0]);
    let resized = json!({"WindowLayoutsChanged": {"changes": [[1, layout]]}});
    applied(&mut desk, &resized, |state| {
        state["windows"][0]["layout"]["tile_size"][0] == 400.0
    })
    .await;
    let moving = screenshot_ref(&mut desk).await;
    let settled = settled_ref(&mut desk).await;
    desk.unplug_display();
    let at = |id: &str| json!({"screenshot_ref": id, "x": 10, "y": 10});
    let (_, why) = tool_error(&desk.server.call("click", at(&moving)).await);
    assert!(
        why.starts_with("layout_changed: niri moved or resized window 1 ")
            && why.contains(
                " ms before the screenshot, so it may show windows niri was still moving"
            ),
        "{why}"
    );
    // Once the screen was seen not changing, the image shows where the window ended.
    let (name, unconnected) = tool_error(&desk.server.call("click", at(&settled)).await);
    assert_eq!(name, "upstream_error", "{unconnected}");
}

// The bug: each connection's stamps started empty, so a screenshot taken right after a
// reconnect, while a switch or a move that began while the stream was down may still have
// been animating, aimed as if nothing had moved.
#[tokio::test]
async fn a_pixel_aim_is_refused_through_a_screenshot_taken_right_after_a_reconnect() {
    let mut desk = Desk::start("act-reconnected", "").await;
    desk.stream.close();
    desk.stream = desk.niri.stream().await;
    desk.stream.workspaces(2);
    desk.stream.send(&json!({"WindowsChanged": {"windows": [
        window_on(1, Some("a"), 1, true),
        window_on(2, Some("b"), 1, false),
    ]}}));
    desk.stream
        .send(&json!({"OverviewOpenedOrClosed": {"is_open": false}}));
    let mut state = desk.server.structured("desktop_state").await;
    while state["windows"].as_array().map(Vec::len) != Some(2) {
        state = desk.server.structured("desktop_state").await;
    }
    let after = screenshot_ref(&mut desk).await;
    desk.unplug_display();
    let click = json!({"screenshot_ref": after, "x": 10, "y": 10});
    let (_, why) = tool_error(&desk.server.call("click", click).await);
    let moved = "layout_changed: niri's event stream reconnected, and niri may have moved windows meanwhile, ";
    assert!(
        why.starts_with(moved) && why.contains(" ms before the screenshot"),
        "{why}"
    );
}

// The bug: the first connection's stamps started empty, so a screenshot taken as the
// server started, while a switch or a move already under way may still have been
// animating, aimed as if nothing had moved.
#[tokio::test]
async fn a_pixel_aim_is_refused_through_a_screenshot_taken_as_the_server_starts() {
    let mut desk = Desk::starting("act-started", "", &[]).await;
    let early = screenshot_ref(&mut desk).await;
    let settled = settled_ref(&mut desk).await;
    desk.unplug_display();
    let at = |id: &str| json!({"screenshot_ref": id, "x": 10, "y": 10});
    let (_, why) = tool_error(&desk.server.call("click", at(&early)).await);
    let moved = "layout_changed: the server had just started reading niri's events, and niri may have been moving windows then, ";
    assert!(
        why.starts_with(moved) && why.contains(" ms before the screenshot"),
        "{why}"
    );
    // The refusal's advice, a screenshot that waited for the screen to settle, gets through.
    let (name, unconnected) = tool_error(&desk.server.call("click", at(&settled)).await);
    assert_eq!(name, "upstream_error", "{unconnected}");
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
            json!(["pointer_move", {"screenshot_ref": {"category": "invalid_ref", "bytes": 6}, "x": 10, "y": 10}, null, null, "ref_invalid"]),
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
            {"screenshot_ref": id, "element": {"category": "invalid_ref", "bytes": 6}},
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

    let leaked = desk
        .server
        .call(
            "type_text",
            json!({"text": concat!("Done.", "<", "/invoke>"), "expect": "none", "submit": true}),
        )
        .await;
    let (tag, refused) = tool_error(&leaked);
    assert_eq!(tag, "leaked_tag");
    assert!(refused.ends_with("nothing was typed"), "{refused}");

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
        tokio::time::sleep(Duration::from_millis(20)).await;
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
            json!(["type_text", {"text_len": 14, "expect": "none", "submit": true}, null, null, "leaked_tag"]),
            json!(["key", {"keys": {"category": "invalid_keys", "count": 1}, "expect": "none"}, null, null, "invalid_arguments"]),
            json!(["key", {"keys": ["ctrl+s"], "expect": {"window_id": 2}}, null, null, "focus_mismatch"]),
            json!(["type_text", {"text_len": 6, "expect": {"app_id_len": 1, "matched": null}}, null, null, "app_denied"]),
        ]
    );
}

#[tokio::test]
async fn a_paste_checks_its_text_focus_app_and_clipboard_before_the_key() {
    let mut desk = Desk::start("act-paste", r#"deny_input_app_ids = ["b"]"#).await;
    let paste = |text: &str, expect: Value| json!({"text": text, "via": "paste", "paste_keys": "ctrl+v", "expect": expect});
    let long = desk
        .server
        .call(
            "type_text",
            paste(&"a".repeat(1024 * 1024 + 1), json!("none")),
        )
        .await;
    assert_eq!(tool_error(&long).0, "text_too_long");
    let leaked = desk
        .server
        .call(
            "type_text",
            paste(concat!("a\nb\n", "<", "/parameter>\n"), json!("none")),
        )
        .await;
    let (tag, refused) = tool_error(&leaked);
    assert_eq!(tag, "leaked_tag");
    assert!(refused.ends_with("nothing was pasted"), "{refused}");
    let elsewhere = desk
        .server
        .call("type_text", paste("x", json!({"window_id": 2})))
        .await;
    assert_eq!(tool_error(&elsewhere).0, "focus_mismatch");
    // Nothing serves the fixture's Wayland display, so the clipboard isn't touched.
    desk.unplug_display();
    let unserved = desk
        .server
        .call("type_text", paste("pasted words", json!({"app_id": "a"})))
        .await;
    let (name, detail) = tool_error(&unserved);
    assert_eq!(name, "upstream_error");
    assert!(detail.starts_with("connect to"), "{detail}");

    focus_changed(&desk.stream, 2);
    for _ in 0..100 {
        if desk.server.structured("desktop_state").await["focused_window"] == 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let denied = desk
        .server
        .call("type_text", paste("x", json!({"app_id": "b"})))
        .await;
    assert_eq!(tool_error(&denied).0, "app_denied");
    let audit = std::fs::read_to_string(desk.fixture.audit_log()).unwrap();
    assert!(!audit.contains("pasted words"), "{audit}");
    assert!(!audit.contains("/parameter"), "{audit}");
    assert_eq!(
        desk.audited(),
        [
            json!(["type_text", {"text_len": 1024 * 1024 + 1, "via": "paste", "paste_keys": "ctrl+v", "expect": "none"}, null, null, "text_too_long"]),
            json!(["type_text", {"text_len": 17, "via": "paste", "paste_keys": "ctrl+v", "expect": "none"}, null, null, "leaked_tag"]),
            json!(["type_text", {"text_len": 1, "via": "paste", "paste_keys": "ctrl+v", "expect": {"window_id": 2}}, null, null, "focus_mismatch"]),
            json!(["type_text", {"text_len": 12, "via": "paste", "paste_keys": "ctrl+v", "expect": {"app_id_len": 1, "matched": null}}, null, null, "upstream_error"]),
            json!(["type_text", {"text_len": 1, "via": "paste", "paste_keys": "ctrl+v", "expect": {"app_id_len": 1, "matched": null}}, null, null, "app_denied"]),
        ]
    );
}

#[tokio::test]
async fn held_pointer_keys_validate_before_any_input() {
    let mut desk = Desk::start("held-invalid", "").await;
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

/// `NIRI_COMPUTER_USE_KEYBOARD` is ignored, whatever it says: the keys go to the native
/// keyboard, here a display nothing serves.
#[tokio::test]
async fn the_old_keyboard_variable_is_ignored() {
    for value in ["native", "typo"] {
        let mut desk = Desk::start_with(
            &format!("keyboard-ignored-{value}"),
            "",
            &[("NIRI_COMPUTER_USE_KEYBOARD", value)],
        )
        .await;
        desk.unplug_display();
        let result = desk
            .server
            .call("type_text", json!({"text": "hi", "expect": "none"}))
            .await;
        let (name, detail) = tool_error(&result);
        assert_eq!(name, "upstream_error", "{value}");
        assert!(detail.starts_with("connect to"), "{value}: {detail}");
        assert!(!desk.fixture.path(MARKER).exists(), "{value}");
    }
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

/// A control character would be pressed as a key: a line break or a tab as Return or Tab,
/// sending a message before the rest of it or moving to another field, and others as
/// `BackSpace`, `Escape` or `Delete`. The text is refused before anything is typed, with a
/// pointer to `submit`, `key` and `via: "paste"`.
#[tokio::test]
async fn text_with_a_control_character_is_refused_before_anything_is_typed() {
    let mut desk = Desk::start("act-newline", "").await;
    let texts = [
        "Hi,\n\nhere is the plan",
        "a\r\nb",
        "name\tvalue",
        "typo\u{8}",
        "dialog\u{1b}",
        "word\u{7f}",
    ];
    for text in texts {
        let message = json!({"text": text, "expect": {"window_id": 1}, "submit": true});
        let refused = desk.server.call("type_text", message).await;
        let said = mistake(&refused);
        assert!(said.contains("holds a control character"), "{said}");
        assert!(
            said.contains("`submit: true`") && said.contains("`via: \"paste\"`"),
            "{said}"
        );
    }
    assert!(!desk.fixture.path(MARKER).exists());
}

#[tokio::test]
async fn a_screenshot_waits_for_the_running_action() {
    let mut desk = Desk::start("act-settled", "").await;
    desk.fixture.program(
        "grim",
        r#"if [ -e "$DIR/answered" ]; then echo after; else echo during; fi > "$DIR/grim.when"; cat "$DIR/grim.out""#,
    );
    let answer = desk.niri.hold_focus();
    let focus = desk
        .server
        .start_call("focus", json!({"window_id": 2}))
        .await;
    answer.arrived().await;
    let shot = desk
        .server
        .start_call("screenshot", json!({"target": "focused_output"}))
        .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    std::fs::write(desk.fixture.path("answered"), "").unwrap();
    answer.release();
    focus_changed(&desk.stream, 2);
    desk.server.response(focus).await;
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
            .start_call("focus", json!({"window_id": 2}))
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
        .act("focus", json!({"window_id": 2}), |stream, _| {
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
            .start_call("focus", json!({"window_id": 2}))
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
        assert_eq!(report(&released)["released"], true);
        assert!(
            !desk.server.answered().contains(&waiting),
            "the wait ended before action/release completed"
        );
        desk.server.cancel(waiting).await;
    }
}

#[tokio::test]
async fn shell_panel_opens_and_closes_watching_noctalias_active_panel() {
    let mut desk = Desk::start("act-shell", "").await;
    let opened = desk
        .server
        .call(
            "shell_panel",
            json!({"panel": "control-center", "open": true}),
        )
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
        .call(
            "shell_panel",
            json!({"panel": "control-center", "open": true}),
        )
        .await;
    assert_eq!(outcome(&again)["accepted"], false);
    let closed = desk
        .server
        .call(
            "shell_panel",
            json!({"panel": "control-center", "open": false}),
        )
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
            json!(["shell_panel", {"panel": "control-center", "open": true}, true, "opened", null]),
            json!(["shell_panel", {"panel": "control-center", "open": true}, false, "opened", null]),
            json!(["shell_panel", {"panel": "control-center", "open": false}, true, "closed", null]),
        ]
    );
}

// niri's event stream reconnected but sent no state, so the change report can't start. The
// panel still opens, as Noctalia needs no stream, and the report says why it has nothing
// rather than reading as "nothing changed".
#[tokio::test]
async fn a_report_that_cannot_start_says_why() {
    let mut desk = Desk::start("act-shell-unwatched", "").await;
    desk.stream.close();
    let _silent = desk.niri.stream().await;
    let opened = desk
        .server
        .call(
            "shell_panel",
            json!({"panel": "control-center", "open": true}),
        )
        .await;
    assert_eq!(
        outcome(&opened),
        json!({
            "accepted": true, "observed": "opened", "focused_window": null,
            "shell": {"active_panel": "control-center"},
            "changes": {"lost": "niri's event stream sent no initial state within 2s"}
        })
    );
}

#[tokio::test]
async fn panels_outside_the_allowlist_are_refused_without_asking_noctalia() {
    let mut desk = Desk::start("act-shell-refused", "").await;
    for panel in ["launcher", "session", "polkit", "clipboard", "nope"] {
        for open in [true, false] {
            let args = json!({"panel": panel, "open": open});
            let refused = desk.server.call("shell_panel", args).await;
            let (name, detail) = tool_error(&refused);
            assert_eq!(name, "panel_not_allowed", "{open} {panel}");
            assert!(detail.contains(panel), "{detail}");
        }
    }
    assert_eq!(desk.noctalia.panel_commands(), Vec::<String>::new());
}

#[tokio::test]
async fn a_panel_that_never_opens_times_out_with_a_screenshot() {
    let mut desk = Desk::start("act-shell-timeout", "").await;
    desk.noctalia.panels_follow(false);
    let started = Instant::now();
    let stuck = desk
        .server
        .call("shell_panel", json!({"panel": "wallpaper", "open": true}))
        .await;
    assert_eq!(
        outcome(&stuck),
        json!({
            "accepted": true, "observed": "timeout", "focused_window": 1,
            "shell": {"active_panel": null}
        })
    );
    assert!(started.elapsed() >= Duration::from_secs(2));
    assert_eq!(desk.noctalia.panel_commands(), ["panel-open wallpaper"]);
}

#[tokio::test]
async fn shell_tools_need_the_lease_and_an_unlocked_screen() {
    let mut desk = Desk::start("act-shell-gate", "").await;
    desk.noctalia.set(LOCKED);
    let locked = desk
        .server
        .call(
            "shell_panel",
            json!({"panel": "control-center", "open": true}),
        )
        .await;
    assert_eq!(tool_error(&locked).0, "screen_locked");
    desk.noctalia.set(UNLOCKED);
    desk.server
        .structured_with("release_desktop", json!({"restore_focus": false}))
        .await;
    let unheld = desk
        .server
        .call(
            "shell_panel",
            json!({"panel": "control-center", "open": false}),
        )
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
        .call(
            "shell_panel",
            json!({"panel": "control-center", "open": true}),
        )
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
    let open = |panel: &str, open: bool| json!({"panel": panel, "open": open});
    outcome(
        &desk
            .server
            .call("shell_panel", open("wallpaper", true))
            .await,
    );
    // Opening another panel replaces it.
    let replaced = desk
        .server
        .call("shell_panel", open("tray-drawer", true))
        .await;
    assert_eq!(
        outcome(&replaced)["shell"],
        json!({"active_panel": "tray-drawer"})
    );
    // Closing a panel that another one replaced sends nothing.
    let closed = desk
        .server
        .call("shell_panel", open("wallpaper", false))
        .await;
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
        assert_eq!(report(result)["observed"], "focused", "{result}");
        report(result)["screenshot"]["settled"].clone()
    };
    let still = desk
        .act(
            "focus",
            json!({"window_id": 2, "screenshot": true}),
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
    let started = Instant::now();
    let moving = desk
        .act(
            "focus",
            json!({"window_id": 1, "screenshot": true}),
            |stream, _| {
                focus_changed(stream, 1);
            },
        )
        .await;
    assert_eq!(shown(&moving), false);
    let waited = started.elapsed();
    assert!((1500..4000).contains(&waited.as_millis()), "{waited:?}");
    // The audit log reads the outcome before the client's result loses its structure.
    assert_eq!(
        desk.audited(),
        [
            json!(["focus", {"window_id": 2, "screenshot": true}, true, "focused", null]),
            json!(["focus", {"window_id": 1, "screenshot": true}, true, "focused", null]),
        ]
    );
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
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let acquired = desk.server.structured("acquire_desktop").await;
    assert_eq!(acquired["users_window"], 1, "{acquired}");
    desk.act("focus", json!({"window_id": 3}), |stream, _| {
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
        report(&restored),
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
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let gone = desk
        .server
        .structured_with("release_desktop", release(true))
        .await;
    assert_eq!(gone["restored"]["observed"], "closed", "{gone}");
    assert_eq!(gone["restored"]["accepted"], false, "{gone}");
    assert!(!desk.niri.sent_action());
}

/// A user who pressed the stop key wants the desktop back at once: the stop took the lease
/// and forgot the user's window with it, so giving focus back is skipped without reading
/// anything, and nothing is sent.
#[tokio::test]
async fn after_a_stop_release_desktop_skips_the_restore_and_its_readiness_report() {
    let mut desk = Desk::start("act-restore-stopped", "").await;
    let release = |restore: bool| json!({ "restore_focus": restore });
    desk.server
        .structured_with("release_desktop", release(false))
        .await;
    while desk.server.structured("desktop_state").await["focused_window"] != 1 {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let acquired = desk.server.structured("acquire_desktop").await;
    assert_eq!(acquired["users_window"], 1, "{acquired}");
    assert!(run(&desk.fixture, "stop").await.status.success());
    while !desk.server.structured("status").await["lease"]["holder"].is_null() {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let read = desk.noctalia.status_requests();
    let released = desk
        .server
        .structured_with("release_desktop", release(true))
        .await;
    assert_eq!(
        released,
        json!({"users_window": null, "released": false}),
        "{released}"
    );
    assert_eq!(desk.noctalia.status_requests(), read);
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
        .start_call("focus", json!({"window_id": 2, "screenshot": true}))
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

/// An action that names no window goes out naming the one focused at the check, so focus
/// moving before niri handles it can't make it act on another window.
#[tokio::test]
async fn niri_action_names_the_window_focused_at_the_check() {
    let mut desk = Desk::start("act-niri-action-pinned", "").await;
    let fullscreen = json!({"action": {"FullscreenWindow": {"id": null}}});
    desk.act("niri_action", fullscreen, |stream, action| {
        assert!(
            matches!(action, Action::FullscreenWindow { id: Some(1) }),
            "{action:?}"
        );
        stream.send(&json!({"WindowFocusChanged": {"id": 2}}));
    })
    .await;
    let moved = json!({"action": {"MoveWindowToWorkspace": {
        "window_id": null, "reference": {"Index": 2}, "focus": false
    }}});
    desk.act("niri_action", moved, |_, action| {
        assert!(
            matches!(
                action,
                Action::MoveWindowToWorkspace {
                    window_id: Some(2),
                    ..
                }
            ),
            "{action:?}"
        );
    })
    .await;
}

#[tokio::test]
async fn focus_and_spawn_answer_well_before_an_action_that_waits_for_quiet() {
    let mut desk = Desk::start("act-quiet", "unrestricted = true").await;
    let started = Instant::now();
    desk.act("focus", json!({"window_id": 2}), |stream, _| {
        focus_changed(stream, 2);
    })
    .await;
    let focused = started.elapsed();
    // `Spawn`'s window comes later than the quiet wait would cover; `wait_for` finds it.
    let spawning = Instant::now();
    let spawn = json!({"action": {"Spawn": {"command": ["foot"]}}});
    desk.act("niri_action", spawn, |_, _| {}).await;
    let spawned = spawning.elapsed();

    // A workspace switch is only `sent`: its own wait saw nothing, so the report waits,
    // and what niri reports a moment after its reply is in it.
    let switching = Instant::now();
    let down = json!({"action": {"FocusWorkspaceDown": {}}});
    let id = desk.server.start_call("niri_action", down).await;
    desk.niri.action().await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    desk.stream
        .send(&json!({"WorkspaceActivated": {"id": 2, "focused": true}}));
    desk.stream
        .send(&json!({"WindowFocusChanged": {"id": null}}));
    let result = desk.server.response(id).await["result"].clone();
    let switched = switching.elapsed();
    assert_eq!(
        outcome(&result),
        json!({
            "accepted": true, "observed": "sent", "focused_window": null,
            "changes": {"focus": {"from": 2, "to": null}, "workspace": {"from": 1, "to": 2}}
        })
    );
    // The switch waited at least 100 ms and then the 250 ms quiet; waiting for quiet too,
    // `focus` and `Spawn` would each take at least 250 ms, over half of that.
    assert!(
        focused * 2 < switched && spawned * 2 < switched,
        "focus {focused:?}, Spawn {spawned:?}, the switch {switched:?}"
    );
}

#[tokio::test]
async fn focus_on_another_workspace_reports_the_switch_and_the_cleared_urgency() {
    let mut desk = Desk::start("act-focus-across", "").await;
    let mut urgent = window_on(4, Some("chat"), 2, false);
    urgent["is_urgent"] = json!(true);
    desk.stream
        .send(&json!({"WindowOpenedOrChanged": {"window": urgent}}));
    let has_it = |state: &Value| state["windows"].as_array().unwrap().len() == 4;
    while !has_it(&desk.server.structured("desktop_state").await) {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let id = desk
        .server
        .start_call("focus", json!({"window_id": 4}))
        .await;
    desk.niri.action().await;
    // niri's order within one refresh. The urgency comes a moment after the focus event,
    // after the focus has returned on it, as when the server reads between niri's writes.
    desk.stream
        .send(&json!({"WorkspaceActivated": {"id": 2, "focused": true}}));
    focus_changed(&desk.stream, 4);
    tokio::time::sleep(Duration::from_millis(2)).await;
    desk.stream
        .send(&json!({"WindowUrgencyChanged": {"id": 4, "urgent": false}}));
    let result = desk.server.response(id).await["result"].clone();
    assert_eq!(
        outcome(&result),
        json!({
            "accepted": true, "observed": "focused", "focused_window": 4,
            "changes": {
                "focus": {"from": 1, "to": 4},
                "workspace": {"from": 1, "to": 2},
                "windows": [{"id": 4, "urgent": false}]
            }
        })
    );
}

/// A floating window's move changes only its place, which niri 26.04 reports for floating
/// windows alone. It used to be `unchanged` after a second's wait.
#[tokio::test]
async fn niri_action_sees_a_floating_window_move() {
    let mut desk = Desk::start("act-niri-float", "").await;
    let mut floating = window_on(2, Some("b"), 1, false);
    floating["is_floating"] = json!(true);
    floating["layout"]["pos_in_scrolling_layout"] = Value::Null;
    floating["layout"]["tile_pos_in_workspace_view"] = json!([200.0, 50.0]);
    desk.stream
        .send(&json!({"WindowOpenedOrChanged": {"window": floating}}));
    while !desk.server.structured("desktop_state").await["windows"][1]["is_floating"]
        .as_bool()
        .unwrap()
    {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let left = json!({"action": {"MoveFloatingWindow": {
        "id": 2, "x": {"AdjustFixed": -100.0}, "y": {"AdjustFixed": 0.0}
    }}});
    let result = desk
        .act("niri_action", left, |stream, _| {
            stream.send(&json!({"WindowLayoutsChanged": {"changes": [[2, {
                "pos_in_scrolling_layout": null, "tile_size": [800.0, 600.0],
                "window_size": [800, 600], "tile_pos_in_workspace_view": [100.0, 50.0],
                "window_offset_in_tile": [0.0, 0.0]
            }]]}}));
        })
        .await;
    let outcome = outcome(&result);
    assert_eq!(outcome["observed"], "changed", "{outcome}");
    assert_eq!(
        outcome["window"]["tile_pos_in_workspace_view"],
        json!([100.0, 50.0])
    );
    assert_eq!(
        outcome["changes"],
        json!({"windows": [{"id": 2, "position": [100.0, 50.0]}]})
    );
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
                "pos_in_scrolling_layout": [1, 1], "tile_pos_in_workspace_view": null
            },
            "changes": {"windows": [{"id": 2, "size": [2560, 1440]}]}
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
            {"action": {"Spawn": {"command": {"count": 1, "bytes": [4]}}}},
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

/// Text an agent could pass on from the clipboard: lowercase words and `-`, the shape of
/// a command word.
const PRIVATE: &str = "synthetic-private-content";

#[tokio::test]
async fn passthrough_arguments_reach_the_audit_log_only_as_metadata() {
    let on = [("NIRI_COMPUTER_USE_UNRESTRICTED", "1")];
    let mut desk = Desk::start_with("act-audit-private", "", &on).await;
    let spawn = json!({"action": {"Spawn": {"command": ["foot", PRIVATE]}}});
    desk.act("niri_action", spawn, |_, _| {}).await;
    // The last breaks the message's framing and is refused.
    for args in [
        json!([PRIVATE, PRIVATE]),
        json!(["panel-open", PRIVATE]),
        json!([
            "panel-open",
            format!("{PRIVATE}\u{1e}panel-close wallpaper")
        ]),
    ] {
        desk.server.call("noctalia", json!({ "args": args })).await;
    }
    // Cancelled while it waits for the window to change.
    let moved = json!({"action": {"MoveWindowToWorkspace": {
        "window_id": 2, "reference": {"Name": PRIVATE}, "focus": false
    }}});
    let id = desk.server.start_call("niri_action", moved).await;
    desk.niri.action().await;
    desk.server.cancel(id).await;
    let cancelled = || {
        desk.fixture
            .audit_lines()
            .iter()
            .any(|line| line["error"] == "cancelled")
    };
    assert!(crate::fixture::eventually(Duration::from_secs(2), cancelled).await);
    for garbled in [json!({PRIVATE: {}}), json!({"Spawn": {"command": PRIVATE}})] {
        let result = desk
            .server
            .call("niri_action", json!({ "action": garbled }))
            .await;
        assert!(
            mistake(&result).starts_with("invalid arguments"),
            "{result}"
        );
    }
    // Refused without the lease.
    let released = json!({"restore_focus": false});
    desk.server.call("release_desktop", released).await;
    let named = json!({"action": {"SetWorkspaceName": {"name": PRIVATE, "workspace": null}}});
    desk.server.call("niri_action", named).await;
    desk.server
        .call("noctalia", json!({"args": [PRIVATE]}))
        .await;

    let text = std::fs::read_to_string(desk.fixture.audit_log()).unwrap();
    assert!(!text.contains(PRIVATE), "{text}");
    let logged: Vec<Value> = desk
        .audited()
        .into_iter()
        .filter(|line| line[0] != "release_desktop")
        .map(|line| json!([line[0], line[1], line[4]]))
        .collect();
    let garbled =
        |bytes: usize| json!({"action": {"category": "invalid_action", "keys": 1, "bytes": bytes}});
    let panel_open = |bytes: usize| {
        json!({
            "count": 2, "bytes": [10, bytes], "command": "panel-open",
            "description": "Open a panel by id, optionally with context (e.g. launcher /emo, control-center audio)"
        })
    };
    assert_eq!(
        logged,
        [
            json!(["niri_action", {"action": {"Spawn": {"command": {"count": 2, "bytes": [4, 25]}}}}, null]),
            json!(["noctalia", {"count": 2, "bytes": [25, 25]}, "upstream_error"]),
            json!(["noctalia", panel_open(25), null]),
            json!(["noctalia", {"count": 2, "bytes": [10, 47]}, "invalid_arguments"]),
            json!(["niri_action", {"action": {"MoveWindowToWorkspace": {
                "window_id": 2, "reference": {"Name": {"bytes": 25}}, "focus": false
            }}}, "cancelled"]),
            json!(["niri_action", garbled(32), "invalid_arguments"]),
            json!(["niri_action", garbled(49), "invalid_arguments"]),
            json!(["niri_action", {"action": {"SetWorkspaceName": {"name": {"bytes": 25}, "workspace": null}}}, "lease_required"]),
            json!(["noctalia", {"count": 1, "bytes": [25]}, "lease_required"]),
        ]
    );
}

#[tokio::test]
async fn noctalia_calls_are_named_only_as_noctalias_own_lists_name_them() {
    let on = [("NIRI_COMPUTER_USE_UNRESTRICTED", "1")];
    let mut desk = Desk::start_with("act-noctalia-named", "", &on).await;
    for args in [
        json!(["clipboard-copy", PRIVATE]),
        json!(["plugin", format!("{PRIVATE}:entry"), "all", PRIVATE]),
        json!(["plugin", "ayagmar/obs-control:controller", "all", PRIVATE]),
    ] {
        desk.server.call("noctalia", json!({ "args": args })).await;
    }

    let text = std::fs::read_to_string(desk.fixture.audit_log()).unwrap();
    assert!(!text.contains(PRIVATE), "{text}");
    let logged: Vec<Value> = desk
        .audited()
        .into_iter()
        .filter(|line| line[0] == "noctalia")
        .map(|line| line[1].clone())
        .collect();
    let plugin = |target: usize| {
        json!({
            "count": 4, "bytes": [6, target, 3, 25],
            "command": "plugin", "description": "Dispatch an event to a plugin entry"
        })
    };
    let mut listed = plugin(30);
    listed["plugin"] = json!("ayagmar/obs-control");
    assert_eq!(
        logged,
        [
            json!({
                "count": 2, "bytes": [14, 25],
                "command": "clipboard-copy", "description": "Copy text to the clipboard"
            }),
            plugin(31),
            listed,
        ]
    );
}

#[tokio::test]
async fn a_refused_noctalia_call_asks_noctalia_for_no_list() {
    let on = [("NIRI_COMPUTER_USE_UNRESTRICTED", "1")];
    let mut desk = Desk::start_with("act-noctalia-unnamed", "", &on).await;
    let framed = json!({"args": ["caffeine-toggle", "a\nb"]});
    desk.server.call("noctalia", framed).await;
    let released = json!({"restore_focus": false});
    desk.server.call("release_desktop", released).await;
    let plugin = json!({"args": ["plugin", "ayagmar/obs-control:controller", "all", "go"]});
    desk.server.call("noctalia", plugin).await;

    assert_eq!(desk.noctalia.list_requests(), 0);
    let logged: Vec<Value> = desk
        .audited()
        .into_iter()
        .filter(|line| line[0] == "noctalia")
        .map(|line| json!([line[1], line[4]]))
        .collect();
    assert_eq!(
        logged,
        [
            json!([{"count": 2, "bytes": [15, 3]}, "invalid_arguments"]),
            json!([{"count": 4, "bytes": [6, 30, 3, 2]}, "lease_required"]),
        ]
    );
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
    // A record separator would make Noctalia read the rest as the whole command.
    for framing in ["x\u{1e}panel-close wallpaper", "a\nb", "a\rb"] {
        let framed = json!({"args": ["panel-open", framing]});
        let refused = open.server.call("noctalia", framed).await;
        assert!(
            mistake(&refused).contains("`args[1]` holds a record separator"),
            "{refused}"
        );
    }
    // Nothing more reached Noctalia.
    assert_eq!(
        open.noctalia.panel_commands(),
        ["panel-open wallpaper", "plugin x:y all go"]
    );
    assert_eq!(
        open.audited()[0],
        json!([
            "noctalia",
            {
                "count": 2, "bytes": [10, 9], "command": "panel-open",
                "description": "Open a panel by id, optionally with context (e.g. launcher /emo, control-center audio)"
            },
            true,
            "sent",
            null
        ])
    );
}
