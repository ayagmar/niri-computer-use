//! The only module that talks to AT-SPI, the accessibility bus. The session bus's
//! `org.a11y.Bus` names that bus; the server finds it once at startup, and `elements`
//! exists only when it did. Every call has a one-second deadline, and each request a
//! three-second budget, so a hung application can't hold a request.
//!
//! An application is found by its process ID: the registry lists the applications by bus
//! name, and the bus itself, not the applications, answers which process each name is.
//! Only the application that owns the niri window asked about is ever called, so a hung
//! application elsewhere can't stall the request.

pub(crate) mod model;
mod switch;
mod walk;

use std::ffi::OsStr;
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::time::Instant;
use zbus::Connection;
use zbus::message::Message;
use zbus::names::OwnedErrorName;
use zbus::zvariant::{DynamicType, OwnedObjectPath, OwnedValue};

use crate::error::{ErrorName, ToolError};
use model::{Extents, Frame, Kept, NoFrame, Picked, Shape, States};
pub(crate) use switch::{Report as SwitchReport, Switch, Turned};
pub(crate) use walk::{Capped, Child, Node, Walked, Want};

/// Each call's deadline.
const CALL: Duration = Duration::from_secs(1);
/// Each request's budget, all its calls together.
pub(crate) const BUDGET: Duration = Duration::from_secs(3);
/// The most nodes one walk reads. Above the largest window measured, 15,022 objects read
/// in about 0.8 s, and under the budget at the slowest rate measured (docs/decisions.md),
/// so the budget, not the cap, ends a walk of a slow app.
pub(crate) const NODE_CAP: usize = 20_000;
const REGISTRY: &str = "org.a11y.atspi.Registry";
const ROOT: &str = "/org/a11y/atspi/accessible/root";
const ACCESSIBLE: &str = "org.a11y.atspi.Accessible";
const COMPONENT: &str = "org.a11y.atspi.Component";
const ACTION: &str = "org.a11y.atspi.Action";
const EDITABLE_TEXT: &str = "org.a11y.atspi.EditableText";
const TEXT: &str = "org.a11y.atspi.Text";
const VALUE: &str = "org.a11y.atspi.Value";
const PROPERTIES: &str = "org.freedesktop.DBus.Properties";
/// AT-SPI's null reference, which Chromium gives as a frame's child until an assistive
/// technology asks it for an object's attributes, and only then builds its tree.
const NULL: &str = "/org/a11y/atspi/null";
/// `ATSPI_COORD_TYPE_WINDOW`: relative to the toolkit's window.
const WINDOW_COORDS: u32 = 1;
/// D-Bus errors that mean the object or its application is gone.
const GONE: [&str; 4] = [
    "org.freedesktop.DBus.Error.ServiceUnknown",
    "org.freedesktop.DBus.Error.NameHasNoOwner",
    "org.freedesktop.DBus.Error.UnknownObject",
    "org.freedesktop.DBus.Error.UnknownMethod",
];

/// What a failure says in place of an error name an application made up.
const APPLICATION_ERROR: &str = "application_error";

/// What the bus answers in place of a reply when the application left without sending one.
const NO_REPLY: &str = "org.freedesktop.DBus.Error.NoReply";

/// Whether this session has an accessibility bus, decided once at startup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Presence {
    pub(crate) available: bool,
    /// The bus's address, from the session bus's `org.a11y.Bus.GetAddress`.
    pub(crate) address: Option<String>,
    /// Why there is none, when there isn't.
    pub(crate) reason: Option<String>,
}

impl Presence {
    fn absent(reason: impl Into<String>) -> Self {
        Self {
            available: false,
            address: None,
            reason: Some(reason.into()),
        }
    }
}

/// Asks the session bus at `session_bus` for the accessibility bus's address, within one
/// call's deadline for connecting and one for the call.
pub(crate) async fn detect(session_bus: Option<&OsStr>) -> Presence {
    let Some(session_bus) = session_bus.and_then(OsStr::to_str) else {
        return Presence::absent("neither DBUS_SESSION_BUS_ADDRESS nor XDG_RUNTIME_DIR is set");
    };
    let address = async {
        let connection = within(CALL, "connect to the session bus", connect(session_bus)).await?;
        let reply = within(
            CALL,
            "org.a11y.Bus.GetAddress",
            connection.call_method(
                Some("org.a11y.Bus"),
                "/org/a11y/bus",
                Some("org.a11y.Bus"),
                "GetAddress",
                &(),
            ),
        )
        .await?;
        reply
            .body()
            .deserialize::<String>()
            .map_err(|error| upstream("org.a11y.Bus.GetAddress", &error))
    };
    match address.await {
        Ok(address) => Presence {
            available: true,
            address: Some(address),
            reason: None,
        },
        Err(error) => Presence::absent(error.detail),
    }
}

