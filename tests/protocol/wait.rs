//! `wait_for` over stdio: waiting on the events the test sends, as niri would, on the
//! screen through a fake grim, and on a mock application's accessible elements.

use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::atspi::{self, Bus, Mock, Object};
use crate::client::{Server, mistake, report};
use crate::elements::{WINDOW, app_window, objects, workspace};
use crate::fixture::{Fixture, jpeg};
use crate::niri::{Niri, Stream, window_on};
use crate::noctalia::{self, UNLOCKED};

/// AT-SPI's `sensitive` state, whose absence the compact listing prints as `disabled`.
const SENSITIVE: u32 = 1 << 24;

/// A server on a fake niri with windows 1 (focused, `a`, titled `t`) and 2 (`b`), and a
/// grim that always captures the same 1280-wide image. It doesn't take the lease.
async fn start(name: &str) -> (Fixture, Niri, Stream, Server) {
    let fixture = Fixture::new(name);
    fixture.grim(&jpeg(1280, 720, b"still"));
    let mut niri = Niri::start(&fixture);
    let server = Server::start(&fixture).await;
    let stream = niri.stream().await;
    stream.workspaces(1);
    stream.send(&json!({"WindowsChanged": {"windows": [
        window_on(1, Some("a"), 1, true),
        window_on(2, Some("b"), 1, false),
    ]}}));
    stream.send(&json!({"OverviewOpenedOrClosed": {"is_open": false}}));
    (fixture, niri, stream, server)
}

/// A window `id` with `app_id` and `title`, focused or not, as niri sends it when it opens
/// or changes.
fn opened(id: u64, app_id: &str, title: &str, focused: bool) -> Value {
    let mut window = window_on(id, Some(app_id), 1, focused);
    window["title"] = json!(title);
    json!({"WindowOpenedOrChanged": {"window": window}})
}

/// Starts `wait_for`, sends `events` a moment later, and returns the structured result
/// without `waited_ms`.
async fn waited(server: &mut Server, stream: &Stream, arguments: Value, events: &[Value]) -> Value {
    let id = server.start_call("wait_for", arguments).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    for event in events {
        stream.send(event);
    }
    let result = server.response(id).await["result"].clone();
    assert_eq!(result["isError"], false, "{result}");
    let mut outcome = report(&result);
    assert!(outcome["waited_ms"].is_u64(), "{outcome}");
    outcome.as_object_mut().unwrap().remove("waited_ms");
    outcome
}

#[tokio::test]
async fn a_window_that_appears_ends_the_wait_and_its_title_is_never_logged() {
    let (fixture, _niri, stream, mut server) = start("wait-window").await;
    let until = json!({"until": {"window": {"app_id": "dialog", "title": "Save"}}});
    let events = [
        opened(3, "dialog", "Open File", false),
        opened(4, "dialog", "Save As", false),
    ];
    assert_eq!(
        waited(&mut server, &stream, until, &events).await,
        json!({"observed": "met", "windows": [4], "focused_window": 1})
    );
    let logged = fixture
        .audit_lines()
        .into_iter()
        .find(|line| line["tool"] == "wait_for")
        .unwrap();
    assert_eq!(
        logged["args"],
        json!({"until": {"window": {"app_id_len": 6, "title_len": 4}}, "timeout_ms": 10000})
    );
}

#[tokio::test]
async fn closes_and_titles_end_the_wait_and_nothing_else_does() {
    let (_fixture, _niri, stream, mut server) = start("wait-follow").await;
    let closed = json!({"until": {"closed": 2}});
    assert_eq!(
        waited(
            &mut server,
            &stream,
            closed,
            &[json!({"WindowClosed": {"id": 2}})]
        )
        .await,
        json!({"observed": "met", "windows": [2], "focused_window": 1})
    );
    let title = json!({"until": {"title": {"window_id": 1, "contains": "done"}}});
    let events = [
        opened(1, "a", "make: running", true),
        opened(1, "a", "make: done", true),
    ];
    assert_eq!(
        waited(&mut server, &stream, title, &events).await["observed"],
        "met"
    );
    let never = json!({"until": {"window": {"app_id": "nope"}}, "timeout_ms": 200});
    assert_eq!(
        waited(&mut server, &stream, never, &[]).await,
        json!({"observed": "timeout", "focused_window": 1})
    );
}

