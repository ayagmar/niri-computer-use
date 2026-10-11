//! A private D-Bus daemon with no service directories, serving as both the session bus and
//! the accessibility bus, and a mock application on it that answers AT-SPI the way a
//! toolkit does. The test changes the mock's objects, and holds, delays or fails a method's
//! next reply, to check the element tools against an application without any desktop's
//! accessibility bus. Each reply, the `Name` property's included, passes the fixture's
//! hold point `atspi:<member>:<path>`. Every name and text is synthetic.

use std::collections::BTreeMap;
use std::process::Stdio;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt as _, BufReader};
use tokio::process::{Child, Command};
use zbus::message::{Header, Message};
use zbus::names::ErrorName;
use zbus::zvariant::OwnedObjectPath;
use zbus::{Connection, fdo, interface};

use crate::client::WAIT;
use crate::fixture::{Fixture, SPAWNING};
use crate::holds::{Held, Points};

pub(crate) const ROOT: &str = "/org/a11y/atspi/accessible/root";
pub(crate) const FRAME: u32 = 23;
pub(crate) const LABEL: u32 = 29;
pub(crate) const PANEL: u32 = 39;
pub(crate) const PASSWORD_TEXT: u32 = 40;
pub(crate) const BUTTON: u32 = 43;
pub(crate) const TEXT: u32 = 61;
const APPLICATION: u32 = 75;
/// `ATSPI_COORD_TYPE_WINDOW`, the only coordinates the server asks for.
const WINDOW_COORDS: u32 = 1;
/// `enabled`, `sensitive`, `showing` and `visible`.
pub(crate) const SHOWING: u32 = (1 << 8) | (1 << 24) | (1 << 25) | (1 << 30);
pub(crate) const EDITABLE: u32 = 1 << 7;

/// One accessible object.
#[derive(Debug, Clone, Default)]
pub(crate) struct Object {
    pub(crate) role: u32,
    pub(crate) name: String,
    pub(crate) states: u32,
    pub(crate) children: Vec<String>,
    /// With a Component interface: its WINDOW extents.
    pub(crate) extents: Option<(i32, i32, i32, i32)>,
    /// With an Action interface: its actions' names.
    pub(crate) actions: Vec<String>,
    /// With `Text` and `EditableText` interfaces: how many characters it holds.
    pub(crate) text: Option<i32>,
    /// A counter button's count, which each `DoAction` adds one to and names it after.
    pub(crate) counter: Option<u32>,
    /// The objects each `DoAction` shows when hidden and hides when shown.
    pub(crate) toggles: Vec<String>,
}

impl Object {
    pub(crate) fn frame(name: &str, size: (i32, i32), children: &[&str]) -> Self {
        Self {
            role: FRAME,
            name: name.to_owned(),
            states: SHOWING,
            children: children.iter().map(|&child| child.to_owned()).collect(),
            extents: Some((0, 0, size.0, size.1)),
            ..Self::default()
        }
    }

    pub(crate) fn panel(children: &[&str]) -> Self {
        Self {
            role: PANEL,
            states: SHOWING,
            children: children.iter().map(|&child| child.to_owned()).collect(),
            extents: Some((0, 0, 100, 100)),
            ..Self::default()
        }
    }

    pub(crate) fn button(name: &str, actions: &[&str]) -> Self {
        Self {
            role: BUTTON,
            name: name.to_owned(),
            states: SHOWING,
            extents: Some((10, 10, 80, 30)),
            actions: actions.iter().map(|&action| action.to_owned()).collect(),
            ..Self::default()
        }
    }

    pub(crate) fn entry(role: u32, name: &str) -> Self {
        Self {
            role,
            name: name.to_owned(),
            states: SHOWING | EDITABLE,
            extents: Some((10, 50, 200, 30)),
            actions: vec!["activate".to_owned()],
            text: Some(0),
            ..Self::default()
        }
    }