async fn connect(address: &str) -> Result<Connection, zbus::Error> {
    zbus::connection::Builder::address(address)?.build().await
}

/// The accessibility bus, connected on first use and again after a connection fails, and
/// the switch that turns the session's accessibility on.
#[derive(Debug, Clone)]
pub(crate) struct A11y {
    address: String,
    connection: Arc<tokio::sync::Mutex<Option<Connection>>>,
    switch: Arc<Switch>,
}

impl A11y {
    /// The accessibility bus at `address`, whose status is on the session bus at
    /// `session_bus`.
    pub(crate) fn new(address: String, session_bus: Option<String>) -> Self {
        Self {
            switch: Arc::new(Switch::new(session_bus, address.clone())),
            address,
            connection: Arc::default(),
        }
    }

    pub(crate) fn switch(&self) -> &Switch {
        &self.switch
    }

    /// A request against the bus with `budget` from now for all its calls.
    pub(crate) async fn request(&self, budget: Duration) -> Result<Request, ToolError> {
        let deadline = Instant::now() + budget;
        let mut cached = self.connection.lock().await;
        let connection = if let Some(connection) = cached.as_ref() {
            connection.clone()
        } else {
            let left = deadline.saturating_duration_since(Instant::now()).min(CALL);
            let fresh = within(
                left,
                "connect to the accessibility bus",
                connect(&self.address),
            )
            .await?;
            *cached = Some(fresh.clone());
            fresh
        };
        drop(cached);
        Ok(Request {
            connection,
            deadline,
            shared: Arc::clone(&self.connection),
            names_only: false,
        })
    }
}

/// One request's calls on the accessibility bus, sharing its budget.
#[derive(Debug)]
pub(crate) struct Request {
    connection: Connection,
    deadline: Instant,
    /// Cleared when a call fails other than with a D-Bus error, so the next request
    /// connects again.
    shared: Arc<tokio::sync::Mutex<Option<Connection>>>,
    /// Whether a failure says only the D-Bus error's name, and only a standard one, never
    /// the message the application wrote with it.
    names_only: bool,
}

/// Why a call didn't answer.
#[derive(Debug)]
pub(crate) enum Failed {
    /// The object or its application is gone.
    Gone(String),
    /// The application answered with another D-Bus error, as toolkits do for an
    /// interface the object doesn't have.
    Refused(ToolError),
    /// No answer: the deadline passed, or the connection failed.
    Error(ToolError),
}

impl From<Failed> for ToolError {
    fn from(failed: Failed) -> Self {
        match failed {
            Failed::Gone(detail) => Self::new(ErrorName::UpstreamError, detail),
            Failed::Refused(error) | Failed::Error(error) => error,
        }
    }
}

/// What became of a call that acts on the application.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Dispatched<T> {
    /// The application answered.
    Answered(T),
    /// The call may have gone out, and its reply was lost or couldn't be read: the
    /// application may have acted. Why, without anything the application wrote.
    Lost(String),
}

/// An application on the bus: its unique name and process ID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct App {
    pub(crate) bus: String,
    pub(crate) pid: i32,
}

/// The frame of an application that is niri's window, read whole.
#[derive(Debug, Clone)]
pub(crate) struct FoundFrame {
    pub(crate) node: Node,
    /// Whether it passes the frame guard.
    pub(crate) fits: bool,
    /// Whether the app built its tree at this request's asking (`Request::built`), and may
    /// not have sent all of it yet.
    pub(crate) building: bool,
}

impl Request {
    /// The same request, whose failures leave out the messages that come with D-Bus
    /// errors. An application writes those, and may put the text of a field in them; the
    /// element actions promise never to return or log that (see docs/decisions.md).
    pub(crate) const fn names_only(mut self) -> Self {
        self.names_only = true;
        self
    }

