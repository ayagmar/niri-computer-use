//! The stall-point explorer. Every review of the element actions found one shape of bug:
//! hold one reply back, from niri, Noctalia or the app, change one fact the gate had
//! already read, release, and the action went out on a stale answer. The explorer searches
//! for that shape instead of waiting for a review. It calls each acting tool and keeps
//! the trace of the hold points the fakes passed, every reply the call awaited, so a
//! new await in the server is explored without new test code. Then, for each point of the
//! trace and each fact the tool's gates depend on, it calls the tool again, holds that
//! reply, changes the fact, waits for the server to see the change, releases the reply, a
//! moment later or past `LAST_GATE`, and checks the contract on what the fakes received:
//!
//! - Every call that acts after the change went out within `LAST_GATE` of it, the window in
//!   which the gates document that a change can be missed, and no more went out than with
//!   nothing held.
//! - Before the first call that acts went out, the change refuses the call with the fact's
//!   error, or the call went out within that window.
//! - After it went out, the call isn't refused: a stop or a lost lease makes it `uncertain`,
//!   `accepted` null until the reply came and true after it, with nothing more asked and no
//!   screenshot; any other change leaves an outcome that doesn't report focus from before
//!   the change. With more calls to come, `type_text`'s strokes, a later one may be refused
//!   with the fact's error, saying what went out before it.
//!
//! The keyboard tools act through the fake `wtype`, whose point `wtype` is a stroke read and
//! going out. The pointer and `paste` act through Wayland only in nested runs (see
//! docs/decisions.md).

use std::collections::BTreeSet;
use std::fs::File;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use niri_ipc::Action;
use serde_json::{Value, json};
use tokio::sync::Semaphore;

use crate::atspi::{self, Bus, Mock, Object, PASSWORD_TEXT};
use crate::client::{Server, WAIT};
use crate::elements::{WINDOW, app_window, objects, workspace};
use crate::fixture::{Fixture, jpeg};
use crate::holds::{Passed, Points};
use crate::niri::{Niri, Stream, window_on};
use crate::noctalia::{self, LOCKED, UNLOCKED};
use crate::session::{self, NiriProcess};

/// The server's bound on the age of the answers a call that acts relies on.
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
/// Ten characters, of which `type_text` types 150 and submits them: two `wtype` calls for
/// the text, at most 100 characters each, and one for `Return`.
const TEXT: &str = "abcdefghij";
/// The Noctalia panel the shell tools open and close.
const PANEL: &str = "wallpaper";

/// The acting tools the explorer calls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tool {
    Activate,
    SetText,
    Focus,
    CloseWindow,
    NiriAction,
    Launch,
    Key,
    TypeText,
    ShellOpen,
    ShellClose,
    Noctalia,
}

/// How a tool's call reaches the desktop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Element,
    Niri,
    Keyboard,
    Shell,
}