    fn interfaces(&self) -> Vec<String> {
        let mut interfaces = vec!["org.a11y.atspi.Accessible"];
        if self.extents.is_some() {
            interfaces.push("org.a11y.atspi.Component");
        }
        if !self.actions.is_empty() {
            interfaces.push("org.a11y.atspi.Action");
        }
        if self.text.is_some() {
            interfaces.extend(["org.a11y.atspi.Text", "org.a11y.atspi.EditableText"]);
        }
        interfaces.into_iter().map(str::to_owned).collect()
    }
}

/// What a method's next call gets instead of its answer.
#[derive(Debug)]
enum Next {
    /// Its reply comes this much later.
    Delay(Duration),
    /// `org.freedesktop.DBus.Error.Failed` with this message.
    Fail(String),
    /// An error whose name the application made up.
    FailNamed(&'static str),
}

/// What a method answers with instead of its reply: one of D-Bus's standard errors, or one
/// whose name an application made up, which may hold anything a name can.
#[derive(Debug)]
pub(crate) enum Refusal {
    Standard(fdo::Error),
    Named(&'static str),
}

impl From<fdo::Error> for Refusal {
    fn from(error: fdo::Error) -> Self {
        Self::Standard(error)
    }
}

/// A property's getter can only answer D-Bus's standard errors.
impl From<Refusal> for fdo::Error {
    fn from(refusal: Refusal) -> Self {
        match refusal {
            Refusal::Standard(error) => error,
            Refusal::Named(_) => Self::Failed("refused".to_owned()),
        }
    }
}

impl From<zbus::Error> for Refusal {
    fn from(error: zbus::Error) -> Self {
        Self::Standard(error.into())
    }
}

impl zbus::DBusError for Refusal {
    fn create_reply(&self, call: &Header<'_>) -> zbus::Result<Message> {
        match self {
            Self::Standard(error) => error.create_reply(call),
            Self::Named(_) => Message::error(call, self.name())?.build(&("refused",)),
        }
    }

    fn name(&self) -> ErrorName<'_> {
        match self {
            Self::Standard(error) => error.name(),
            Self::Named(name) => ErrorName::from_static_str_unchecked(name),
        }
    }