    /// Calls `method` and reads its reply as `R`, within the call deadline and what is
    /// left of the budget.
    async fn call<B, R>(
        &self,
        target: (&str, &str),
        member: (&str, &str),
        body: &B,
    ) -> Result<R, Failed>
    where
        B: Serialize + DynamicType + Sync,
        R: DeserializeOwned + zbus::zvariant::Type,
    {
        let (destination, path) = target;
        let (interface, method) = member;
        let what = format!("{interface}.{method} on {destination}");
        let left = self.deadline.saturating_duration_since(Instant::now());
        let call =
            self.connection
                .call_method(Some(destination), path, Some(interface), method, body);
        let reply = match tokio::time::timeout(left.min(CALL), call).await {
            Ok(Ok(reply)) => reply,
            Ok(Err(error)) => return Err(self.failed(&what, error).await),
            Err(_) => return Err(Failed::Error(late(&what, left))),
        };
        reply
            .body()
            .deserialize::<R>()
            .map_err(|error| Failed::Error(self.upstream(&what, &error)))
    }

    /// Calls `member`, which acts on the application, once. Nothing goes out when less than
    /// the call's own deadline is left of the budget or the connection is closed already.
    /// Once the call may have gone out, no reply within the deadline, a lost connection, the
    /// bus's `NoReply` and a reply that can't be read are `Lost`, since the application may
    /// have acted; an error the application answered is its answer, as `Failed`.
    async fn dispatch<B, R>(
        &self,
        target: (&str, &str),
        member: (&str, &str),
        body: &B,
    ) -> Result<Dispatched<R>, Failed>
    where
        B: Serialize + DynamicType + Sync,
        R: DeserializeOwned + zbus::zvariant::Type,
    {
        let (destination, path) = target;
        let (interface, method) = member;
        let what = format!("{interface}.{method} on {destination}");
        let left = self.deadline.saturating_duration_since(Instant::now());
        let deadline = acting_deadline(&what, left).map_err(Failed::Error)?;
        if self.connection.is_closed() {
            *self.shared.lock().await = None;
            return Err(Failed::Error(ToolError::new(
                ErrorName::UpstreamError,
                format!("{what}: the connection to the accessibility bus closed; nothing was sent"),
            )));
        }
        let lost = |why: String| {
            Ok(Dispatched::Lost(format!(
                "{why}; the call went out, so the app may have acted"
            )))
        };
        let call =
            self.connection
                .call_method(Some(destination), path, Some(interface), method, body);
        let reply = match tokio::time::timeout(deadline, call).await {
            Ok(Ok(reply)) => reply,
            Err(_) => return lost(late(&what, deadline).detail),
            Ok(Err(zbus::Error::MethodError(name, _, _))) if name.as_str() == NO_REPLY => {
                return lost(format!("{what}: {NO_REPLY}"));
            }
            Ok(Err(error @ zbus::Error::MethodError(..))) => {
                return Err(self.failed(&what, error).await);
            }
            Ok(Err(error)) => {
                *self.shared.lock().await = None;
                return lost(self.upstream(&what, &error).detail);
            }
        };
        reply.body().deserialize::<R>().map_or_else(
            |_| lost(format!("{what}: its reply couldn't be read")),
            |answer| Ok(Dispatched::Answered(answer)),
        )
    }

    async fn failed(&self, what: &str, error: zbus::Error) -> Failed {
        if let zbus::Error::MethodError(name, _, _) = &error {
            if GONE.contains(&name.as_str()) {
                return Failed::Gone(self.upstream(what, &error).detail);
            }
            return Failed::Refused(self.upstream(what, &error));
        }
        *self.shared.lock().await = None;
        Failed::Error(self.upstream(what, &error))
    }

    /// `upstream_error` for a failed call: the whole error, or with `names_only` its
    /// name alone, when it is one of D-Bus's standard ones.
    fn upstream(&self, what: &str, error: &zbus::Error) -> ToolError {
        if !self.names_only {
            return upstream(what, error);
        }
        let named = if let zbus::Error::MethodError(name, _, reply) = error {
            standard_name(name, reply)
        } else if let zbus::Error::InputOutput(error) = error {
            error.to_string()
        } else {
            "the connection failed or the reply couldn't be read".to_owned()
        };
        ToolError::new(ErrorName::UpstreamError, format!("{what}: {named}"))
    }