impl Tool {
    const fn name(self) -> &'static str {
        match self {
            Self::Activate => "activate_element",
            Self::SetText => "set_element_text",
            Self::Focus => "focus",
            Self::CloseWindow => "close_window",
            Self::NiriAction => "niri_action",
            Self::Launch => "launch",
            Self::Key => "key",
            Self::TypeText => "type_text",
            Self::ShellOpen | Self::ShellClose => "shell_panel",
            Self::Noctalia => "noctalia",
        }
    }

    /// The tool's name in what the explorer reports, telling `shell_panel`'s two calls
    /// apart.
    fn label(self) -> String {
        format!("{self:?}")
    }

    const fn kind(self) -> Kind {
        match self {
            Self::Activate | Self::SetText => Kind::Element,
            Self::Focus | Self::CloseWindow | Self::NiriAction | Self::Launch => Kind::Niri,
            Self::Key | Self::TypeText => Kind::Keyboard,
            Self::ShellOpen | Self::ShellClose | Self::Noctalia => Kind::Shell,
        }
    }

    /// The element an element action acts on: its name and path.
    const fn element(self) -> (&'static str, &'static str) {
        match self {
            Self::SetText => ("Field", "/entry"),
            Self::Activate
            | Self::Focus
            | Self::CloseWindow
            | Self::NiriAction
            | Self::Launch
            | Self::Key
            | Self::TypeText
            | Self::ShellOpen
            | Self::ShellClose
            | Self::Noctalia => ("Safe", "/button"),
        }
    }

    /// Whether the hold point `point` is a call that acts reaching niri, the app or
    /// Noctalia, or `wtype` having read a stroke.
    fn acts(self, point: &str) -> bool {
        match self.kind() {
            Kind::Element if self == Self::Activate => point.starts_with("atspi:DoAction:"),
            Kind::Element => point.starts_with("atspi:SetTextContents:"),
            Kind::Niri => point.starts_with("niri:Action:"),
            Kind::Keyboard => point == "wtype",
            Kind::Shell if self == Self::ShellClose => point == "noctalia:panel-close",
            Kind::Shell => point == "noctalia:panel-open",
        }
    }

    /// The arguments, with an element action's ref `element`.
    fn arguments(self, element: Option<&str>) -> Value {
        let expect = json!({"window_id": WINDOW});
        match self {
            Self::Activate => json!({"element": element, "expect": expect}),
            Self::SetText => json!({"element": element, "text": "abc", "expect": expect}),
            Self::Focus => json!({"window_id": TARGET}),
            Self::CloseWindow => json!({"id": TARGET}),
            Self::NiriAction => json!({"action": {"ToggleWindowFloating": {"id": TARGET}}}),
            Self::Launch => json!({"preset": "foot"}),
            Self::Key => json!({"keys": ["Down"], "expect": expect}),
            Self::TypeText => json!({"text": TEXT.repeat(15), "submit": true, "expect": expect}),
            Self::ShellOpen => json!({"panel": PANEL, "open": true}),
            Self::ShellClose => json!({"panel": PANEL, "open": false}),
            Self::Noctalia => json!({"args": ["panel-open", PANEL]}),
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
    /// Noctalia locks the screen.
    ScreenLocked,
    /// The user sets the stop flag.
    Stopped,
    /// The lease file is replaced and another server locks the new one.
    LeaseTaken,
    /// The window takes an `app_id` the policy denies. The policy is read once per session,
    /// so this is how a denial can appear while a call runs.
    DenyListed,
    /// The element becomes a password field.
    BecamePassword,
    /// The element is reused in place for another record.
    Renamed,
    /// The label that binds `Safe` to its record is, as a list reusing the row's objects
    /// renames it.
    LabelRenamed,
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
            Self::ScreenLocked => Some(&["screen_locked"]),
            Self::Stopped => Some(&["stopped"]),
            Self::LeaseTaken => Some(&["lease_required"]),
            Self::FocusMoved => {
                matches!(kind, Kind::Element | Kind::Keyboard).then_some(&["focus_mismatch"])
            }
            Self::WindowClosed => match kind {
                Kind::Element => Some(&["element_stale"]),
                Kind::Keyboard => Some(&["focus_mismatch"]),
                Kind::Niri => (tool != Tool::Launch).then_some(&["invalid_arguments"]),
                Kind::Shell => None,
            },
            Self::DenyListed => {
                let input = matches!(kind, Kind::Element | Kind::Keyboard);
                (input || tool == Tool::CloseWindow).then_some(&["app_denied"])
            }
            Self::BecamePassword => element.then_some(&["secret_field"]),
            Self::Renamed | Self::AncestorRenamed | Self::MovedAway | Self::Removed => {
                element.then_some(&["element_stale"])
            }
            Self::ActionsReordered | Self::LabelRenamed => {
                (tool == Tool::Activate).then_some(&["element_stale"])
            }
        }
    }

    /// What `detail` says when the change cut a call short after it went out, for the
    /// changes that cut a running call short at once instead of being found by the call's
    /// own checks.
    const fn cut(self) -> Option<&'static str> {
        match self {
            Self::Stopped => Some("the user's stop flag"),
            Self::LeaseTaken => Some("the lease file was removed or replaced"),
            Self::FocusMoved
            | Self::WindowClosed
            | Self::InputDirty
            | Self::ScreenLocked
            | Self::DenyListed
            | Self::BecamePassword
            | Self::Renamed
            | Self::LabelRenamed
            | Self::AncestorRenamed
            | Self::MovedAway
            | Self::ActionsReordered
            | Self::Removed => None,
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
    /// Where the first call that acts is in the trace.
    sends: usize,
    /// Where the last is.
    last: usize,
    /// How many calls act with nothing held.
    acts: usize,
    index: usize,
    hold: Hold,
}