    fn description(&self) -> Option<&str> {
        match self {
            Self::Standard(error) => error.description(),
            Self::Named(_) => Some("refused"),
        }
    }
}

/// A method's reply, or why not. Named `Result` so zbus's `interface` sees an error type.
mod answer {
    pub(crate) type Result<T> = std::result::Result<T, super::Refusal>;
}

#[derive(Debug, Default)]
struct State {
    objects: BTreeMap<String, Object>,
    /// The application's unique name on the bus.
    app: String,
    /// By method, and the object's path when only that object's call is meant.
    next: BTreeMap<(&'static str, Option<String>), Next>,
    /// How much later every call of a method on an object is answered.
    slow: BTreeMap<(&'static str, String), Duration>,
    calls: BTreeMap<&'static str, usize>,
    /// The index each `DoAction` asked for, in the order they arrived.
    done: Vec<i32>,
}

/// The mock application, which the test changes.
#[derive(Debug, Clone, Default)]
pub(crate) struct Mock {
    state: Arc<Mutex<State>>,
    points: Points,
}

impl Mock {
    /// Holds the next call of `member` on the object at `path` until the returned handle
    /// releases it.
    pub(crate) fn hold_on(&self, member: &'static str, path: &str) -> Held {
        self.points.hold_next(&point(member, path))
    }

    /// Answers the next call of `member` after `delay`.
    pub(crate) fn delay(&self, member: &'static str, delay: Duration) {
        self.set_next((member, None), Next::Delay(delay));
    }

    /// Answers every call of `member` on the object at `path` `delay` later, from now on.
    pub(crate) fn slow_on(&self, member: &'static str, path: &str, delay: Duration) {
        let mut state = self.state.lock().unwrap();
        state.slow.insert((member, path.to_owned()), delay);
    }

    /// Fails the next call of `member` with `message`.
    pub(crate) fn fail(&self, member: &'static str, message: &str) {
        self.set_next((member, None), Next::Fail(message.to_owned()));
    }

    /// Fails the next call of `member` on the object at `path` with an error named `name`,
    /// a name the application made up.
    pub(crate) fn fail_named(&self, member: &'static str, path: &str, name: &'static str) {
        self.set_next((member, Some(path.to_owned())), Next::FailNamed(name));
    }

    /// The index each `DoAction` asked for, in order.
    pub(crate) fn done(&self) -> Vec<i32> {
        self.state.lock().unwrap().done.clone()
    }

    /// How many calls of `member` arrived.
    pub(crate) fn calls(&self, member: &str) -> usize {
        self.state
            .lock()
            .unwrap()
            .calls
            .get(member)
            .copied()
            .unwrap_or(0)
    }

    /// Changes the object at `path`.
    pub(crate) fn change(&self, path: &str, change: impl FnOnce(&mut Object)) {
        change(self.state.lock().unwrap().objects.get_mut(path).unwrap());
    }

    /// The object at `path` as it is now.
    pub(crate) fn object(&self, path: &str) -> Object {
        self.state.lock().unwrap().objects[path].clone()
    }

    fn set_next(&self, at: (&'static str, Option<String>), next: Next) {
        self.state.lock().unwrap().next.insert(at, next);
    }

    /// Counts a call of `member` on the object at `path` and takes what the test set for
    /// it: for that object first, else for any; and how slow its calls are.
    fn count(&self, member: &'static str, path: &str) -> (Option<Next>, Duration) {
        let mut state = self.state.lock().unwrap();
        *state.calls.entry(member).or_default() += 1;
        let next = state.next.remove(&(member, Some(path.to_owned())));
        let next = next.or_else(|| state.next.remove(&(member, None)));
        let slow = state.slow.get(&(member, path.to_owned())).copied();
        drop(state);
        (next, slow.unwrap_or_default())
    }

    /// Counts a call of `member` on the object at `path`, passes its hold point, and applies
    /// what the test set for it: for that object first, else for any.
    async fn called(&self, member: &'static str, path: &str) -> answer::Result<()> {
        let (next, slow) = self.count(member, path);
        let _busy = self.points.pass(point(member, path)).await;
        tokio::time::sleep(slow).await;
        match next {
            None => {}
            Some(Next::Delay(delay)) => tokio::time::sleep(delay).await,
            Some(Next::Fail(message)) => return Err(fdo::Error::Failed(message).into()),
            Some(Next::FailNamed(name)) => return Err(Refusal::Named(name)),
        }
        Ok(())
    }

    fn read<T>(&self, path: &str, read: impl FnOnce(&Object, &str) -> T) -> fdo::Result<T> {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state
            .objects
            .get(path)
            .map(|object| read(object, &state.app))
            .ok_or_else(|| fdo::Error::UnknownObject(format!("no object at {path}")))
    }
}

/// The hold point of a call of `member` on the object at `path`.
fn point(member: &str, path: &str) -> String {
    format!("atspi:{member}:{path}")
}

/// The daemon and the mock's connections. Dropping it kills the daemon.
#[derive(Debug)]
pub(crate) struct Bus {
    daemon: Child,
    _registry: Connection,
    _app: Connection,
}

impl Bus {
    /// Kills the daemon, which ends every connection to it.
    pub(crate) async fn kill(&mut self) {
        self.daemon.kill().await.unwrap();
    }
}

/// Starts a daemon in the fixture's directory, makes it the server's session bus, and puts
/// the mock application on it with `objects` under its root, whose children are `frames`.
pub(crate) async fn start(
    fixture: &mut Fixture,
    frames: &[&str],
    objects: Vec<(&str, Object)>,
) -> (Bus, Mock) {
    let (daemon, address) = daemon(fixture).await;
    fixture.set("DBUS_SESSION_BUS_ADDRESS", &address);
    let mock = Mock {
        state: Arc::default(),
        points: fixture.points(),
    };
    let app = connect(&address).await;
    let unique = app.unique_name().unwrap().to_string();
    let root = Object {
        role: APPLICATION,
        name: "mock".to_owned(),
        children: frames.iter().map(|&frame| frame.to_owned()).collect(),
        ..Object::default()
    };
    {
        let mut state = mock.state.lock().unwrap();
        state.app.clone_from(&unique);
        state.objects.insert(ROOT.to_owned(), root);
        for (path, object) in objects {
            state.objects.insert(path.to_owned(), object);
        }
    }
    let paths: Vec<(String, Object)> = mock
        .state
        .lock()
        .unwrap()
        .objects
        .clone()
        .into_iter()
        .collect();
    for (path, object) in paths {
        serve(&app, &mock, &path, &object).await;
    }
    let registry = connect(&address).await;
    let server = registry.object_server();
    let apps = vec![(unique, OwnedObjectPath::try_from(ROOT).unwrap())];
    server.at(ROOT, Registry { apps }).await.unwrap();
    server
        .at("/org/a11y/bus", A11yBus { address })
        .await
        .unwrap();
    registry
        .request_name("org.a11y.atspi.Registry")
        .await
        .unwrap();
    registry.request_name("org.a11y.Bus").await.unwrap();
    let bus = Bus {
        daemon,
        _registry: registry,
        _app: app,
    };
    (bus, mock)
}

#[expect(
    clippy::disallowed_methods,
    reason = "the test starts its own D-Bus daemon, outside the server's runner"
)]
async fn daemon(fixture: &Fixture) -> (Child, String) {
    let config = fixture.path("bus.conf");
    let config_text = format!(
        r#"<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:path={}</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
"#,
        fixture.path("bus").display()
    );
    std::fs::write(&config, config_text).unwrap();
    let spawning = SPAWNING.lock().unwrap_or_else(PoisonError::into_inner);
    let mut daemon = Command::new("dbus-daemon")
        .arg(format!("--config-file={}", config.display()))
        .args(["--nofork", "--print-address"])
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    drop(spawning);
    let mut stdout = BufReader::new(daemon.stdout.take().unwrap()).lines();
    let address = tokio::time::timeout(WAIT, stdout.next_line())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    (daemon, address)
}

async fn connect(address: &str) -> Connection {
    zbus::connection::Builder::address(address)
        .unwrap()
        .build()
        .await
        .unwrap()
}

/// Serves `object`'s interfaces at `path`.
async fn serve(app: &Connection, mock: &Mock, path: &str, object: &Object) {
    let server = app.object_server();
    let at = || Node {
        path: path.to_owned(),
        mock: mock.clone(),
    };
    server.at(path, Accessible(at())).await.unwrap();
    if object.extents.is_some() {
        server.at(path, Component(at())).await.unwrap();
    }
    if !object.actions.is_empty() {
        server.at(path, Action(at())).await.unwrap();
    }
    if object.text.is_some() {
        server.at(path, Text(at())).await.unwrap();
        server.at(path, EditableText(at())).await.unwrap();
    }
}

/// An object's place in the mock.
#[derive(Debug)]
struct Node {
    path: String,
    mock: Mock,
}

impl Node {
    fn read<T>(&self, read: impl FnOnce(&Object, &str) -> T) -> answer::Result<T> {
        Ok(self.mock.read(&self.path, read)?)
    }

