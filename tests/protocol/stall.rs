//! The stall-point explorer. Every review of the element actions found one shape of bug:
//! hold one reply back, from niri, Noctalia or the app, change one fact the gate had
//! already read, release, and the action went out on a stale answer. The explorer searches
//! for that shape instead of waiting for a review. It calls each acting tool once and
//! keeps the trace of the hold points the fakes passed, every reply the call awaited, so a
//! new await in the server is explored without new test code. Then, for each point of the
//! trace and each fact the tool's gates depend on, it calls the tool again, holds that
//! reply, changes the fact, waits for the server to see the change, releases the reply, a
//! moment later or past `LAST_GATE`, and checks the contract on what the fakes received:
//!
//! - Before the call that acts went out, the change refuses the call with the fact's error
//!   and nothing acts, or the call went out within `LAST_GATE` of the change, the window
//!   in which the element actions document that a change can be missed.
//! - After it went out, the call isn't refused, and its outcome doesn't report focus from
//!   before the change.
//! - Nothing acts twice.
//!
//! The keyboard tools act through `wtype`, which the fakes see but which passes no point,
//! so every point is before their send. The pointer and `paste` act through Wayland only in
//! nested runs (see docs/decisions.md).

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use niri_ipc::Action;
use serde_json::{Value, json};

use crate::atspi::{self, Bus, Mock, Object, PASSWORD_TEXT};
use crate::client::{Server, WAIT};
use crate::elements::{WINDOW, app_window, objects, workspace};
use crate::fixture::{Fixture, jpeg};
use crate::holds::{Passed, Points};
use crate::niri::{Niri, Stream, window_on};
use crate::noctalia::{self, UNLOCKED};

/// The server's bound on the age of what the element actions' last gate read.
const LAST_GATE: Duration = Duration::from_millis(250);
/// A hold this long makes a last gate pass run over.
const PAST_THE_LAST_GATE: Duration = Duration::from_millis(300);
/// An app the policy denies input to.
const DENIED: &str = "org.ncu.Denied";
const POLICY: &str = r#"
deny_input_app_ids = ["org.ncu.Denied"]

[[preset]]
name = "foot"
argv = ["foot"]
app_id = "foot"
"#;
/// The window of another app, which niri actions are about.
const TARGET: u64 = 2;

/// The acting tools the explorer calls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tool {
    Activate,
    SetText,
    FocusWindow,
    CloseWindow,
    NiriAction,
    Launch,
    Key,
    TypeText,
}

/// How a tool's call reaches the desktop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Element,
    Niri,
    Keyboard,
}

impl Tool {
    const fn name(self) -> &'static str {
        match self {
            Self::Activate => "activate_element",
            Self::SetText => "set_element_text",
            Self::FocusWindow => "focus_window",
            Self::CloseWindow => "close_window",
            Self::NiriAction => "niri_action",
            Self::Launch => "launch",
            Self::Key => "key",
            Self::TypeText => "type_text",
        }
    }

    const fn kind(self) -> Kind {
        match self {
            Self::Activate | Self::SetText => Kind::Element,
            Self::FocusWindow | Self::CloseWindow | Self::NiriAction | Self::Launch => Kind::Niri,
            Self::Key | Self::TypeText => Kind::Keyboard,
        }
    }

    /// The element an element action acts on: its name and path.
    const fn element(self) -> (&'static str, &'static str) {
        match self {
            Self::SetText => ("Field", "/entry"),
            Self::Activate
            | Self::FocusWindow
            | Self::CloseWindow
            | Self::NiriAction
            | Self::Launch
            | Self::Key
            | Self::TypeText => ("Safe", "/button"),
        }
    }

    /// Whether the hold point `point` is the call that acts reaching niri or the app.
    fn acts(self, point: &str) -> bool {
        match self.kind() {
            Kind::Element if self == Self::Activate => point.starts_with("atspi:DoAction:"),
            Kind::Element => point.starts_with("atspi:SetTextContents:"),
            Kind::Niri => point.starts_with("niri:Action:"),
            Kind::Keyboard => false,
        }
    }

    /// The arguments, with an element action's ref `element`.
    fn arguments(self, element: Option<&str>) -> Value {
        let expect = json!({"window_id": WINDOW});
        match self {
            Self::Activate => json!({"element": element, "expect": expect}),
            Self::SetText => json!({"element": element, "text": "abc", "expect": expect}),
            Self::FocusWindow | Self::CloseWindow => json!({"id": TARGET}),
            Self::NiriAction => json!({"action": {"ToggleWindowFloating": {"id": TARGET}}}),
            Self::Launch => json!({"preset": "foot"}),
            Self::Key => json!({"keys": ["Down"], "expect": expect}),
            Self::TypeText => json!({"text": "abc", "expect": expect}),
        }
    }
}