impl Case<'_> {
    const fn before_send(&self) -> bool {
        self.index < self.sends
    }

    /// Whether a call that acts comes after the point held.
    const fn acts_later(&self) -> bool {
        self.index < self.last
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
    /// The points passed after the change.
    after: Vec<String>,
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
/// `DENIED` and has the preset `foot`, with `unrestricted` on. The server holds the lease.
/// Exploring the lock in the full matrix,
/// niri runs in a logind session and logind's answer, from `loginctl`, is a point too.
///
/// Fields drop in this order: the server first, so it can't react to the rest going away,
/// and the fixture last.
struct World {
    server: Server,
    _niri: Niri,
    /// The niri in a logind session that relays to the fake niri, exploring the lock.
    _relay: Option<NiriProcess>,
    stream: Stream,
    noctalia: noctalia::Reply,
    _bus: Bus,
    mock: Mock,
    points: Points,
    closed: Closed,
    /// The mock's objects as they were before the last change.
    saved: Vec<(&'static str, Object)>,
    /// The lease file another server locked, while it holds it.
    taken: Option<File>,
    fixture: Fixture,
}

impl World {
    async fn start(name: &str, fact: Fact) -> Self {
        let mut fixture = Fixture::new(name);
        fixture.set("NIRI_COMPUTER_USE_KEYBOARD", "wtype");
        fixture.set("NIRI_COMPUTER_USE_UNRESTRICTED", "1");
        std::fs::create_dir_all(fixture.path("config/niri-computer-use")).unwrap();
        std::fs::write(fixture.path("config/niri-computer-use/policy.toml"), POLICY).unwrap();
        fixture.program("noctalia", "exit 0");
        // The point once it read the stroke, which then goes out as wtype exits.
        fixture.program("wtype", "cat > /dev/null\npass wtype");
        fixture.grim(&jpeg(1280, 720, b"evidence"));
        let mut objects = objects();
        // `Safe` is bound to its record through a label, so every call on it reads the
        // label's relations and name too.
        for (path, object) in &mut objects {
            match *path {
                "/button" => object.labelled_by = vec!["/caption".to_owned()],
                "/panel" => object.children.push("/caption".to_owned()),
                _ => {}
            }
        }
        let caption = Object {
            label_for: vec!["/button".to_owned()],
            ..Object::label("Record 1")
        };
        objects.extend([
            ("/caption", caption),
            ("/frame2", Object::frame("Other", (640, 480), &["/panel2"])),
            ("/panel2", Object::panel(&[])),
        ]);
        let (bus, mock) = atspi::start(&mut fixture, &["/frame", "/frame2"], objects).await;
        let noctalia = noctalia::start(&fixture, UNLOCKED);
        let backend = fixture.path("run/backend.sock");
        // logind's answer for every tool costs `make check` 5 s on 4 cores, where
        // `shell::a_lock_while_loginds_answer_is_held_refuses_the_call` holds it instead.
        let session = fact == Fact::ScreenLocked && full();
        let mut niri = if session {
            Niri::behind(&fixture, &backend)
        } else {
            Niri::start(&fixture)
        };
        niri.set_windows(&windows(), &[workspace()]);
        let closed = Closed::default();
        let closing = Arc::clone(&closed);
        niri.effects(move |action| effects(action, &closing));
        fixture.serve_programs();
        let relay = if session {
            session::held_loginctl(&fixture);
            Some(NiriProcess::in_session(&fixture, &backend).await)
        } else {
            None
        };
        let mut server = Server::start(&fixture).await;
        let stream = niri.stream().await;
        stream.initial(&windows());
        server.structured("acquire_desktop").await;
        Self {
            server,
            _niri: niri,
            _relay: relay,
            stream,
            noctalia,
            _bus: bus,
            mock,
            points: fixture.points(),
            closed,
            saved: Vec::new(),
            taken: None,
            fixture,
        }
    }

    /// Puts every fact back as it was, takes the lease again if it was lost, and returns
    /// `tool`'s arguments with a fresh element ref. The trace starts afterwards.
    async fn reset(&mut self, tool: Tool) -> Value {
        self.points.idle().await;
        let runtime = self.fixture.runtime_dir();
        for flag in ["stop", "input-dirty"] {
            std::fs::remove_file(runtime.join(flag)).ok();
        }
        self.noctalia.set(UNLOCKED);
        self.noctalia
            .open_panel((tool == Tool::ShellClose).then_some(PANEL));
        std::fs::remove_file(self.fixture.path("locked")).ok();
        self.taken = None;
        self.closed.lock().unwrap().clear();
        for (path, object) in self.saved.drain(..) {
            self.mock.change(path, |now| *now = object);
        }
        self.stream
            .send(&json!({"WindowsChanged": {"windows": windows()}}));
        self.until_desktop(initial).await;
        self.acquire().await;
        let element = if tool.kind() == Kind::Element {
            Some(self.element(tool.element().0).await)
        } else {
            None
        };
        self.points.start();
        tool.arguments(element.as_deref())
    }

    /// Takes the lease, once the server has seen the stop flag cleared.
    async fn acquire(&mut self) {
        let start = Instant::now();
        loop {
            let result = self.server.call("acquire_desktop", json!({})).await;
            if result["isError"] == false {
                return;
            }
            assert!(start.elapsed() < WAIT, "{result}");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
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
        let runtime = self.fixture.runtime_dir();
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
            Fact::InputDirty => std::fs::write(runtime.join("input-dirty"), "").unwrap(),
            Fact::ScreenLocked => {
                self.noctalia.set(LOCKED);
                std::fs::write(self.fixture.path("locked"), "").unwrap();
            }
            Fact::Stopped => std::fs::write(runtime.join("stop"), "").unwrap(),
            Fact::LeaseTaken => {
                let lease = runtime.join("lease");
                std::fs::remove_file(&lease).unwrap();
                let file = File::create(&lease).unwrap();
                file.try_lock().unwrap();
                self.taken = Some(file);
            }
            Fact::BecamePassword => self.change_object(path, |object| object.role = PASSWORD_TEXT),
            Fact::Renamed => self.change_object(path, |object| {
                object.name = "Different record".to_owned();
            }),
            Fact::LabelRenamed => self.change_object("/caption", |object| {
                object.name = "Another record".to_owned();
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

    /// Calls `tool` with nothing held until two calls in a row pass the same points, and
    /// returns that trace. Their order can differ, as the readiness report asks niri and
    /// Noctalia at once. On a loaded machine a pass of the element actions' last gate can run
    /// over and be done again, which adds points the next calls don't pass.
    async fn baseline(&mut self, tool: Tool) -> Vec<Passed> {
        let points = |trace: &[Passed]| -> BTreeSet<(String, usize)> {
            trace
                .iter()
                .map(|passed| (passed.point.clone(), passed.nth))
                .collect()
        };
        let mut last = self.trace(tool).await;
        for _ in 0..5 {
            let trace = self.trace(tool).await;
            if points(&trace) == points(&last) {
                return trace;
            }
            last = trace;
        }
        panic!(
            "{}: no two calls in a row passed the same points",
            tool.label()
        );
    }

    /// Calls `tool` once with nothing held, and returns the trace of its call.
    async fn trace(&mut self, tool: Tool) -> Vec<Passed> {
        let arguments = self.reset(tool).await;
        let result = self.server.call(tool.name(), arguments).await;
        let trace = self.points.trace();
        assert_eq!(result["isError"], false, "{}: {result}", tool.label());
        let strokes = if tool == Tool::TypeText { 3 } else { 1 };
        assert_eq!(
            acts(tool, &trace).len(),
            strokes,
            "{}: {trace:?}",
            tool.label()
        );
        trace
    }

    /// Runs `case` up to three times until its point is reached. A lock read made again
    /// because the last one had aged is in the trace only when the machine was that slow,
    /// so on a loaded machine the next call may not make it.
    async fn reach(&mut self, case: &Case<'_>) -> Result<Ran, String> {
        let mut ran = self.run(case).await;
        for _ in 0..2 {
            if ran.is_ok() {
                break;
            }
            ran = self.run(case).await;
        }
        ran
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
        // A cut answers the call without waiting for the held reply.
        let early = if case.fact.cut().is_some() {
            tokio::time::timeout(PAST_THE_LAST_GATE, self.server.response(id))
                .await
                .ok()
        } else {
            None
        };
        held.release();
        let response = match early {
            Some(response) => response,
            None => self.server.response(id).await,
        };
        let trace = self.points.trace();
        Ok(Ran {
            result: response["result"].clone(),
            acts: acts(case.tool, &trace),
            indexes: self.mock.done()[done_before..].to_vec(),
            changed,
            after: trace
                .iter()
                .filter(|passed| passed.at > changed)
                .map(|passed| passed.point.clone())
                .collect(),
        })
    }
}

/// When each call that acts in `trace` reached the fakes.
fn acts(tool: Tool, trace: &[Passed]) -> Vec<Instant> {
    trace
        .iter()
        .filter(|passed| tool.acts(&passed.point))
        .map(|passed| passed.at)
        .collect()
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
    if ran.acts.len() > case.acts {
        return Err(format!("acted {} times, not {}", ran.acts.len(), case.acts));
    }
    let late = ran
        .acts
        .iter()
        .map(|acted| acted.saturating_duration_since(ran.changed))
        .find(|age| *age >= LAST_GATE);
    if let Some(age) = late {
        return Err(format!("acted {} ms after the change", age.as_millis()));
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

/// The change came before the first call that acts: refused with the fact's error and
/// nothing acted, or it acted within `LAST_GATE` of the change, which `judge` checked.
fn before_send(case: &Case<'_>, ran: &Ran) -> Result<(), String> {
    match (verdict(&ran.result), ran.acts.is_empty()) {
        (Err(name), true) => {
            let busy = case.tool.kind() == Kind::Element && name == "element_busy";
            if busy {
                return Ok(());
            }
            refused_with_the_facts_error(case, &name)
        }
        (Err(name), false) => refused_partway(case, ran, &name),
        (Ok(_), false) => Ok(()),
        (Ok(result), true) => Err(format!("neither refused nor acted: {result}")),
    }
}

fn refused_with_the_facts_error(case: &Case<'_>, name: &str) -> Result<(), String> {
    let refusals = case.fact.refusals(case.tool).unwrap_or_default();
    if refusals.contains(&name) {
        return Ok(());
    }
    Err(format!("refused with {name}, not {refusals:?}"))
}

/// A call that acts went out, and a later one was refused: with the fact's error, saying
/// what went out before it, and never for a stop or a lost lease, which leave the call
/// `uncertain`.
fn refused_partway(case: &Case<'_>, ran: &Ran, name: &str) -> Result<(), String> {
    let detail = ran.result["structuredContent"]["detail"]
        .as_str()
        .unwrap_or_default();
    if case.fact.cut().is_some() || !detail.contains("; then ") {
        return Err(format!("refused with {name} after a call acted: {detail}"));
    }
    refused_with_the_facts_error(case, name)
}

/// The change came once the call that acts went out: no error. A stop or a lost lease
/// leaves it `uncertain`, accepted only once its reply came, with nothing more asked; any
/// other change leaves no focus from before the change reported as the focus afterwards.
fn after_send(case: &Case<'_>, ran: &Ran) -> Result<(), String> {
    if ran.acts.is_empty() {
        return Err("the call that acts didn't go out".to_owned());
    }
    let result = match verdict(&ran.result) {
        Ok(result) => result,
        Err(name) => return refused_partway(case, ran, &name),
    };
    let outcome = &result["structuredContent"];
    if case.fact == Fact::FocusMoved && outcome["focused_window"] == WINDOW {
        return Err(format!("reported focus from before the change: {outcome}"));
    }
    let Some(why) = case.fact.cut() else {
        return Ok(());
    };
    let replied = case.index > case.sends;
    let detail = outcome["detail"].as_str().unwrap_or_default();
    let accepted = if replied { json!(true) } else { Value::Null };
    if outcome["observed"] != "uncertain"
        || outcome["accepted"] != accepted
        || !detail.contains(why)
    {
        return Err(format!("not uncertain with accepted {accepted}: {outcome}"));
    }
    if result["content"].as_array().map(Vec::len) != Some(1) {
        return Err("took a screenshot after the cut".to_owned());
    }
    if !ran.after.is_empty() {
        return Err(format!("asked {:?} after the cut", ran.after));
    }
    Ok(())
}

/// How many worlds explore each tool against each fact, each taking every `SHARDS`th
/// point of the trace. A case mostly waits for the server and its holds, so worlds side by
/// side finish sooner even on few cores.
const SHARDS: usize = 8;
/// The most worlds exploring at once, across the fact tests libtest runs side by side. With
/// a world for every shard, a 4-core machine got so busy that the element actions' last
/// gate ran over twice on a call with nothing held.
static WORLDS: Semaphore = Semaphore::const_new(16);

/// Explores `tool` against `fact` in a world of its own: every `SHARDS`th point of the
/// trace from `shard` on, held briefly and, before the call that acts, past `LAST_GATE`.
/// Returns the broken cases.
async fn explore_fact(tool: Tool, fact: Fact, shard: usize) -> (usize, Vec<String>) {
    let tag = format!("st{shard}-{}-{fact:?}", tool.label());
    let tag: String = tag.chars().take(24).collect();
    let _turn = WORLDS.acquire().await.unwrap();
    let mut world = World::start(&tag, fact).await;
    let trace = world.baseline(tool).await;
    let acting: Vec<usize> = (0..trace.len())
        .filter(|&index| tool.acts(&trace[index].point))
        .collect();
    let (Some(&sends), Some(&last)) = (acting.first(), acting.last()) else {
        panic!("{}: nothing acted in {trace:?}", tool.label());
    };
    let mut broken = Vec::new();
    let mut runs = 0;
    for (index, at) in trace.iter().enumerate().skip(shard).step_by(SHARDS) {
        for hold in [Hold::Short, Hold::Long] {
            let case = Case {
                tool,
                fact,
                at,
                sends,
                last,
                acts: acting.len(),
                index,
                hold,
            };
            if hold == Hold::Long && !case.acts_later() {
                continue;
            }
            runs += 1;
            let judged = match world.reach(&case).await {
                Ok(ran) => judge(&case, &ran).map_err(|why| format!("{why}; {}", ran.result)),
                Err(why) => Err(why),
            };
            if let Err(why) = judged {
                broken.push(format!(
                    "{} {fact:?} held at {} #{} ({hold:?}): {why}",
                    tool.label(),
                    at.point,
                    at.nth
                ));
            }
        }
    }
    (runs, broken)
}

const TOOLS: [Tool; 11] = [
    Tool::Activate,
    Tool::SetText,
    Tool::Focus,
    Tool::CloseWindow,
    Tool::NiriAction,
    Tool::Launch,
    Tool::Key,
    Tool::TypeText,
    Tool::ShellOpen,
    Tool::ShellClose,
    Tool::Noctalia,
];

/// The tools `make check` explores, one of each kind. They catch every bug the explorer
/// found but one between `type_text`'s strokes; `NCU_STALL=full` explores all of `TOOLS`,
/// as `make stall` does.
const SAMPLE: [Tool; 4] = [Tool::Activate, Tool::Focus, Tool::Key, Tool::ShellOpen];

/// Whether `NCU_STALL=full` asks for the full matrix.
fn full() -> bool {
    std::env::var_os("NCU_STALL").is_some_and(|scope| scope == "full")
}

fn tools() -> &'static [Tool] {
    if full() { &TOOLS } else { &SAMPLE }
}

/// Explores every tool that depends on `fact`, each shard on a thread and a desktop of its
/// own, and fails with every broken case.
fn explore(fact: Fact) {
    let threads: Vec<_> = tools()
        .iter()
        .copied()
        .filter(|tool| fact.refusals(*tool).is_some())
        .flat_map(|tool| (0..SHARDS).map(move |shard| (tool, shard)))
        .map(|(tool, shard)| {
            std::thread::Builder::new()
                .name(format!("{}-{fact:?}-{shard}", tool.label()))
                .spawn(move || {
                    tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .unwrap()
                        .block_on(explore_fact(tool, fact, shard))
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
fn the_screen_locking_while_a_reply_is_held() {
    explore(Fact::ScreenLocked);
}

#[test]
fn a_stop_while_a_reply_is_held() {
    explore(Fact::Stopped);
}

#[test]
fn the_lease_taken_while_a_reply_is_held() {
    explore(Fact::LeaseTaken);
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
fn the_elements_label_reused_while_a_reply_is_held() {
    explore(Fact::LabelRenamed);
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