    /// The applications the registry lists, with the process ID the bus gives each.
    /// Names that go away meanwhile are left out.
    pub(crate) async fn apps(&self) -> Result<Vec<App>, ToolError> {
        let children: Vec<(String, OwnedObjectPath)> = self
            .call((REGISTRY, ROOT), (ACCESSIBLE, "GetChildren"), &())
            .await?;
        let mut apps = Vec::new();
        for (bus, _) in children {
            match self.pid(&bus).await {
                Ok(pid) => apps.push(App { bus, pid }),
                Err(Failed::Gone(_)) => {}
                Err(failed) => return Err(failed.into()),
            }
        }
        Ok(apps)
    }

    async fn pid(&self, bus: &str) -> Result<i32, Failed> {
        let pid: u32 = self
            .call(
                ("org.freedesktop.DBus", "/org/freedesktop/DBus"),
                ("org.freedesktop.DBus", "GetConnectionUnixProcessID"),
                &(bus,),
            )
            .await?;
        i32::try_from(pid).map_err(|_| Failed::Gone(format!("{bus} has pid {pid}")))
    }

    /// The application whose process is `pid`, or `not_accessible`.
    pub(crate) async fn app(&self, pid: i32) -> Result<App, ToolError> {
        self.find_app(pid)
            .await?
            .ok_or_else(|| not_accessible(off_bus(pid)))
    }

    /// The application whose process is `pid`, if it is on the bus.
    pub(crate) async fn find_app(&self, pid: i32) -> Result<Option<App>, ToolError> {
        Ok(self.apps().await?.into_iter().find(|app| app.pid == pid))
    }

    /// When the request's budget runs out.
    pub(crate) const fn deadline(&self) -> Instant {
        self.deadline
    }