/// A fact the gates depend on, and the change the explorer makes to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fact {
    /// Keyboard focus moves to another app's window.
    FocusMoved,
    /// The element's window, or the window a niri action is about, closes.
    WindowClosed,
    /// Another server's input may be stuck.
    InputDirty,
    /// The window takes an `app_id` the policy denies. The policy is read once per session,
    /// so this is how a denial can appear while a call runs.
    DenyListed,
    /// The element becomes a password field.
    BecamePassword,
    /// The element is reused in place for another record.
    Renamed,
    /// Its parent is.
    AncestorRenamed,
    /// It moves into the app's other window, which isn't focused.
    MovedAway,
    /// Its actions are reordered, so its listed index names another action.
    ActionsReordered,
    /// It leaves the tree.
    Removed,
}

impl Fact {
    /// The errors the change refuses `tool` with before its call went out, or none when
    /// the tool doesn't depend on the fact.
    fn refusals(self, tool: Tool) -> Option<&'static [&'static str]> {
        let kind = tool.kind();
        let element = kind == Kind::Element;
        match self {
            Self::InputDirty => Some(&["recovery_required"]),
            Self::FocusMoved => (kind != Kind::Niri).then_some(&["focus_mismatch"]),
            Self::WindowClosed => match kind {
                Kind::Element => Some(&["element_stale"]),
                Kind::Keyboard => Some(&["focus_mismatch"]),
                Kind::Niri => (tool != Tool::Launch).then_some(&["invalid_arguments"]),
            },
            Self::DenyListed => {
                (kind != Kind::Niri || tool == Tool::CloseWindow).then_some(&["app_denied"])
            }
            Self::BecamePassword => element.then_some(&["secret_field"]),
            Self::Renamed | Self::AncestorRenamed | Self::MovedAway | Self::Removed => {
                element.then_some(&["element_stale"])
            }
            Self::ActionsReordered => (tool == Tool::Activate).then_some(&["element_stale"]),
        }
    }
}

/// How long a reply is held once the change was seen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hold {
    /// Released at once.
    Short,
    /// Released past `LAST_GATE`.
    Long,
}

/// One case: `tool`, holding the reply at `at`, of the trace's point `index`, while `fact`
/// changes.
#[derive(Debug)]
struct Case<'a> {
    tool: Tool,
    fact: Fact,
    at: &'a Passed,
    /// Where the call that acts is in the trace: the trace's length for the keyboard tools.
    sends: usize,
    index: usize,
    hold: Hold,
}

impl Case<'_> {
    const fn before_send(&self) -> bool {
        self.index < self.sends
    }
}

/// What a case's call came to, and what reached the fakes.
#[derive(Debug)]
struct Ran {
    result: Value,
    /// When each call that acts reached niri, the app or `wtype`.
    acts: Vec<Instant>,
    /// The indexes `DoAction` asked for.
    indexes: Vec<i32>,
    /// When the change began.
    changed: Instant,
}

/// The windows: the mock app's focused window 1, another app's window 2, and the mock
/// app's window 3, smaller, holding an empty panel.
fn windows() -> Vec<Value> {
    let mut other = app_window(3, false);
    other["title"] = json!("Other");
    other["layout"]["window_size"] = json!([640, 480]);
    vec![
        app_window(WINDOW, true),
        window_on(TARGET, Some("other"), 1, false),
        other,
    ]
}

/// The windows closed since the world's last reset.
type Closed = Arc<Mutex<BTreeSet<u64>>>;