#[tokio::test]
async fn conditions_that_cant_be_met_are_argument_mistakes() {
    let (_fixture, _niri, _stream, mut server) = start("wait-mistakes").await;
    for (until, says) in [
        (json!({"window": {}}), "`window` needs"),
        (
            json!({"title": {"window_id": 9, "contains": "x"}}),
            "no window with id 9",
        ),
        (
            json!({"title": {"window_id": 1, "contains": ""}}),
            "must not be empty",
        ),
        (json!({"element": {"window_id": 1}}), "`element` needs"),
        (
            json!({"element": {"window_id": 1, "name_contains": ""}}),
            "must not be empty",
        ),
        (
            json!({"element": {"window_id": 1, "state": "greyed"}}),
            "unknown state",
        ),
        (
            json!({"element": {"window_id": 1, "role": "push button"}}),
            "unknown role",
        ),
        (
            json!({"element": {"window_id": 9, "role": "button"}}),
            "no window with id 9",
        ),
    ] {
        let result = server.call("wait_for", json!({ "until": until })).await;
        assert!(mistake(&result).contains(says), "{result}");
    }
    let long = server
        .call(
            "wait_for",
            json!({"until": "screen_stable", "timeout_ms": 60000}),
        )
        .await;
    assert!(mistake(&long).contains("100 to 30000"), "{long}");
}

#[tokio::test]
async fn a_slow_capture_respects_the_screen_wait_budget() {
    let (fixture, _niri, _stream, mut server) = start("wait-slow-frame").await;
    fixture.program("grim", "exec sleep 30");
    let first_start = Instant::now();
    let failed = server
        .call(
            "wait_for",
            json!({"until": "screen_stable", "timeout_ms": 200}),
        )
        .await;
    assert_eq!(crate::client::tool_error(&failed).0, "deadline_exceeded");
    assert!(first_start.elapsed() < Duration::from_secs(1));

    fixture.program(
        "grim",
        r#"
        if [ -e "$DIR/frame.taken" ]; then exec sleep 30; fi
        : > "$DIR/frame.taken"
        cat "$DIR/grim.out"
    "#,
    );
    let start = Instant::now();
    let result = server
        .call(
            "wait_for",
            json!({"until": "screen_stable", "timeout_ms": 200, "screenshot": true}),
        )
        .await;
    assert_eq!(result["isError"], false);
    assert_eq!(report(&result)["observed"], "timeout");
    assert_eq!(report(&result)["screenshot"]["settled"], false);
    assert!(start.elapsed() < Duration::from_secs(1));
}

#[tokio::test]
async fn a_still_screen_is_met_with_its_screenshot() {
    let (_fixture, _niri, _stream, mut server) = start("wait-screen").await;
    let result = server
        .call(
            "wait_for",
            json!({"until": "screen_stable", "screenshot": true}),
        )
        .await;
    assert_eq!(result["isError"], false, "{result}");
    let outcome = &report(&result);
    assert_eq!(outcome["observed"], "met", "{outcome}");
    assert_eq!(outcome["screenshot"]["settled"], true, "{outcome}");
    // Without the lease, the ref is only for zooming.
    let id = outcome["screenshot"]["screenshot_ref"].as_str().unwrap();
    assert!(id.starts_with("shot-"), "{outcome}");
    let content = result["content"].as_array().unwrap();
    assert_eq!(content.len(), 2, "{result}");
    assert_eq!(content[1]["type"], "image", "{result}");
}