    /// The application's frame that is niri's `window`, decided with the app's `others`
    /// windows in niri.
    pub(crate) async fn frame(
        &self,
        app: &App,
        window: Shape<'_>,
        others: &[Shape<'_>],
    ) -> Result<FoundFrame, ToolError> {
        let root = self
            .node(&app.bus, ROOT)
            .await?
            .ok_or_else(|| not_accessible(format!("application {} left the bus", app.bus)))?;
        let mut frames = Vec::new();
        for child in &root.children {
            if let Some(node) = self.node(&app.bus, &child.path).await? {
                frames.push(node);
            }
        }
        let candidates: Vec<(usize, Frame<'_>)> = frames
            .iter()
            .enumerate()
            .filter_map(|(index, node)| {
                node.extents.map(|extents| {
                    (
                        index,
                        Frame {
                            extents,
                            name: &node.name,
                        },
                    )
                })
            })
            .collect();
        let listed: Vec<Frame<'_>> = candidates.iter().map(|(_, frame)| *frame).collect();
        let (picked, fits) = match model::pick_frame(&listed, window, others) {
            Ok(Picked::Fits(index)) => (index, true),
            Ok(Picked::Mismatch(index)) => (index, false),
            Err(NoFrame::None) => {
                return Err(not_accessible(format!(
                    "application {} has no window on the accessibility bus",
                    app.bus
                )));
            }
            Err(NoFrame::Ambiguous(count)) => {
                return Err(ToolError::new(
                    ErrorName::AmbiguousWindow,
                    format!("{count} of the application's accessible windows could be this window"),
                ));
            }
            Err(NoFrame::Contradicted) => {
                return Err(ToolError::new(
                    ErrorName::AmbiguousWindow,
                    "the application's accessible window of this window's size doesn't have its title, and another of its accessible windows does",
                ));
            }
            Err(NoFrame::Shared) => {
                return Err(ToolError::new(
                    ErrorName::AmbiguousWindow,
                    "another of the application's windows could be this window's accessible window: it has no accessible window of its own size, and its title doesn't tell the two apart",
                ));
            }
        };
        let node = candidates
            .get(picked)
            .and_then(|(index, _)| frames.get(*index))
            .cloned()
            .ok_or_else(|| ToolError::new(ErrorName::UpstreamError, "frame index out of range"))?;
        let (node, building) = self.built(&app.bus, node).await?;
        Ok(FoundFrame {
            node,
            fits,
            building,
        })
    }

    /// `frame` with its tree built, and whether it was built just now: a frame whose
    /// children are all null references, as Chromium's are until an assistive technology
    /// asks for an object's attributes, is asked for its own, then read again. Chromium
    /// then builds its own window's tree at once and the page's within a second or so. A
    /// frame read again with no child but null references built nothing.
    async fn built(&self, bus: &str, frame: Node) -> Result<(Node, bool), ToolError> {
        if frame.children.is_empty() || has_tree(&frame) {
            return Ok((frame, false));
        }
        let attributes = self
            .call::<_, std::collections::HashMap<String, String>>(
                (bus, &frame.path),
                (ACCESSIBLE, "GetAttributes"),
                &(),
            )
            .await;
        if let Err(Failed::Error(error)) = attributes {
            return Err(error);
        }
        let read = self.node(bus, &frame.path).await?.unwrap_or(frame);
        let building = has_tree(&read);
        Ok((read, building))
    }

    /// The node at `path` in application `bus`, or `None` once it is gone. A node without
    /// a Component or Action interface has no extents or no actions. Its action names are
    /// as the app lists them, empty ones too (`named`). An app's refusal to list its
    /// children is an error.
    pub(crate) async fn node(&self, bus: &str, path: &str) -> Result<Option<Node>, ToolError> {
        self.read_node((bus, path), false).await
    }

    /// `node` for the walk: children the app refuses to list, as at-spi2-atk refuses more
    /// than 65,536, leave it `children_refused` with none, so the walk reads on without
    /// them instead of failing.
    async fn walked_node(&self, bus: &str, path: &str) -> Result<Option<Node>, ToolError> {
        self.read_node((bus, path), true).await
    }

    async fn read_node(
        &self,
        (bus, path): (&str, &str),
        unread_if_refused: bool,
    ) -> Result<Option<Node>, ToolError> {
        let at = (bus, path);
        let (role, name, states, children, extents, actions) = tokio::join!(
            self.call::<_, u32>(at, (ACCESSIBLE, "GetRole"), &()),
            self.name(at),
            self.call::<_, Vec<u32>>(at, (ACCESSIBLE, "GetState"), &()),
            self.call::<_, Vec<(String, OwnedObjectPath)>>(at, (ACCESSIBLE, "GetChildren"), &()),
            self.extents(bus, path),
            self.listed_actions(at),
        );
        let (children, children_refused) = match children {
            Err(Failed::Refused(_)) if unread_if_refused => (Ok(Vec::new()), true),
            listed => (listed, false),
        };
        let (Some(role), Some(name), Some(states), Some(children)) = (
            present(role)?,
            present(name)?,
            present(states)?,
            present(children)?,
        ) else {
            return Ok(None);
        };
        Ok(Some(Node {
            path: path.to_owned(),
            role,
            name,
            states: States::from_words(&states),
            extents: optional(extents)?,
            actions: optional(actions)?.unwrap_or_default(),
            children: (0..)
                .zip(children)
                .filter(|(_, (child_bus, _))| child_bus == bus)
                .map(|(index, (_, child))| Child {
                    index,
                    path: child.to_string(),
                })
                .collect(),
            children_refused,
        }))
    }

    /// Whether the request's whole budget is used up.
    pub(crate) fn spent(&self) -> bool {
        Instant::now() >= self.deadline
    }

    /// The nodes under `root` in application `bus`, at most `NODE_CAP` of them, until
    /// `want` has more than it asked for or the budget runs out.
    pub(crate) async fn walk<F: Fn(&Node) -> bool + Sync>(
        &self,
        bus: &str,
        root: &Node,
        want: Want<F>,
    ) -> Result<Walked, ToolError> {
        walk::walk(&AppTree { request: self, bus }, root, NODE_CAP, want).await
    }

    /// The accessible name of the object at `at`.
    pub(crate) async fn name(&self, at: (&str, &str)) -> Result<String, Failed> {
        let value: OwnedValue = self
            .call(at, (PROPERTIES, "Get"), &(ACCESSIBLE, "Name"))
            .await?;
        Ok(String::try_from(value).unwrap_or_default())
    }

    /// The child at `index` among all the children of the object at `parent` in
    /// application `bus`: its bus name and path.
    pub(crate) async fn child_at(
        &self,
        bus: &str,
        parent: &str,
        index: i32,
    ) -> Result<(String, String), Failed> {
        let (child_bus, child): (String, OwnedObjectPath) = self
            .call((bus, parent), (ACCESSIBLE, "GetChildAtIndex"), &(index,))
            .await?;
        Ok((child_bus, child.to_string()))
    }

    /// The object's extents in its window, from its Component interface.
    pub(crate) async fn extents(&self, bus: &str, path: &str) -> Result<Extents, Failed> {
        let (x, y, width, height): (i32, i32, i32, i32) = self
            .call((bus, path), (COMPONENT, "GetExtents"), &(WINDOW_COORDS,))
            .await?;
        Ok(Extents {
            x,
            y,
            width,
            height,
        })
    }

    /// What a kept element is now: its role, states and extents, and its frame's extents,
    /// read together.
    pub(crate) async fn probe(&self, element: &ElementRef) -> Result<Probe, Failed> {
        let at = element.at();
        let (role, states, extents, frame) = tokio::join!(
            self.call::<_, u32>(at, (ACCESSIBLE, "GetRole"), &()),
            self.call::<_, Vec<u32>>(at, (ACCESSIBLE, "GetState"), &()),
            self.extents(&element.bus, &element.path),
            self.extents(&element.bus, &element.frame),
        );
        Ok(Probe {
            role: role?,
            states: States::from_words(&states?),
            extents: extents?,
            frame: frame?,
        })
    }

    /// The kept element's role and states now: its states, then its role, one call after
    /// the other, so the role is as late as the app answers.
    pub(crate) async fn state(&self, element: &ElementRef) -> Result<(u32, States), Failed> {
        let at = element.at();
        let states: Vec<u32> = self.call(at, (ACCESSIBLE, "GetState"), &()).await?;
        let role: u32 = self.call(at, (ACCESSIBLE, "GetRole"), &()).await?;
        Ok((role, States::from_words(&states)))
    }

    /// The names of the element's actions now, as `GetActions` lists them, empty ones
    /// included.
    pub(crate) async fn listed(&self, element: &ElementRef) -> Result<Vec<String>, Failed> {
        self.listed_actions(element.at()).await
    }

    /// The names of the actions of the object at `at`, in index order, as `GetActions`
    /// lists them.
    async fn listed_actions(&self, at: (&str, &str)) -> Result<Vec<String>, Failed> {
        let listed: Vec<(String, String, String)> =
            self.call(at, (ACTION, "GetActions"), &()).await?;
        Ok(listed.into_iter().map(|(name, _, _)| name).collect())
    }

    /// `names`, the actions of the object at `at` in index order, with each empty one
    /// asked for by `GetName`. Chromium lists every name empty and gives its own, such as
    /// `click`, only there. That costs a round trip per action, so only elements that are
    /// listed or acted on are named, not every node a walk reads. A name the app refuses,
    /// or an object gone meanwhile, leaves that name empty and the others as they are.
    pub(crate) async fn named(
        &self,
        at: (&str, &str),
        mut names: Vec<String>,
    ) -> Result<Vec<String>, ToolError> {
        for (index, name) in (0_i32..).zip(names.iter_mut()) {
            if !name.is_empty() {
                continue;
            }
            match self
                .call::<_, String>(at, (ACTION, "GetName"), &(index,))
                .await
            {
                Ok(own) => *name = own,
                Err(Failed::Refused(_) | Failed::Gone(_)) => {}
                Err(Failed::Error(error)) => return Err(error),
            }
        }
        Ok(names)
    }

    /// What the app names the element's action at `index`, the one `DoAction(index)` does,
    /// by `GetName` and by `GetLocalizedName`.
    pub(crate) async fn action_names(
        &self,
        element: &ElementRef,
        index: i32,
    ) -> Result<ActionNames, Failed> {
        Ok(ActionNames {
            own: self.action_name(element, "GetName", index).await?,
            localized: self.action_name(element, "GetLocalizedName", index).await?,
        })
    }

    /// The name `method` gives the element's action at `index`, `None` when the app
    /// answers it with an error. That includes the errors that mean the object is gone:
    /// Qt answers `UnknownObject` for a method its adaptor doesn't handle, such as
    /// `GetLocalizedName`, so it says nothing about the object. An object really gone
    /// shows in the calls around this one.
    async fn action_name(
        &self,
        element: &ElementRef,
        method: &str,
        index: i32,
    ) -> Result<Option<String>, Failed> {
        match self
            .call::<_, String>(element.at(), (ACTION, method), &(index,))
            .await
        {
            Ok(name) => Ok(Some(name)),
            Err(Failed::Refused(_) | Failed::Gone(_)) => Ok(None),
            Err(failed) => Err(failed),
        }
    }

    /// Whether the element has an `EditableText` interface.
    pub(crate) async fn editable_text(&self, element: &ElementRef) -> Result<bool, Failed> {
        self.has_interface(element, EDITABLE_TEXT).await
    }

    /// Whether the element lists `interface` among its interfaces.
    async fn has_interface(&self, element: &ElementRef, interface: &str) -> Result<bool, Failed> {
        let interfaces: Vec<String> = self
            .call(element.at(), (ACCESSIBLE, "GetInterfaces"), &())
            .await?;
        Ok(interfaces.iter().any(|listed| listed == interface))
    }

    /// Asks the application to do the element's action at `index`. The answer only says
    /// whether it took the request: the app does the action afterwards, on its own time.
    pub(crate) async fn do_action(
        &self,
        element: &ElementRef,
        index: i32,
    ) -> Result<Dispatched<bool>, Failed> {
        self.dispatch(element.at(), (ACTION, "DoAction"), &(index,))
            .await
    }

    /// Replaces the element's whole text with `text`.
    pub(crate) async fn set_text(
        &self,
        element: &ElementRef,
        text: &str,
    ) -> Result<Dispatched<bool>, Failed> {
        self.dispatch(element.at(), (EDITABLE_TEXT, "SetTextContents"), &(text,))
            .await
    }

    /// How many characters the element's text has now.
    pub(crate) async fn character_count(&self, element: &ElementRef) -> Result<i32, Failed> {
        let value: OwnedValue = self
            .call(element.at(), (PROPERTIES, "Get"), &(TEXT, "CharacterCount"))
            .await?;
        i32::try_from(value).map_err(|_| {
            Failed::Error(ToolError::new(
                ErrorName::UpstreamError,
                format!("{TEXT}.CharacterCount on {}: not an integer", element.bus),
            ))
        })
    }

    /// The element's first `count` characters of text. The caller bounds `count`.
    pub(crate) async fn text(&self, element: &ElementRef, count: i32) -> Result<String, Failed> {
        self.call(element.at(), (TEXT, "GetText"), &(0_i32, count))
            .await
    }

    /// The element's `Value.CurrentValue`, or `None` when it has no `Value` interface, as
    /// most elements don't. The interfaces are asked first: the error an app answers for a
    /// property of an interface the element lacks varies, and may be one that `GONE` reads
    /// as the object gone.
    pub(crate) async fn current_value(&self, element: &ElementRef) -> Result<Option<f64>, Failed> {
        if !self.has_interface(element, VALUE).await? {
            return Ok(None);
        }
        let value: OwnedValue = self
            .call(element.at(), (PROPERTIES, "Get"), &(VALUE, "CurrentValue"))
            .await?;
        f64::try_from(value).map(Some).map_err(|_| {
            Failed::Error(ToolError::new(
                ErrorName::UpstreamError,
                format!("{VALUE}.CurrentValue on {}: not a number", element.bus),
            ))
        })
    }
}

/// One application's tree, for the walk.
struct AppTree<'a> {
    request: &'a Request,
    bus: &'a str,
}

impl walk::Source for AppTree<'_> {
    async fn node(&self, path: &str) -> Result<Option<Node>, ToolError> {
        self.request.walked_node(self.bus, path).await
    }

