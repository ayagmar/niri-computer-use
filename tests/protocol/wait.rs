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
            json!({"element": {"window_id": 1, "state": "disabled"}}),
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

/// A server on a fake niri whose focused window 1 is a mock application's, beside window 2
/// of an app not on the accessibility bus, holding the lease when `lease` says. The mock has a button
/// named `Later` that no object lists as its child yet.
///
/// Fields drop in this order: the server first, so it can't react to the rest going away,
/// and the fixture last.
struct App {
    server: Server,
    niri: Niri,
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
            niri,
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
    let detail = outcome["detail"].as_str().unwrap_or_default();
    assert!(detail.contains("role and name"), "{outcome}");
}

// The bug: a window that closes during the wait left to the timeout.
#[tokio::test]
async fn a_window_that_closes_ends_an_element_wait_at_once() {
    let mut app = App::start("wait-element-closed", false).await;
    let arguments = json!({"until": {"element": {"window_id": WINDOW, "name_contains": "never"}}});
    let started = Instant::now();
    let id = app.server.start_call("wait_for", arguments).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let windows = [window_on(2, Some("other"), 1, true)];
    app.niri.set_windows(&windows, &[workspace()]);
    app.stream.send(&json!({"WindowClosed": {"id": WINDOW}}));
    let result = app.server.response(id).await["result"].clone();
    let outcome = report(&result);
    assert_eq!(outcome["observed"], "window_gone", "{outcome}");
    assert!(started.elapsed() < Duration::from_secs(2), "{outcome}");
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