// The bug: every capture a screen wait took kept a ref, about ten a second, and dropped the
// one before only after keeping the next, so with the 64 kept refs full each capture pushed
// one of the agent's out, and a wait without a screenshot kept a ref nobody got.
#[tokio::test]
async fn a_screen_wait_keeps_the_agents_refs() {
    let (fixture, _niri, _stream, mut server) = start("wait-screen-refs").await;
    server.structured("desktop_state").await;
    let mut kept = Vec::new();
    for _ in 0..64 {
        let shot = server
            .call("screenshot", json!({"target": "focused_output"}))
            .await;
        kept.push(report(&shot)["screenshot_ref"].as_str().unwrap().to_owned());
    }
    // Every capture differs, as with a spinner on screen.
    fixture.program(
        "grim",
        r#"n=$(($(cat "$DIR/frames" 2>/dev/null || echo 0) + 1)); echo "$n" > "$DIR/frames"
cat "$DIR/grim.out"; echo "$n""#,
    );
    let unseen = json!({"until": "screen_stable", "timeout_ms": 1000});
    assert_eq!(
        report(&server.call("wait_for", unseen).await)["observed"],
        "timeout"
    );
    let seen = json!({"until": "screen_stable", "timeout_ms": 1000, "screenshot": true});
    let waited = report(&server.call("wait_for", seen).await);
    assert_eq!(waited["observed"], "timeout", "{waited}");
    // The returned screenshot's ref is the only new one, so it alone pushed out the oldest.
    kept.remove(0);
    kept.push(
        waited["screenshot"]["screenshot_ref"]
            .as_str()
            .unwrap()
            .to_owned(),
    );
    // Oldest first: each zoom keeps a ref too, which pushes out the one it just zoomed.
    for id in &kept {
        // At the output's own scale, twice the image's, as large as what grim writes.
        let region = json!({"x": 0, "y": 0, "width": 640, "height": 360});
        let zoom = json!({"target": "region", "screenshot_ref": id, "region": region});
        let zoomed = server.call("screenshot", zoom).await;
        assert_eq!(zoomed["isError"], false, "{id}: {zoomed}");
    }
}

/// A server on a fake niri whose focused window 1 is a mock application's, beside window 2
/// of an app not on the accessibility bus, holding the lease when `lease` says. The mock has a button
/// named `Later` that no object lists as its child yet.
///
/// Fields drop in this order: the server first, so it can't react to the rest going away,
/// and the fixture last.
struct App {
    server: Server,
    _niri: Niri,
    stream: Stream,
    _noctalia: noctalia::Reply,
    _bus: Bus,
    mock: Mock,
    fixture: Fixture,
}

impl App {
    async fn start(name: &str, lease: bool) -> Self {
        let mut fixture = Fixture::new(name);
        fixture.grim(&jpeg(1280, 720, b"still"));
        let mut objects = objects();
        objects.push(("/later", Object::button("Later", &["click"])));
        let (bus, mock) = atspi::start(&mut fixture, &["/frame"], objects).await;
        fixture.program("noctalia", "exit 0");
        let noctalia = noctalia::start(&fixture, UNLOCKED);
        let windows = [
            app_window(WINDOW, true),
            window_on(2, Some("other"), 1, false),
        ];
        let mut niri = Niri::start(&fixture);
        niri.set_windows(&windows, &[workspace()]);
        let mut server = Server::start(&fixture).await;
        let stream = niri.stream().await;
        stream.initial(&windows);
        if lease {
            server.structured("acquire_desktop").await;
        }
        Self {
            server,
            _niri: niri,
            stream,
            _noctalia: noctalia,
            _bus: bus,
            mock,
            fixture,
        }
    }