    async fn called(&self, member: &'static str) -> answer::Result<()> {
        self.mock.called(member, &self.path).await
    }
}

fn reference(app: &str, path: &str) -> (String, OwnedObjectPath) {
    (app.to_owned(), OwnedObjectPath::try_from(path).unwrap())
}

struct Accessible(Node);

#[interface(name = "org.a11y.atspi.Accessible")]
impl Accessible {
    async fn get_role(&self) -> answer::Result<u32> {
        self.0.called("GetRole").await?;
        self.0.read(|object, _| object.role)
    }

    async fn get_state(&self) -> answer::Result<Vec<u32>> {
        self.0.called("GetState").await?;
        self.0.read(|object, _| vec![object.states, 0])
    }

    async fn get_children(&self) -> answer::Result<Vec<(String, OwnedObjectPath)>> {
        self.0.called("GetChildren").await?;
        self.0.read(|object, app| {
            object
                .children
                .iter()
                .map(|child| reference(app, child))
                .collect()
        })
    }

    async fn get_child_at_index(&self, index: i32) -> answer::Result<(String, OwnedObjectPath)> {
        self.0.called("GetChildAtIndex").await?;
        self.0.read(|object, app| {
            let child = usize::try_from(index)
                .ok()
                .and_then(|index| object.children.get(index));
            child.map_or_else(
                || reference("", "/org/a11y/atspi/null"),
                |child| reference(app, child),
            )
        })
    }