    fn spent(&self) -> bool {
        self.request.spent()
    }
}

/// What an app names one of an element's actions by each of its two naming methods, `None`
/// for one it answers with an error.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ActionNames {
    /// `GetName`'s.
    pub(crate) own: Option<String>,
    /// `GetLocalizedName`'s.
    pub(crate) localized: Option<String>,
}

/// A kept element as it is now.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Probe {
    pub(crate) role: u32,
    pub(crate) states: States,
    pub(crate) extents: Extents,
    pub(crate) frame: Extents,
}

/// An element listed under the lease, kept so a pointer tool can aim at it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ElementRef {
    /// The application's unique bus name, never reused while the bus lives.
    pub(crate) bus: String,
    pub(crate) path: String,
    /// The path of the frame that is the window, for the frame guard.
    pub(crate) frame: String,
    pub(crate) kept: Kept,
}

impl ElementRef {
    /// Where the element is on the bus.
    pub(crate) fn at(&self) -> (&str, &str) {
        (&self.bus, &self.path)
    }
}

/// A call on an object that may be gone: `None` once it is.
fn present<T>(result: Result<T, Failed>) -> Result<Option<T>, ToolError> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(Failed::Gone(_)) => Ok(None),
        Err(failed) => Err(failed.into()),
    }
}