    /// `wait_for` on `element`, which names window 1 unless it says otherwise: the result
    /// and how long it took.
    async fn wait(&mut self, element: Value, timeout_ms: u32) -> (Value, Duration) {
        let mut element = element;
        element
            .as_object_mut()
            .unwrap()
            .entry("window_id")
            .or_insert(json!(WINDOW));
        let arguments = json!({"until": {"element": element}, "timeout_ms": timeout_ms});
        let started = Instant::now();
        let result = self.server.call("wait_for", arguments).await;
        assert_eq!(result["isError"], false, "{result}");
        (report(&result), started.elapsed())
    }
}

// The bug: a wait that reads once, or returns no ref the element tools take, leaves the
// agent listing again; a name it returns or logs leaks the app's text.
#[tokio::test]
async fn an_element_that_appears_ends_the_wait_with_a_ref_the_element_tools_take() {
    let mut app = App::start("wait-element-appears", true).await;
    let mock = app.mock.clone();
    let shows = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(400)).await;
        mock.change("/panel", |panel| panel.children.push("/later".to_owned()));
    });
    let wanted = json!({"role": "button", "name_contains": "later", "state": "enabled"});
    let (outcome, took) = app.wait(wanted, 5000).await;
    shows.await.unwrap();
    assert_eq!(outcome["observed"], "met", "{outcome}");
    assert_eq!(outcome["windows"], json!([WINDOW]), "{outcome}");
    assert!(took >= Duration::from_millis(400), "{took:?}");
    let element = &outcome["element"];
    assert_eq!(element["role"], "button", "{outcome}");
    assert!(!outcome.to_string().contains("Later"), "{outcome}");
    let id = element["element_ref"].as_str().unwrap();
    let arguments = json!({"element": id, "expect": {"window_id": WINDOW}});
    let acted = app
        .server
        .structured_with("activate_element", arguments)
        .await;
    assert_eq!(acted["accepted"], true, "{acted}");
    assert_eq!(app.mock.calls("DoAction"), 1);
    let logged = app
        .fixture
        .audit_lines()
        .into_iter()
        .find(|line| line["tool"] == "wait_for")
        .unwrap();
    assert_eq!(
        logged["args"]["until"],
        json!({"element": {
            "window_id": WINDOW, "role": "button", "name_contains_len": 5, "state": "enabled"
        }})
    );
}

// Without the lease the wait still works, and gives no ref, as `elements` doesn't.
#[tokio::test]
async fn an_element_wait_needs_no_lease_and_gives_no_ref_without_it() {
    let mut app = App::start("wait-element-no-lease", false).await;
    let (outcome, _) = app.wait(json!({"name_contains": "safe"}), 2000).await;
    assert_eq!(outcome["observed"], "met", "{outcome}");
    assert_eq!(outcome["element"]["element_ref"], Value::Null, "{outcome}");
}

/// The numbers of the element refs a listing of window 1 issues.
async fn ref_numbers(server: &mut Server) -> Vec<u64> {
    let listing = server
        .structured_with("elements", json!({"window_id": WINDOW, "format": "json"}))
        .await;
    let number = |element: &Value| {
        let id = element["element_ref"].as_str().unwrap();
        id.rsplit('-').next().unwrap().parse().unwrap()
    };
    listing["elements"]
        .as_array()
        .unwrap()
        .iter()
        .map(number)
        .collect()
}

// The bug: an `absent` wait that kept a ref for every walk that still saw the element, so
// a long wait pushed the agent's listed refs out of the thousand the lease keeps.
#[tokio::test]
async fn an_absent_wait_keeps_no_refs() {
    let mut app = App::start("wait-element-absent-refs", true).await;
    let before = ref_numbers(&mut app.server).await;
    let present = json!({"name_contains": "safe", "absent": true});
    let (outcome, _) = app.wait(present, 1000).await;
    assert_eq!(outcome["observed"], "timeout", "{outcome}");
    let after = ref_numbers(&mut app.server).await;
    assert_eq!(after[0], before.last().unwrap() + 1, "{before:?} {after:?}");
}

