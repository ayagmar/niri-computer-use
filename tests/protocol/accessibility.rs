//! Turning the session's accessibility on when a window's app isn't on the accessibility
//! bus: the server registers a listener, as a screen reader does, and never writes
//! `org.a11y.Status`. The mock launcher keeps `IsEnabled` true while a listener's
//! connection lasts, as at-spi2-core 2.61 and later do, unless a test makes it older.

use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::atspi::{self, Bus, Mock, Object};
use crate::client::{Server, WAIT, tool_error};
use crate::elements::{WINDOW, app_window, objects, workspace};
use crate::fixture::{self, Fixture, shared_mode};
use crate::niri::{Niri, Stream};

/// A process ID no application on the mock's bus has.
const OFF_BUS_PID: u32 = 1;
/// The server's budget for a request on the accessibility bus.
const BUDGET: Duration = Duration::from_secs(3);

/// A server on a fake niri whose window 1 belongs to an app off the accessibility bus.
///
/// Fields drop in this order: the server first, so it can't react to the rest going away,
/// and the fixture last.
struct Desk {
    server: Server,
    _niri: Niri,
    _stream: Stream,
    bus: Bus,
    mock: Mock,
    fixture: Fixture,
}

impl Desk {
    async fn start(name: &str, policy: &str, window: Value) -> Self {
        Self::start_with(name, policy, window, objects()).await
    }