/// A method call's optional interface: absent when the object doesn't have it.
fn optional<T>(result: Result<T, Failed>) -> Result<Option<T>, ToolError> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(Failed::Gone(_) | Failed::Refused(_)) => Ok(None),
        Err(Failed::Error(error)) => Err(error),
    }
}

async fn within<T>(
    limit: Duration,
    what: &str,
    work: impl Future<Output = Result<T, zbus::Error>>,
) -> Result<T, ToolError> {
    match tokio::time::timeout(limit, work).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => Err(upstream(what, &error)),
        Err(_) => Err(late(what, limit)),
    }
}

/// The deadline of a call that acts, `what`: the whole of `CALL`, or `deadline_exceeded` when
/// less is `left` of the request's budget. Sent with less, an app answering in time for any
/// other call could lose its reply to the deadline, and the action would be `uncertain`
/// though it most likely happened.
fn acting_deadline(what: &str, left: Duration) -> Result<Duration, ToolError> {
    if left < CALL {
        return Err(ToolError::new(
            ErrorName::DeadlineExceeded,
            format!(
                "{what}: less than the call's {} s is left of the request's {} s budget; nothing was sent",
                CALL.as_secs(),
                BUDGET.as_secs()
            ),
        ));
    }
    Ok(CALL)
}