// The bug: `absent` met by a walk its budget cut short, which saw no match only because it
// stopped before reaching it; and a slow walk running past `timeout_ms`. Each of four
// objects answers in 0.9 s, so a walk runs out of its three seconds.
#[tokio::test]
async fn absent_waits_for_a_whole_walk_and_a_slow_walk_keeps_the_timeout() {
    let mut app = App::start("wait-element-absent", false).await;
    app.mock.change("/panel", |panel| {
        panel.children.retain(|child| child != "/button");
    });
    for path in ["/panel", "/odd", "/entry", "/password"] {
        app.mock
            .slow_on("GetState", path, Duration::from_millis(900));
    }
    let gone = json!({"role": "button", "name_contains": "safe", "absent": true});
    let (slow, took) = app.wait(gone.clone(), 1000).await;
    assert_eq!(slow["observed"], "timeout", "{slow}");
    assert!(took < Duration::from_millis(1500), "{took:?}");
    let (cut, _) = app.wait(gone.clone(), 4000).await;
    assert_eq!(cut["observed"], "timeout", "{cut}");
    assert_eq!(
        cut["detail"], "the last walk was cut short: budget_exhausted",
        "{cut}"
    );
    for path in ["/panel", "/odd", "/entry", "/password"] {
        app.mock.slow_on("GetState", path, Duration::ZERO);
    }
    let (whole, _) = app.wait(gone, 2000).await;
    assert_eq!(whole["observed"], "met", "{whole}");
}

// The bug: `absent` met by a walk that skipped an object gone by the time it read it, and
// with it everything under it, where the element may still be.
#[tokio::test]
async fn absent_isnt_met_by_a_walk_that_lost_part_of_the_tree() {
    let mut app = App::start("wait-element-lost", false).await;
    app.mock.fail_named(
        "GetRole",
        "/panel",
        "org.freedesktop.DBus.Error.UnknownObject",
    );
    let gone = json!({"name_contains": "safe", "absent": true});
    let (outcome, _) = app.wait(gone, 1000).await;
    assert_eq!(outcome["observed"], "timeout", "{outcome}");
}

// The bug: an `absent` wait that named the actions of the element it still saw, which it
// throws away, so an app slow to name them made every walk look cut short, and a
// timeout told the agent to narrow a condition whose walks had read the whole tree.
#[tokio::test]
async fn an_absent_wait_names_no_actions() {
    let mut app = App::start("wait-el-unnamed", false).await;
    app.mock.change("/button", |button| {
        button.listed = Some(vec![String::new()]);
    });
    app.mock
        .slow_on("GetName", "/button", Duration::from_millis(1500));
    let present = json!({"name_contains": "safe", "absent": true});
    let (outcome, _) = app.wait(present, 1500).await;
    assert_eq!(outcome["observed"], "timeout", "{outcome}");
    assert_eq!(outcome["detail"], Value::Null, "{outcome}");
    assert_eq!(app.mock.calls("GetName"), 0);
}

// The bug: a walk that stopped at its second match, before the second `Delete` of a list
// put on the bus without rows, handed back a bound ref that `elements`, reading on, marks
// `unbound: repeated`, so it could act on another record.
#[tokio::test]
async fn an_appeared_element_alike_another_further_on_is_unbound() {
    let mut app = App::start("wait-el-repeated", true).await;
    app.mock.change("/panel", |panel| {
        panel.children = ["/button", "/odd", "/later"].map(str::to_owned).to_vec();
    });
    app.mock
        .change("/button", |button| button.name = "Delete".to_owned());
    app.mock
        .change("/odd", |odd| odd.name = "Delete all".to_owned());
    app.mock
        .change("/later", |later| later.name = "Delete".to_owned());
    let wanted = json!({"role": "button", "name_contains": "del"});
    let (outcome, _) = app.wait(wanted, 3000).await;
    assert_eq!(outcome["observed"], "met", "{outcome}");
    assert_eq!(outcome["element"]["unbound"], "repeated", "{outcome}");
}