/// The events niri sends once it carried out the actions the explorer's tools send. A
/// window closes once: niri says nothing more for one that already closed, which niri-ipc's
/// state, and so the server's, requires.
#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "the explorer's tools send these four actions; niri sends nothing for any other"
)]
fn effects(action: &Action, closed: &Closed) -> Vec<Value> {
    match action {
        Action::FocusWindow { id } => vec![json!({"WindowFocusChanged": {"id": id}})],
        Action::CloseWindow { id: Some(id) } if closed.lock().unwrap().insert(*id) => {
            vec![json!({"WindowClosed": {"id": id}})]
        }
        Action::ToggleWindowFloating { id: Some(id) } => {
            let mut window = window_on(*id, Some("other"), 1, false);
            window["is_floating"] = json!(true);
            vec![json!({"WindowOpenedOrChanged": {"window": window}})]
        }
        Action::Spawn { .. } => {
            let window = window_on(9, Some("foot"), 1, true);
            vec![json!({"WindowOpenedOrChanged": {"window": window}})]
        }
        _ => Vec::new(),
    }
}

/// A desktop for every tool: the mock app's windows 1 and 3 with their accessible
/// elements, another app's window 2, Noctalia, `wtype`, grim, and a policy that denies
/// `DENIED` and has the preset `foot`. The server holds the lease.
///
/// Fields drop in this order: the server first, so it can't react to the rest going away,
/// and the fixture last.
struct World {
    server: Server,
    _niri: Niri,
    stream: Stream,
    _noctalia: noctalia::Reply,
    _bus: Bus,
    mock: Mock,
    points: Points,
    closed: Closed,
    /// The mock's objects as they were before the last change.
    saved: Vec<(&'static str, Object)>,
    fixture: Fixture,
}

impl World {
    async fn start(name: &str) -> Self {
        let mut fixture = Fixture::new(name);
        fixture.set("NIRI_COMPUTER_USE_KEYBOARD", "wtype");
        std::fs::create_dir_all(fixture.path("config/niri-computer-use")).unwrap();
        std::fs::write(fixture.path("config/niri-computer-use/policy.toml"), POLICY).unwrap();
        fixture.program("noctalia", "exit 0");
        fixture.program(
            "wtype",
            "cat >> \"$DIR/wtype.in\"\necho \"$*\" >> \"$DIR/wtype.calls\"",
        );
        fixture.grim(&jpeg(1280, 720, b"evidence"));
        let mut objects = objects();
        objects.extend([
            ("/frame2", Object::frame("Other", (640, 480), &["/panel2"])),
            ("/panel2", Object::panel(&[])),
        ]);
        let (bus, mock) = atspi::start(&mut fixture, &["/frame", "/frame2"], objects).await;
        let noctalia = noctalia::start(&fixture, UNLOCKED);
        let mut niri = Niri::start(&fixture);
        niri.set_windows(&windows(), &[workspace()]);
        let closed = Closed::default();
        let closing = Arc::clone(&closed);
        niri.effects(move |action| effects(action, &closing));
        let mut server = Server::start(&fixture).await;
        let stream = niri.stream().await;
        stream.initial(&windows());
        server.structured("acquire_desktop").await;
        Self {
            server,
            _niri: niri,
            stream,
            _noctalia: noctalia,
            _bus: bus,
            mock,
            points: fixture.points(),
            closed,
            saved: Vec::new(),
            fixture,
        }
    }

    /// Puts every fact back as it was and returns `tool`'s arguments with a fresh element
    /// ref. The trace starts afterwards.
    async fn reset(&mut self, tool: Tool) -> Value {
        self.points.idle().await;
        let marker = self.fixture.runtime_dir().join("input-dirty");
        std::fs::remove_file(marker).ok();
        self.closed.lock().unwrap().clear();
        for (path, object) in self.saved.drain(..) {
            self.mock.change(path, |now| *now = object);
        }
        self.stream
            .send(&json!({"WindowsChanged": {"windows": windows()}}));
        self.until_desktop(initial).await;
        std::fs::remove_file(self.fixture.path("wtype.calls")).ok();
        let element = if tool.kind() == Kind::Element {
            Some(self.element(tool.element().0).await)
        } else {
            None
        };
        self.points.start();
        tool.arguments(element.as_deref())
    }