    async fn start_with(
        name: &str,
        policy: &str,
        window: Value,
        objects: Vec<(&'static str, Object)>,
    ) -> Self {
        let mut fixture = Fixture::new(name);
        std::fs::create_dir_all(fixture.path("config/niri-computer-use")).unwrap();
        std::fs::write(fixture.path("config/niri-computer-use/policy.toml"), policy).unwrap();
        let (bus, mock) = atspi::start(&mut fixture, &["/frame"], objects).await;
        let windows = [window];
        let mut niri = Niri::start(&fixture);
        niri.set_windows(&windows, &[workspace()]);
        let server = Server::start(&fixture).await;
        let stream = niri.stream().await;
        stream.initial(&windows);
        Self {
            server,
            _niri: niri,
            _stream: stream,
            bus,
            mock,
            fixture,
        }
    }

    /// `elements` of window 1, which must fail: its error's name and detail.
    async fn refused(&mut self) -> (String, String) {
        tool_error(
            &self
                .server
                .call("elements", json!({"window_id": WINDOW}))
                .await,
        )
    }

    async fn switch(&mut self) -> Value {
        self.server.structured("status").await["accessibility_switch"].clone()
    }
}

/// Window 1 of an app that isn't on the accessibility bus.
fn off_bus_window() -> Value {
    let mut window = app_window(WINDOW, true);
    window["pid"] = json!(OFF_BUS_PID);
    window
}

/// Whether `IsEnabled` reads `on` within `WAIT`.
async fn turns(bus: &Bus, on: bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < WAIT {
        if bus.is_enabled().await == on {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    false
}

#[tokio::test]
async fn an_app_off_the_bus_turns_accessibility_on_until_the_server_ends_however_it_ends() {
    let mut desk = Desk::start("acc-on", "", off_bus_window()).await;
    assert!(!desk.bus.is_enabled().await);
    let (name, detail) = desk.refused().await;
    assert_eq!(name, "not_accessible");
    assert!(
        detail.contains("was off, so the server turned it on") && detail.contains("ask the user"),
        "{detail}"
    );
    // A restart helps only an app that checks at start, which the server can't tell, so the
    // agent gets the window's app_id and the condition, not a request to pass on.
    assert!(
        detail.contains("org.ncu.Mock")
            && detail.contains("if it is a Chromium, Electron or CEF app"),
        "{detail}"
    );
    // The launcher's GSettings write outlives the server, so the agent is told of it.
    assert!(
        detail.contains("org.gnome.desktop.interface toolkit-accessibility")
            && detail.contains("stays after the server ends"),
        "{detail}"
    );
    assert!(desk.bus.is_enabled().await);
    assert_eq!(desk.mock.listeners().len(), 1);
    assert_eq!(desk.switch().await, "on");
    // The listener already holds it on: no second one, and the agent is told to ask.
    let (_, again) = desk.refused().await;
    assert!(again.contains("is on"), "{again}");
    assert_eq!(desk.mock.listeners().len(), 1);
    assert_eq!(desk.mock.status_writes(), 0);
    // Killed outright, the engine still lets it go back off: its connection ends with it.
    let engine = desk.server.serving_pid().await;
    fixture::kill(engine);
    assert!(turns(&desk.bus, false).await);
    assert_eq!(desk.mock.status_writes(), 0);
    desk.fixture.started(engine);
}

/// The user turning accessibility off over the server's listener means it off: the server
/// lets its listener go, or the next screen reader to start or end would turn it back on.
#[tokio::test]
async fn accessibility_turned_off_over_the_server_is_let_go_for_good() {
    let mut desk = Desk::start("acc-off", "", off_bus_window()).await;
    desk.refused().await;
    let listeners = desk.mock.listeners();
    desk.mock.written_off();
    let (_, detail) = desk.refused().await;
    assert!(detail.contains("let its listener go"), "{detail}");
    assert!(!desk.bus.connected(&listeners[0]).await);
    assert_eq!(desk.switch().await, "let_go");
    let (_, again) = desk.refused().await;
    assert!(again.contains("let its listener go"), "{again}");
    assert_eq!(desk.mock.listeners().len(), 1);
    assert!(!desk.bus.is_enabled().await);
    assert_eq!(desk.mock.status_writes(), 0);
}

#[tokio::test]
async fn accessibility_already_on_is_left_alone() {
    let mut desk = Desk::start("acc-reader", "", off_bus_window()).await;
    // As a running screen reader leaves it.
    desk.mock.accessibility_on();
    let (name, detail) = desk.refused().await;
    assert_eq!(name, "not_accessible");
    assert!(
        detail.contains("the session's accessibility is on"),
        "{detail}"
    );
    assert!(
        detail.contains("ask the user") && detail.contains("if it is a Chromium"),
        "{detail}"
    );
    assert_eq!(desk.mock.listeners(), [""; 0]);
    assert_eq!(desk.mock.status_writes(), 0);
    assert_eq!(desk.switch().await, "off");
}

#[tokio::test]
async fn the_policy_file_keeps_the_server_from_turning_accessibility_on() {
    let mut desk = Desk::start(
        "acc-refused",
        "turn_on_accessibility = false",
        off_bus_window(),
    )
    .await;
    let (name, detail) = desk.refused().await;
    assert_eq!(name, "not_accessible");
    assert!(detail.contains("turn_on_accessibility = false"), "{detail}");
    assert_eq!(desk.mock.listeners(), [""; 0]);
    assert!(!desk.bus.is_enabled().await);
    let status = desk.server.structured("status").await;
    assert_eq!(status["policy"]["turn_on_accessibility"], false, "{status}");
    assert_eq!(status["accessibility_switch"], "off", "{status}");
}

#[tokio::test]
async fn a_launcher_that_ignores_listeners_is_let_go_and_named() {
    let mut desk = Desk::start("acc-older", "", off_bus_window()).await;
    desk.mock.older_launcher();
    // Reads that take a while, so the last one ends past the 500 ms the server waits.
    desk.mock
        .slow_on("IsEnabled", "/org/a11y/bus", Duration::from_millis(30));
    let (_, detail) = desk.refused().await;
    assert!(detail.contains("before 2.61"), "{detail}");
    let listeners = desk.mock.listeners();
    assert_eq!(listeners.len(), 1);
    // Its listener, which did nothing, is gone, and isn't registered again.
    assert!(!desk.bus.connected(&listeners[0]).await);
    assert_eq!(desk.switch().await, "unsupported");
    let (_, again) = desk.refused().await;
    assert!(again.contains("before 2.61"), "{again}");
    assert_eq!(desk.mock.listeners().len(), 1);
    assert_eq!(desk.mock.status_writes(), 0);
}

/// A call with less than the launcher's time left, as a wait's walk near its end has,
/// reads `IsEnabled` before a launcher that takes a moment turned it on: that is no sign
/// of an older launcher, so a later call with time to spare still turns it on.
#[tokio::test]
async fn a_launcher_still_turning_on_when_a_call_runs_out_is_asked_again() {
    let mut desk = Desk::start("acc-late", "", off_bus_window()).await;
    desk.mock.slow_launcher(Duration::from_millis(350));
    // Its one walk has 150 ms.
    let until = json!({"element": {"window_id": WINDOW, "role": "button"}});
    let waited = desk
        .server
        .structured_with("wait_for", json!({"until": until, "timeout_ms": 200}))
        .await;
    assert_ne!(desk.switch().await, "unsupported", "{waited}");
    let (_, detail) = desk.refused().await;
    assert!(
        detail.contains("was off, so the server turned it on"),
        "{detail}"
    );
    assert_eq!(desk.switch().await, "on");
}

#[tokio::test]
async fn a_registry_that_never_answers_holds_neither_elements_past_its_budget_nor_status() {
    let mut desk = Desk::start("acc-hung", "", off_bus_window()).await;
    let held = desk
        .mock
        .hold_on("RegisterEvent", "/org/a11y/atspi/registry");
    let started = Instant::now();
    let id = desk
        .server
        .start_call("elements", json!({"window_id": WINDOW}))
        .await;
    held.arrived().await;
    assert_eq!(desk.switch().await, "turning_on");
    let result = desk.server.response(id).await;
    let took = started.elapsed();
    let (name, detail) = tool_error(&result["result"]);
    assert_eq!(name, "not_accessible");
    assert!(
        detail.contains("registering the server's listener failed") && detail.contains("no answer"),
        "{detail}"
    );
    assert!(took < BUDGET + Duration::from_millis(500), "{took:?}");
    assert_eq!(desk.switch().await, "off");
    held.release();
}

#[tokio::test]
async fn a_launcher_that_never_answers_holds_no_elements_call_past_its_budget() {
    let mut desk = Desk::start("acc-mute", "", off_bus_window()).await;
    let held = desk.mock.hold_on("IsEnabled", "/org/a11y/bus");
    let started = Instant::now();
    let (name, detail) = desk.refused().await;
    let took = started.elapsed();
    assert_eq!(name, "not_accessible");
    assert!(detail.contains("couldn't be read"), "{detail}");
    assert!(took < BUDGET + Duration::from_millis(500), "{took:?}");
    assert_eq!(desk.mock.listeners(), [""; 0]);
    held.release();
}

/// Each standalone server holds a listener of its own, the second one joining the first's;
/// in shared mode the engine holds one for all its clients. Either way, accessibility
/// stays on while one is left, and goes off once none is.
#[tokio::test]
async fn accessibility_stays_on_while_any_server_still_holds_it() {
    let mut desk = Desk::start("acc-two", "", off_bus_window()).await;
    let mut second = Server::start(&desk.fixture).await;
    desk.refused().await;
    let (_, detail) = tool_error(&second.call("elements", json!({"window_id": WINDOW})).await);
    assert!(detail.contains("is on"), "{detail}");
    let engines = if shared_mode() { 1 } else { 2 };
    assert_eq!(desk.mock.listeners().len(), engines);
    let first = desk.server.serving_pid().await;
    let other = second.serving_pid().await;
    if !shared_mode() {
        fixture::kill(first);
        desk.fixture.started(first);
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(desk.bus.is_enabled().await);
    }
    fixture::kill(other);
    desk.fixture.started(other);
    assert!(turns(&desk.bus, false).await);
    assert_eq!(desk.mock.status_writes(), 0);
}

#[tokio::test]
async fn a_frame_without_its_tree_is_asked_for_its_attributes_and_read_again() {
    let mut objects = objects();
    let frame = objects
        .iter_mut()
        .find(|(path, _)| *path == "/frame")
        .map(|(_, object)| object)
        .unwrap();
    // As Chromium's frame is until an assistive technology asks for attributes.
    frame.built = Some(std::mem::take(&mut frame.children));
    frame.children = vec!["/org/a11y/atspi/null".to_owned()];
    let mut desk = Desk::start_with("acc-built", "", app_window(WINDOW, true), objects).await;
    let listing = desk
        .server
        .structured_with("elements", json!({"window_id": WINDOW, "format": "json"}))
        .await;
    let safe = listing["elements"]
        .as_array()
        .unwrap()
        .iter()
        .any(|element| element["name"] == "Safe");
    assert!(safe, "{listing}");
    // The app may still be sending its tree, so the agent is told to list again.
    assert_eq!(listing["building"], true, "{listing}");
    assert_eq!(desk.mock.calls("GetAttributes"), 1);
    // Built, the frame isn't asked again.
    let again = desk
        .server
        .structured_with("elements", json!({"window_id": WINDOW, "format": "json"}))
        .await;
    assert_eq!(again.get("building"), None, "{again}");
    assert_eq!(desk.mock.calls("GetAttributes"), 1);
    assert_eq!(desk.mock.listeners(), [""; 0]);
}