// The bug: a ref bound only because no element read had its role and name, from a walk
// that ran out of time before the rest of the tree. Each object after `Safe` answers in
// 0.9 s, as does the panel holding them, so the walk runs out of time before the last.
#[tokio::test]
async fn an_appeared_element_a_cut_walk_cant_tell_apart_gets_no_ref() {
    let mut app = App::start("wait-el-unchecked", true).await;
    for path in ["/panel", "/odd", "/entry", "/password"] {
        app.mock
            .slow_on("GetState", path, Duration::from_millis(900));
    }
    let (outcome, _) = app.wait(json!({"name_contains": "safe"}), 5000).await;
    assert_eq!(outcome["observed"], "met", "{outcome}");
    assert_eq!(outcome["element"]["element_ref"], Value::Null, "{outcome}");
    assert_eq!(outcome["element"]["unbound"], "cut_short", "{outcome}");
    let detail = outcome["detail"].as_str().unwrap_or_default();
    assert!(detail.contains("role and name"), "{outcome}");
}

// The bug: without the lease there is no ref to hold back, yet the result said one was
// withheld and sent the agent to aim at pixels, which needs the lease.
#[tokio::test]
async fn an_appeared_element_without_the_lease_has_no_withheld_detail() {
    let mut app = App::start("wait-el-unchecked-free", false).await;
    for path in ["/panel", "/odd", "/entry", "/password"] {
        app.mock
            .slow_on("GetState", path, Duration::from_millis(900));
    }
    let (outcome, _) = app.wait(json!({"name_contains": "safe"}), 5000).await;
    assert_eq!(outcome["observed"], "met", "{outcome}");
    assert_eq!(outcome["element"]["element_ref"], Value::Null, "{outcome}");
    assert_eq!(outcome["detail"], Value::Null, "{outcome}");
}

// The bug: the walk that read the match went on through the rest of the tree past
// `timeout_ms`, and the wait cut it off and lost the match. `Safe` and `Odd` come before
// two objects that answer in 0.9 s each.
#[tokio::test]
async fn a_match_read_by_a_walk_longer_than_the_wait_ends_it_met() {
    let mut app = App::start("wait-el-short", true).await;
    for path in ["/entry", "/password"] {
        app.mock
            .slow_on("GetState", path, Duration::from_millis(900));
    }
    let (outcome, took) = app.wait(json!({"role": "button"}), 1000).await;
    assert_eq!(outcome["observed"], "met", "{outcome}");
    assert_eq!(outcome["element"]["role"], "button", "{outcome}");
    assert!(took < Duration::from_millis(1200), "{took:?}");
}

// The bug: `disabled`, the one state the default listing prints for a button that can't
// be pressed, refused as unknown, and `state` named by what only the JSON listing shows.
// The mock's `Safe` button loses `sensitive`, then gets it back.
#[tokio::test]
async fn a_disabled_button_is_waited_on_as_the_compact_listing_names_it() {
    let mut app = App::start("wait-el-disabled", false).await;
    app.mock
        .change("/button", |button| button.states &= !SENSITIVE);
    let line = app
        .server
        .call(
            "elements",
            json!({"window_id": WINDOW, "name_contains": "safe"}),
        )
        .await;
    assert!(line.to_string().contains("[disabled]"), "{line}");
    let mock = app.mock.clone();
    let enables = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(400)).await;
        mock.change("/button", |button| button.states |= SENSITIVE);
    });
    let disabled = json!({"name_contains": "safe", "state": "disabled", "absent": true});
    let (outcome, took) = app.wait(disabled, 3000).await;
    enables.await.unwrap();
    assert_eq!(outcome["observed"], "met", "{outcome}");
    assert!(took >= Duration::from_millis(400), "{took:?}");
}