    async fn get_interfaces(&self) -> answer::Result<Vec<String>> {
        self.0.called("GetInterfaces").await?;
        self.0.read(|object, _| object.interfaces())
    }

    #[zbus(property, name = "Name")]
    async fn accessible_name(&self) -> fdo::Result<String> {
        self.0.called("Name").await?;
        Ok(self.0.read(|object, _| object.name.clone())?)
    }
}

struct Component(Node);

#[interface(name = "org.a11y.atspi.Component")]
impl Component {
    async fn get_extents(&self, coord_type: u32) -> answer::Result<(i32, i32, i32, i32)> {
        self.0.called("GetExtents").await?;
        if coord_type != WINDOW_COORDS {
            return Err(fdo::Error::InvalidArgs("only WINDOW coordinates".to_owned()).into());
        }
        self.0.read(|object, _| object.extents.unwrap_or_default())
    }
}

struct Action(Node);

#[interface(name = "org.a11y.atspi.Action")]
impl Action {
    async fn get_actions(&self) -> answer::Result<Vec<(String, String, String)>> {
        self.0.called("GetActions").await?;
        self.0.read(|object, _| {
            object
                .actions
                .iter()
                .map(|action| (action.clone(), String::new(), String::new()))
                .collect()
        })
    }

    async fn do_action(&self, index: i32) -> answer::Result<bool> {
        self.0.mock.state.lock().unwrap().done.push(index);
        self.0.called("DoAction").await?;
        self.0.mock.change(&self.0.path, |object| {
            if let Some(count) = object.counter.as_mut() {
                *count += 1;
                object.name = format!("Count: {count}");
            }
        });
        for path in self.0.read(|object, _| object.toggles.clone())? {
            self.0.mock.change(&path, |object| object.states ^= SHOWING);
        }
        self.0.read(|object, _| {
            usize::try_from(index).is_ok_and(|index| index < object.actions.len())
        })
    }
}

struct Text(Node);

#[interface(name = "org.a11y.atspi.Text")]
impl Text {
    #[zbus(property)]
    async fn character_count(&self) -> fdo::Result<i32> {
        self.0.called("CharacterCount").await?;
        Ok(self.0.read(|object, _| object.text.unwrap_or_default())?)
    }
}

struct EditableText(Node);

#[interface(name = "org.a11y.atspi.EditableText")]
impl EditableText {
    async fn set_text_contents(&self, text: String) -> answer::Result<bool> {
        self.0.called("SetTextContents").await?;
        let characters = i32::try_from(text.chars().count()).unwrap();
        self.0
            .mock
            .change(&self.0.path, |object| object.text = Some(characters));
        Ok(true)
    }
}

/// The registry, which lists the applications on the bus.
struct Registry {
    apps: Vec<(String, OwnedObjectPath)>,
}

#[interface(name = "org.a11y.atspi.Accessible")]
impl Registry {
    fn get_children(&self) -> Vec<(String, OwnedObjectPath)> {
        self.apps.clone()
    }
}

/// The session bus's pointer to the accessibility bus: this same bus.
struct A11yBus {
    address: String,
}

#[interface(name = "org.a11y.Bus")]
impl A11yBus {
    fn get_address(&self) -> String {
        self.address.clone()
    }
}