fn late(what: &str, left: Duration) -> ToolError {
    let detail = if left < CALL {
        format!(
            "{what}: the request's {} s budget ran out; an application may be hung",
            BUDGET.as_secs()
        )
    } else {
        format!(
            "{what}: no answer within {} s; the application may be hung",
            CALL.as_secs()
        )
    };
    ToolError::new(ErrorName::DeadlineExceeded, detail)
}

/// A D-Bus error's name when it is one the D-Bus specification defines, as zbus lists them,
/// or `application_error`: an application names its own errors, and may put anything that
/// fits in a name, such as a record's identifier.
fn standard_name(name: &OwnedErrorName, reply: &Message) -> String {
    let known = zbus::fdo::Error::from(zbus::Error::MethodError(name.clone(), None, reply.clone()));
    if matches!(known, zbus::fdo::Error::ZBus(_)) {
        return APPLICATION_ERROR.to_owned();
    }
    name.to_string()
}

fn upstream(what: &str, error: &zbus::Error) -> ToolError {
    ToolError::new(ErrorName::UpstreamError, format!("{what}: {error}"))
}

/// Whether `node` has a child that isn't AT-SPI's null reference.
fn has_tree(node: &Node) -> bool {
    node.children.iter().any(|child| child.path != NULL)
}

/// Why `not_accessible` for an application that isn't on the bus.
pub(crate) fn off_bus(pid: i32) -> String {
    format!(
        "no application with pid {pid} is on the accessibility bus; it may not support accessibility"
    )
}

pub(crate) fn not_accessible(detail: String) -> ToolError {
    ToolError::new(ErrorName::NotAccessible, detail)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn without_a_session_bus_there_is_no_accessibility_bus() {
        let presence = detect(None).await;
        assert!(!presence.available);
        assert_eq!(
            presence.reason.as_deref(),
            Some("neither DBUS_SESSION_BUS_ADDRESS nor XDG_RUNTIME_DIR is set")
        );
    }

    #[tokio::test]
    async fn an_unreachable_session_bus_says_why_without_waiting_long() {
        let dir = crate::test_support::fresh_dir("a11y-session");
        let address = format!("unix:path={}", dir.join("missing").display());
        let started = std::time::Instant::now();
        let presence = detect(Some(OsStr::new(&address))).await;
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(!presence.available);
        assert!(
            presence
                .reason
                .is_some_and(|reason| reason.starts_with("connect to the session bus")),
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn an_acting_call_goes_out_only_with_its_whole_deadline_left() {
        assert_eq!(acting_deadline("DoAction", BUDGET), Ok(CALL));
        assert_eq!(acting_deadline("DoAction", CALL), Ok(CALL));
        for left in [Duration::from_millis(999), Duration::ZERO] {
            let refused = acting_deadline("DoAction", left).unwrap_err();
            assert_eq!(refused.name, ErrorName::DeadlineExceeded);
            assert!(
                refused.detail.ends_with("nothing was sent"),
                "{}",
                refused.detail
            );
        }
    }
}