// The bug: after a walk that used the time the wait left it, naming the match's actions
// had none, and an app that names them only through `GetName`, as Chromium does, lost the
// match it had read: the wait timed out and blamed a cut walk.
#[tokio::test]
async fn a_match_whose_actions_the_wait_had_no_time_to_name_ends_it_met() {
    let mut app = App::start("wait-el-unnamed", true).await;
    app.mock.change("/button", |button| {
        button.listed = Some(vec![String::new()]);
    });
    for path in ["/entry", "/password"] {
        app.mock
            .slow_on("GetState", path, Duration::from_millis(900));
    }
    // Naming starts with no time left, but tokio's timer rounds that deadline up to its next
    // tick, so on a loaded machine a reply quicker than a tick still arrived in time, and the
    // wait blamed the cut walk instead. An app slower than any tick leaves no such margin.
    app.mock
        .slow_on("GetName", "/button", Duration::from_millis(500));
    let (outcome, took) = app.wait(json!({"role": "button"}), 1000).await;
    assert_eq!(outcome["observed"], "met", "{outcome}");
    assert_eq!(outcome["element"]["element_ref"], Value::Null, "{outcome}");
    let detail = outcome["detail"].as_str().unwrap_or_default();
    assert!(detail.contains("naming its actions"), "{outcome}");
    assert!(took < Duration::from_millis(1200), "{took:?}");
}

// The bug: an app slower to name an action than naming's own second, with most of the
// wait left, was told it had run out of the wait's time, and to wait longer, which
// would end the same way.
#[tokio::test]
async fn a_match_the_app_names_too_slowly_ends_the_wait_met_without_more_waiting() {
    let mut app = App::start("wait-el-slow-name", true).await;
    app.mock.change("/button", |button| {
        button.listed = Some(vec![String::new()]);
    });
    app.mock
        .slow_on("GetName", "/button", Duration::from_millis(1500));
    let (outcome, took) = app.wait(json!({"name_contains": "safe"}), 8000).await;
    assert_eq!(outcome["observed"], "met", "{outcome}");
    assert_eq!(outcome["element"]["element_ref"], Value::Null, "{outcome}");
    let detail = outcome["detail"].as_str().unwrap_or_default();
    assert!(detail.contains("longer to name its actions"), "{outcome}");
    assert!(!detail.contains("timeout_ms"), "{outcome}");
    assert!(took < Duration::from_secs(3), "{took:?}");
}

// The bug: a window that closes during the wait left to the timeout, or to the end of a
// walk. niri goes on listing the window, so only its event stream can end the wait, and
// the panel takes 0.9 s to answer, so the close comes mid-walk.
#[tokio::test]
async fn a_window_that_closes_ends_an_element_wait_at_once() {
    let mut app = App::start("wait-element-closed", false).await;
    app.mock
        .slow_on("GetState", "/panel", Duration::from_millis(900));
    let arguments = json!({"until": {"element": {"window_id": WINDOW, "name_contains": "never"}}});
    let started = Instant::now();
    let id = app.server.start_call("wait_for", arguments).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    app.stream.send(&json!({"WindowClosed": {"id": WINDOW}}));
    let result = app.server.response(id).await["result"].clone();
    let outcome = report(&result);
    assert_eq!(outcome["observed"], "window_gone", "{outcome}");
    assert!(started.elapsed() < Duration::from_millis(800), "{outcome}");
}

#[tokio::test]
async fn an_element_wait_on_a_window_off_the_accessibility_bus_says_so() {
    let mut app = App::start("wait-element-off-bus", false).await;
    let (outcome, _) = app
        .wait(json!({"window_id": 2, "role": "button"}), 2000)
        .await;
    assert_eq!(outcome["observed"], "not_accessible", "{outcome}");
    assert!(
        outcome["detail"]
            .as_str()
            .unwrap()
            .contains("no application with pid 100"),
        "{outcome}"
    );
}