    /// The `element_ref` of the element named `name` in window 1.
    async fn element(&mut self, name: &str) -> String {
        let listing = self
            .server
            .structured_with("elements", json!({"window_id": WINDOW}))
            .await;
        let elements = listing["elements"].as_array().unwrap();
        let element = elements
            .iter()
            .find(|element| element["name"] == name)
            .unwrap_or_else(|| panic!("no {name} in {listing}"));
        element["element_ref"].as_str().unwrap().to_owned()
    }

    /// Waits until the server's view of niri passes `done`.
    async fn until_desktop(&mut self, done: impl Fn(&Value) -> bool) {
        let start = Instant::now();
        loop {
            let desktop = self.server.structured("desktop_state").await;
            if done(&desktop) {
                return;
            }
            assert!(start.elapsed() < WAIT, "{desktop}");
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    }

    /// Makes `fact` change for `tool`, and waits until the server can see it.
    async fn change(&mut self, fact: Fact, tool: Tool) {
        let window = if tool.kind() == Kind::Niri {
            TARGET
        } else {
            WINDOW
        };
        let (_, path) = tool.element();
        match fact {
            Fact::FocusMoved => {
                self.stream
                    .send(&json!({"WindowFocusChanged": {"id": TARGET}}));
                self.until_desktop(|desktop| desktop["focused_window"] == TARGET)
                    .await;
            }
            Fact::WindowClosed => {
                self.closed.lock().unwrap().insert(window);
                self.stream.send(&json!({"WindowClosed": {"id": window}}));
                self.until_desktop(|desktop| app_id(desktop, window).is_none())
                    .await;
            }
            Fact::DenyListed => {
                let mut denied = windows()[usize::from(window != WINDOW)].clone();
                denied["app_id"] = json!(DENIED);
                self.stream
                    .send(&json!({"WindowOpenedOrChanged": {"window": denied}}));
                self.until_desktop(|desktop| app_id(desktop, window).as_deref() == Some(DENIED))
                    .await;
            }
            Fact::InputDirty => {
                let marker = self.fixture.runtime_dir().join("input-dirty");
                std::fs::write(marker, "").unwrap();
            }
            Fact::BecamePassword => self.change_object(path, |object| object.role = PASSWORD_TEXT),
            Fact::Renamed => self.change_object(path, |object| {
                object.name = "Different record".to_owned();
            }),
            Fact::AncestorRenamed => self.change_object("/panel", |object| {
                object.name = "Another record".to_owned();
            }),
            Fact::ActionsReordered => self.change_object(path, |object| {
                object.actions = vec!["delete".to_owned(), "click".to_owned()];
            }),
            Fact::Removed => self.change_object("/panel", |panel| {
                panel.children.retain(|child| child != path);
            }),
            Fact::MovedAway => {
                self.change_object("/panel", |panel| {
                    panel.children.retain(|child| child != path);
                });
                self.change_object("/panel2", |panel| panel.children.push(path.to_owned()));
            }
        }
    }

    /// Changes the mock's object at `path`, keeping it as it was for `reset`.
    fn change_object(&mut self, path: &'static str, change: impl FnOnce(&mut Object)) {
        self.saved.push((path, self.mock.object(path)));
        self.mock.change(path, change);
    }

    /// Calls `tool` once with nothing held, and returns the trace of its call.
    async fn baseline(&mut self, tool: Tool) -> Vec<Passed> {
        let arguments = self.reset(tool).await;
        let result = self.server.call(tool.name(), arguments).await;
        let trace = self.points.trace();
        assert_eq!(result["isError"], false, "{}: {result}", tool.name());
        assert_eq!(
            self.acts(tool, &trace).len(),
            1,
            "{}: {trace:?}",
            tool.name()
        );
        trace
    }

    /// When each call that acts reached the fakes: from `trace`, or `wtype`'s calls.
    fn acts(&self, tool: Tool, trace: &[Passed]) -> Vec<Instant> {
        if tool.kind() != Kind::Keyboard {
            return trace
                .iter()
                .filter(|passed| tool.acts(&passed.point))
                .map(|passed| passed.at)
                .collect();
        }
        let calls = self.fixture.path("wtype.calls");
        let Ok(text) = std::fs::read_to_string(&calls) else {
            return Vec::new();
        };
        let written = std::fs::metadata(&calls).unwrap().modified().unwrap();
        let ago = SystemTime::now()
            .duration_since(written)
            .unwrap_or_default();
        vec![Instant::now().checked_sub(ago).unwrap(); text.lines().count()]
    }

    /// Runs `case`, or says why its point wasn't reached.
    async fn run(&mut self, case: &Case<'_>) -> Result<Ran, String> {
        let arguments = self.reset(case.tool).await;
        let done_before = self.mock.done().len();
        let held = self.points.hold(&case.at.point, case.at.nth);
        let id = self.server.start_call(case.tool.name(), arguments).await;
        tokio::select! {
            () = held.arrived() => {}
            response = self.server.response(id) => {
                return Err(format!("never reached: {}", response["result"]));
            }
        }
        let changed = Instant::now();
        self.change(case.fact, case.tool).await;
        if case.hold == Hold::Long {
            tokio::time::sleep(PAST_THE_LAST_GATE).await;
        }
        held.release();
        let response = self.server.response(id).await;
        let trace = self.points.trace();
        Ok(Ran {
            result: response["result"].clone(),
            acts: self.acts(case.tool, &trace),
            indexes: self.mock.done()[done_before..].to_vec(),
            changed,
        })
    }
}

/// Whether niri's view is the world's own again.
fn initial(desktop: &Value) -> bool {
    let windows = desktop["windows"].as_array().unwrap();
    let floating = windows.iter().any(|window| window["is_floating"] == true);
    desktop["focused_window"] == WINDOW
        && windows.len() == 3
        && !floating
        && app_id(desktop, WINDOW).as_deref() == Some("org.ncu.Mock")
        && app_id(desktop, TARGET).as_deref() == Some("other")
}

/// The `app_id` of window `id`, if it is open.
fn app_id(desktop: &Value, id: u64) -> Option<String> {
    desktop["windows"]
        .as_array()?
        .iter()
        .find(|window| window["id"] == id)
        .map(|window| window["app_id"].as_str().unwrap_or_default().to_owned())
}

/// What a call came to: an error's name, `invalid_arguments` for a mistake, or the outcome.
fn verdict(result: &Value) -> Result<&Value, String> {
    if result["isError"] != true {
        return Ok(result);
    }
    let name = result["structuredContent"]["error"]
        .as_str()
        .unwrap_or("invalid_arguments");
    Err(name.to_owned())
}

/// Checks the contract on what `case` came to, and says how it broke it.
fn judge(case: &Case<'_>, ran: &Ran) -> Result<(), String> {
    if ran.acts.len() > 1 {
        return Err(format!("acted {} times", ran.acts.len()));
    }
    if ran.indexes.iter().any(|&index| index != 0) {
        return Err(format!(
            "asked for actions {:?}, not the one listed",
            ran.indexes
        ));
    }
    if case.before_send() {
        before_send(case, ran)
    } else {
        after_send(case, ran)
    }
}

/// The change came before the call that acts: refused with the fact's error and nothing
/// acted, or it acted within `LAST_GATE` of the change.
fn before_send(case: &Case<'_>, ran: &Ran) -> Result<(), String> {
    let refusals = case.fact.refusals(case.tool).unwrap_or_default();
    match (verdict(&ran.result), ran.acts.first()) {
        (Err(name), None) => {
            let busy = case.tool.kind() == Kind::Element && name == "element_busy";
            if busy || refusals.contains(&name.as_str()) {
                return Ok(());
            }
            Err(format!("refused with {name}, not {refusals:?}"))
        }
        (Err(name), Some(_)) => Err(format!("refused with {name} after the call acted")),
        (Ok(_), Some(&acted)) => {
            let age = acted.saturating_duration_since(ran.changed);
            if age < LAST_GATE {
                return Ok(());
            }
            Err(format!("acted {} ms after the change", age.as_millis()))
        }
        (Ok(result), None) => Err(format!("neither refused nor acted: {result}")),
    }
}

/// The change came once the call that acts went out: no error, and no focus from before
/// the change reported as the focus afterwards.
fn after_send(case: &Case<'_>, ran: &Ran) -> Result<(), String> {
    if ran.acts.len() != 1 {
        return Err("the call that acts didn't go out".to_owned());
    }
    let result = verdict(&ran.result).map_err(|name| format!("refused with {name}"))?;
    let outcome = &result["structuredContent"];
    if case.fact == Fact::FocusMoved && outcome["focused_window"] == WINDOW {
        return Err(format!("reported focus from before the change: {outcome}"));
    }
    Ok(())
}

/// Explores `tool` against `fact` in a world of its own: every point of the trace, held
/// briefly and, before the call that acts, past `LAST_GATE`. Returns the broken cases.
async fn explore_fact(tool: Tool, fact: Fact) -> (usize, Vec<String>) {
    let tag = format!("st-{}-{fact:?}", tool.name());
    let tag: String = tag.chars().take(24).collect();
    let mut world = World::start(&tag).await;
    let trace = world.baseline(tool).await;
    let sends = trace
        .iter()
        .position(|passed| tool.acts(&passed.point))
        .unwrap_or(trace.len());
    let mut broken = Vec::new();
    let mut runs = 0;
    for (index, at) in trace.iter().enumerate() {
        for hold in [Hold::Short, Hold::Long] {
            let case = Case {
                tool,
                fact,
                at,
                sends,
                index,
                hold,
            };
            if hold == Hold::Long && !case.before_send() {
                continue;
            }
            runs += 1;
            let judged = match world.run(&case).await {
                Ok(ran) => judge(&case, &ran).map_err(|why| format!("{why}; {}", ran.result)),
                Err(why) => Err(why),
            };
            if let Err(why) = judged {
                broken.push(format!(
                    "{} {fact:?} held at {} #{} ({hold:?}): {why}",
                    tool.name(),
                    at.point,
                    at.nth
                ));
            }
        }
    }
    (runs, broken)
}

const TOOLS: [Tool; 8] = [
    Tool::Activate,
    Tool::SetText,
    Tool::FocusWindow,
    Tool::CloseWindow,
    Tool::NiriAction,
    Tool::Launch,
    Tool::Key,
    Tool::TypeText,
];

/// Explores every tool that depends on `fact`, each on a thread and a desktop of its own,
/// and fails with every broken case.
fn explore(fact: Fact) {
    let threads: Vec<_> = TOOLS
        .into_iter()
        .filter(|tool| fact.refusals(*tool).is_some())
        .map(|tool| {
            std::thread::Builder::new()
                .name(format!("{}-{fact:?}", tool.name()))
                .spawn(move || {
                    tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .unwrap()
                        .block_on(explore_fact(tool, fact))
                })
                .unwrap()
        })
        .collect();
    let mut runs = 0;
    let mut broken = Vec::new();
    for thread in threads {
        let (ran, found) = thread
            .join()
            .unwrap_or_else(|panic| std::panic::resume_unwind(panic));
        runs += ran;
        broken.extend(found);
    }
    assert!(
        broken.is_empty(),
        "{} of {runs} cases broke the contract:\n{}",
        broken.len(),
        broken.join("\n")
    );
}

#[test]
fn focus_moving_while_a_reply_is_held() {
    explore(Fact::FocusMoved);
}

#[test]
fn the_window_closing_while_a_reply_is_held() {
    explore(Fact::WindowClosed);
}

#[test]
fn a_marker_set_while_a_reply_is_held() {
    explore(Fact::InputDirty);
}

#[test]
fn the_window_becoming_denied_while_a_reply_is_held() {
    explore(Fact::DenyListed);
}

#[test]
fn the_element_becoming_a_password_field_while_a_reply_is_held() {
    explore(Fact::BecamePassword);
}

#[test]
fn the_element_reused_for_another_record_while_a_reply_is_held() {
    explore(Fact::Renamed);
}

#[test]
fn the_elements_parent_reused_while_a_reply_is_held() {
    explore(Fact::AncestorRenamed);
}

#[test]
fn the_element_moving_to_another_window_while_a_reply_is_held() {
    explore(Fact::MovedAway);
}

#[test]
fn the_elements_actions_reordered_while_a_reply_is_held() {
    explore(Fact::ActionsReordered);
}

#[test]
fn the_element_removed_while_a_reply_is_held() {
    explore(Fact::Removed);
}
