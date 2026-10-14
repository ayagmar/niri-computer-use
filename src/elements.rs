//! `elements`' work, and aiming a pointer tool at an element it listed. niri says where
//! the window is, right when asked; the accessibility bus says what is in it and where
//! relative to the window (see `a11y::model`). Names are the app's data: they are
//! returned, never logged. `actions` acts on a listed element without the pointer.

pub(crate) mod actions;

use std::time::Duration;

use niri_ipc::Window;
use serde::Serialize;
use tokio::time::Instant;

use crate::a11y::model::{
    self, Extents, Filter, Fresh, LayoutBox, Link, NameHash, Origin, Placement, Refused, Row,
    Scene, Shape, Unbound, Unmappable,
};
use crate::a11y::{self, A11y, Capped, ElementRef, Failed, Node, Switch, Turned, Want};
use crate::changes::elements::{Seen, Unknown};
use crate::coords::LayoutPt;
use crate::error::{CallError, ErrorName, ToolError};
use crate::niri;
use crate::niri::Socket;
use crate::policy::{self, Loaded, OffBus};

/// What `elements` was asked for.
#[derive(Debug, Clone)]
pub(crate) struct Ask {
    pub(crate) window_id: u64,
    pub(crate) filter: Filter,
    /// The most elements to list. 0 lists none: the walk stops at the first match, names
    /// no actions, and `truncated` says whether there was one.
    pub(crate) limit: usize,
    /// Read on past the matches listed, to the end of the tree, its budget or the cap, so
    /// that an element outside any row is bound only when no other element of the tree has
    /// its role and name. When the walk still stopped before the end, such an element is
    /// `Unbound::CutShort`, and `look` leaves its ref out for a wait (`by`).
    pub(crate) whole: bool,
    /// When the walk and the naming of its actions must be done by, if sooner than their
    /// own budgets allow: a wait's end. A walk that runs out of it gives what it read, cut
    /// short, rather than nothing.
    pub(crate) by: Option<Instant>,
}

/// What `elements` returns.
#[derive(Debug, Serialize)]
pub(crate) struct Listing {
    pub(crate) window_id: u64,
    pub(crate) elements: Vec<Listed>,
    /// More elements matched than `limit`, and those past it aren't listed.
    pub(crate) truncated: bool,
    /// How many accessible objects the walk read.
    pub(crate) walked: usize,
    /// The walk stopped early, so elements further on are missing.
    pub(crate) capped: bool,
    /// Why: `node_cap`, or `budget_exhausted` when the request's three seconds ran out.
    pub(crate) capped_reason: Option<Capped>,
    /// Why a listed element's ref was left out, with `Ask::whole` or `Ask::by`.
    #[serde(skip)]
    pub(crate) withheld: Option<Withheld>,
    /// The app built its tree at this call's asking, as Chromium builds it only when an
    /// assistive technology asks, and may not have sent all of it yet: list again.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub(crate) building: bool,
}

/// Why `look` left a listed element's ref out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Withheld {
    /// The walk stopped before it could tell whether another element has its role and
    /// name (`Unbound::CutShort`). `elements` gives such a ref, which every tool refuses.
    Unchecked,
    /// Naming its actions ran out of `Ask::by`'s time, so they are as the walk read them,
    /// and a ref would be refused rather than guessed.
    Unnamed,
    /// As `Unnamed`, but naming ran out of its own `NAMING` with `Ask::by`'s time left: the
    /// app names actions slower than naming allows, and more time wouldn't help.
    SlowNames,
}

/// One element.
#[derive(Debug, Serialize)]
pub(crate) struct Listed {
    /// `elem-<tag>-N` for the pointer tools' `element`, while this server holds the lease.
    pub(crate) element_ref: Option<String>,
    pub(crate) role: &'static str,
    /// The app's text: untrusted data.
    pub(crate) name: String,
    pub(crate) states: Vec<&'static str>,
    pub(crate) actions: Vec<String>,
    pub(crate) layout_box: Option<LayoutBox>,
    pub(crate) unmappable: Option<Unmappable>,
    /// Set when nothing on the bus binds the element to the record it acts on, so its ref
    /// is refused with `element_unbound`: aim at its pixels instead.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) unbound: Option<Unbound>,
}

/// States most elements have, which a compact line leaves out; `disabled` stands for a
/// missing `sensitive`.
/// How long naming the actions of the elements a listing or a change report returns may
/// take, after the walk's own budget (`Request::named`).
const NAMING: Duration = Duration::from_secs(1);
const USUAL_STATES: [&str; 5] = ["enabled", "focusable", "sensitive", "showing", "visible"];

impl Listing {
    /// The listing of a walk of window `window_id` cut short before it read anything.
    const fn cut(window_id: u64) -> Self {
        Self {
            window_id,
            elements: Vec::new(),
            truncated: false,
            walked: 0,
            capped: true,
            capped_reason: Some(Capped::BudgetExhausted),
            withheld: None,
            building: false,
        }
    }

    /// The listing as text, a few tokens per element: a line about the walk, then one line
    /// per element.
    pub(crate) fn compact(&self) -> String {
        let mut walk = format!(
            "window {}: {} elements, {} objects walked",
            self.window_id,
            self.elements.len(),
            self.walked
        );
        if self.truncated {
            walk.push_str("; more match past the limit");
        }
        if let Some(reason) = self.capped_reason {
            walk = format!("{walk}; walk cut short: {}", word(reason));
        }
        if self.building {
            walk.push_str("; the app built its tree just now, so list again for the rest");
        }
        std::iter::once(walk)
            .chain(self.elements.iter().map(Listed::line))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

impl Listed {
    /// `ref role "name" [states] ("action" …) x,y,w,h`, with `-` for no ref,
    /// `unmappable:<why>` for no box, and `unbound:<why>` last for a ref that is refused.
    /// The name and each action, the app's text, are quoted
    /// and escaped, so they can't start a line of their own or run into each other; the
    /// role and states come from this server's tables.
    fn line(&self) -> String {
        let mut states: Vec<&str> = self
            .states
            .iter()
            .copied()
            .filter(|state| !USUAL_STATES.contains(state))
            .collect();
        if !self.states.contains(&"sensitive") {
            states.push(model::DISABLED);
        }
        let place = match (self.layout_box, self.unmappable) {
            (Some(placed), _) => format!(
                "{:.0},{:.0},{:.0},{:.0}",
                placed.x, placed.y, placed.width, placed.height
            ),
            (None, Some(why)) => format!("unmappable:{}", word(why)),
            (None, None) => "-".to_owned(),
        };
        let unbound = self
            .unbound
            .map(|why| format!(" unbound:{}", word(why)))
            .unwrap_or_default();
        format!(
            "{} {} {:?} [{}] ({}) {place}{unbound}",
            self.element_ref.as_deref().unwrap_or("-"),
            self.role,
            self.name,
            states.join(" "),
            self.actions
                .iter()
                .map(|action| format!("{action:?}"))
                .collect::<Vec<_>>()
                .join(" "),
        )
    }
}

/// A unit variant's name as it is serialized.
pub(crate) fn word(value: impl Serialize) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// A window as niri reports it right now, where its geometry starts in the layout or why
/// that isn't known, and niri's other windows.
#[derive(Debug)]
struct Placed {
    window: Window,
    origin: Result<Origin, Unmappable>,
    others: Vec<Window>,
}

/// niri's window `id` right now, with its place in the layout, from fresh requests. Any of
/// them failing fails the call with niri's error.
async fn placed(socket: &Socket, id: u64) -> Result<Option<Placed>, ToolError> {
    let (windows, workspaces, outputs, overview) = tokio::join!(
        niri::windows(socket),
        niri::workspaces(socket),
        niri::outputs(socket),
        niri::overview_open(socket)
    );
    let (window, others): (Vec<Window>, Vec<Window>) =
        windows?.into_iter().partition(|window| window.id == id);
    let Some(window) = window.into_iter().next() else {
        return Ok(None);
    };
    let scene = Scene {
        workspaces: &workspaces?,
        outputs: &outputs?,
        windows: &others,
        overview_open: overview?,
    };
    let origin = model::origin(&window, &scene);
    Ok(Some(Placed {
        window,
        origin,
        others,
    }))
}

/// Whether `other` is another niri window of `window`'s application.
fn same_app(window: &Window, other: &Window) -> bool {
    other.id != window.id && other.pid.is_some() && other.pid == window.pid
}

/// How frames are matched to niri's `window`.
fn shape(window: &Window) -> Shape<'_> {
    Shape {
        size: window.layout.window_size,
        title: window.title.as_deref(),
    }
}

/// The application's other windows among `windows`, as frames are matched to them, so its
/// frames are decided for all of them together.
fn others<'a>(window: &Window, windows: impl IntoIterator<Item = &'a Window>) -> Vec<Shape<'a>> {
    windows
        .into_iter()
        .filter(|other| same_app(window, other))
        .map(shape)
        .collect()
}

/// One walk of a window's tree: the window as niri placed it, its application's process,
/// bus name and frame, and the objects under the frame.
struct Read {
    placed: Placed,
    pid: i32,
    bus: String,
    frame: a11y::FoundFrame,
    walked: a11y::Walked,
}

impl Read {
    /// What a ref to the object at `at` keeps.
    fn element(&self, at: usize, node: &Node) -> ElementRef {
        ElementRef {
            bus: self.bus.clone(),
            path: node.path.clone(),
            frame: self.frame.node.path.clone(),
            kept: model::Kept {
                role: node.role,
                window: self.placed.window.id,
                pid: self.pid,
                actions: node.actions.clone(),
                lineage: self.walked.lineage(at),
                row: self.walked.row(at),
            },
        }
    }

    /// How `elements` lists the object at `at` with its actions named `actions`, keeping
    /// a ref to it through `remember`.
    fn listed_element(
        &self,
        at: usize,
        node: &Node,
        actions: Vec<String>,
        remember: &mut impl FnMut(ElementRef) -> Option<String>,
    ) -> Listed {
        let placement = Placement {
            states: node.states,
            extents: node.extents.unwrap_or(Extents {
                x: 0,
                y: 0,
                width: 0,
                height: 0,
            }),
            frame_fits: self.frame.fits,
            origin: self.placed.origin.clone(),
        };
        let placed_box = model::place(placement);
        let mut element = self.element(at, node);
        element.kept.actions.clone_from(&actions);
        let unbound = match element.kept.row {
            Row::Unbound(why) => Some(why),
            Row::Lineage | Row::Own => None,
        };
        Listed {
            element_ref: remember(element),
            role: model::role_name(node.role),
            name: node.name.clone(),
            states: node.states.names(),
            actions,
            layout_box: placed_box.ok(),
            unmappable: placed_box.err(),
            unbound,
        }
    }

    /// Whether the object at `at` sits in no row, and the walk didn't read the whole tree,
    /// so it can't tell whether another object has its role and name.
    fn unchecked(&self, at: usize) -> bool {
        self.walked.row(at) == Row::Unbound(Unbound::CutShort)
    }

    /// The objects `filter` lists, with where each sits in the walk.
    fn listed<'a>(&'a self, filter: &'a Filter) -> impl Iterator<Item = (usize, &'a Node)> {
        self.walked
            .nodes
            .iter()
            .enumerate()
            .filter(|(_, node)| listed(filter, node))
    }
}

/// What a walk looks for: the objects `filter` lists, until it found more than `limit`,
/// within `budget`; with `names_only`, its failures leave the app's error messages out.
#[derive(Debug, Clone, Copy)]
struct Walk<'a> {
    filter: &'a Filter,
    limit: usize,
    budget: Duration,
    names_only: bool,
}

/// Walks window `window_id`'s tree as `walk` says, or none when niri has no such window:
/// niri's window right now, the deny list, its application and frame, then the walk.
async fn read(
    socket: &Socket,
    a11y: &A11y,
    policy: &Loaded,
    window_id: u64,
    walk: Walk<'_>,
) -> Result<Option<Read>, ToolError> {
    let Some(placed) = placed(socket, window_id).await? else {
        return Ok(None);
    };
    let window = &placed.window;
    if let Some(refused) = policy::refuse_window(policy, window.id, window.app_id.as_deref()) {
        return Err(refused);
    }
    let pid = window.pid.ok_or_else(|| {
        a11y::not_accessible(format!("niri knows no process for window {}", window.id))
    })?;
    let request = a11y.request(walk.budget).await?;
    let request = if walk.names_only {
        request.names_only()
    } else {
        request
    };
    let Some(app) = request.find_app(pid).await? else {
        let app_id = window.app_id.as_deref();
        return Err(off_bus(a11y, policy, (pid, app_id), request.deadline()).await);
    };
    let frame = request
        .frame(&app, shape(window), &others(window, &placed.others))
        .await?;
    let want = Want {
        wanted: walk.limit,
        matches: |node: &Node| listed(walk.filter, node),
    };
    let walked = request.walk(&app.bus, &frame.node, want).await?;
    Ok(Some(Read {
        placed,
        pid,
        bus: app.bus,
        frame,
        walked,
    }))
}

/// `not_accessible` for the app of process `pid` and `app_id`, which isn't on the
/// accessibility bus, saying what the server did about the session's accessibility by
/// `deadline`. Chromium, Electron and CEF apps check it only when they start, which the
/// server can't tell from the window, so the agent gets the `app_id` and is told to ask the
/// user before restarting such an app.
async fn off_bus(
    a11y: &A11y,
    policy: &Loaded,
    (pid, app_id): (i32, Option<&str>),
    deadline: Instant,
) -> ToolError {
    let note = switched(a11y.switch(), policy, deadline).await;
    let app = app_id.map_or_else(
        || "niri knows no app_id for its window".to_owned(),
        |app_id| format!("its window's app_id is {app_id:?}"),
    );
    a11y::not_accessible(format!("{}; {app}; {note}", a11y::off_bus(pid)))
}

/// What the server did about the session's accessibility, as `policy::off_bus` decides.
async fn switched(switch: &Switch, policy: &Loaded, deadline: Instant) -> String {
    let enabled = match switch.session_enabled(deadline).await {
        Ok(enabled) => enabled,
        Err(error) => {
            return format!(
                "whether the session's accessibility is on couldn't be read: {}",
                error.detail
            );
        }
    };
    // Unread, the listeners count as none, so accessibility that is on is left alone.
    let listened = enabled && switch.listened(deadline).await.unwrap_or(false);
    let on = format!(
        "the session's accessibility is on: the app may not support it, or started while it was off; {RESTART}"
    );
    match policy::off_bus(policy, enabled, listened) {
        OffBus::Leave => on,
        OffBus::Refused(why) => format!("the session's accessibility is off, and {why}"),
        OffBus::Listen => match switch.listen(deadline, enabled).await {
            Ok(Turned::On) if enabled => on,
            Ok(Turned::On) => format!(
                "the session's accessibility was off, so the server turned it on; {GSETTINGS}; {RESTART}"
            ),
            Ok(Turned::AlreadyOn) => on,
            Ok(Turned::LetGo) => "the session's accessibility is off: something turned it off while the server's listener held it on, so the server let its listener go and leaves it off until the server restarts".to_owned(),
            Ok(Turned::Unsupported(why)) => format!("the session's accessibility is off: {why}"),
            Ok(Turned::Late) => "the session's accessibility is off, and this call's time ran out before the server's listener turned it on; the server let the listener go and tries again on the next call".to_owned(),
            Ok(Turned::Busy) => format!(
                "the session's accessibility is {}, and the server was still turning it on for another call when this one's time ran out; {RESTART}",
                if enabled { "on" } else { "off" }
            ),
            Err(error) => format!(
                "the session's accessibility is {}, and registering the server's listener failed: {}",
                if enabled { "on" } else { "off" },
                error.detail
            ),
        },
    }
}

/// What at-spi2-core does beside turning accessibility on, which the server can't undo.
const GSETTINGS: &str = "at-spi2-core also sets GSettings org.gnome.desktop.interface toolkit-accessibility to true when it turns accessibility on, and that stays after the server ends";

/// What an app started while the session's accessibility was off needs, if it is one that
/// checks it at start.
const RESTART: &str = "if it is a Chromium, Electron or CEF app, it checks only when it starts, so if it started while accessibility was off, it lists its elements only after a restart: ask the user before restarting it; a restart doesn't help other apps";

/// Lists the accessible elements of window `ask.window_id`, keeping a ref of each through
/// `remember` while there is a lease.
pub(crate) async fn list(
    socket: &Socket,
    a11y: &A11y,
    policy: &Loaded,
    ask: &Ask,
    remember: impl FnMut(ElementRef) -> Option<String>,
) -> Result<Listing, CallError> {
    look(socket, a11y, policy, ask, remember)
        .await?
        .ok_or_else(|| CallError::InvalidArguments(format!("no window with id {}", ask.window_id)))
}

/// What `elements` lists for `ask` now, for one check of `wait_for`'s element condition,
/// or none when niri has no window `ask.window_id`, also when the walk failed because the
/// window closed during it.
pub(crate) async fn poll(
    socket: &Socket,
    a11y: &A11y,
    policy: &Loaded,
    ask: &Ask,
    remember: impl FnMut(ElementRef) -> Option<String>,
) -> Result<Option<Listing>, ToolError> {
    match look(socket, a11y, policy, ask, remember).await {
        Err(_) if matches!(placed(socket, ask.window_id).await, Ok(None)) => Ok(None),
        // Out of time before the walk began, finding the app or its frame.
        Err(error)
            if error.name == ErrorName::DeadlineExceeded
                && ask.by.is_some_and(|by| Instant::now() >= by) =>
        {
            Ok(Some(Listing::cut(ask.window_id)))
        }
        looked => looked,
    }
}

/// What is left until `by`, at most `budget`.
fn budget_by(budget: Duration, by: Option<Instant>) -> Duration {
    by.map_or(budget, |by| {
        budget.min(by.saturating_duration_since(Instant::now()))
    })
}

/// `list`'s work, or none when niri has no window `ask.window_id`.
async fn look(
    socket: &Socket,
    a11y: &A11y,
    policy: &Loaded,
    ask: &Ask,
    mut remember: impl FnMut(ElementRef) -> Option<String>,
) -> Result<Option<Listing>, ToolError> {
    let walk = Walk {
        filter: &ask.filter,
        limit: if ask.whole { usize::MAX } else { ask.limit },
        budget: budget_by(a11y::BUDGET, ask.by),
        names_only: false,
    };
    let Some(read) = read(socket, a11y, policy, ask.window_id, walk).await? else {
        return Ok(None);
    };
    let naming = a11y.request(budget_by(NAMING, ask.by)).await?;
    let mut matching = read.listed(&ask.filter);
    let mut elements = Vec::new();
    let (mut cut, mut withheld) = (false, None);
    for (at, node) in matching.by_ref().take(ask.limit) {
        let actions = naming.named((&read.bus, &node.path), node.actions.clone());
        let (actions, why) = match actions.await {
            // A wait says why in its `detail` rather than give a ref every tool refuses.
            Ok(actions) => (
                actions,
                (ask.by.is_some() && read.unchecked(at)).then_some(Withheld::Unchecked),
            ),
            Err(error) if error.name == ErrorName::DeadlineExceeded && naming.spent() => {
                cut = true;
                // A wait that ran out keeps the match it read, without a ref.
                if ask.by.is_none() {
                    break;
                }
                let out = ask.by.is_some_and(|by| Instant::now() >= by);
                let why = if out {
                    Withheld::Unnamed
                } else {
                    Withheld::SlowNames
                };
                (node.actions.clone(), Some(why))
            }
            Err(error) => return Err(error),
        };
        withheld = withheld.or(why);
        let mut keep = |element| {
            if why.is_some() {
                None
            } else {
                remember(element)
            }
        };
        elements.push(read.listed_element(at, node, actions, &mut keep));
        if cut {
            break;
        }
    }
    let capped = if cut {
        Some(Capped::BudgetExhausted)
    } else {
        read.walked.capped
    };
    Ok(Some(Listing {
        window_id: read.placed.window.id,
        truncated: cut || matching.next().is_some(),
        elements,
        walked: read.walked.nodes.len(),
        capped: capped.is_some(),
        capped_reason: capped,
        withheld,
        building: read.frame.building,
    }))
}

/// Every element of window `window_id` that `elements` would list without filters, for
/// an action's change report: `Unknown` when the walk fails, stops early, or the window is
/// gone. Its failures leave the app's error messages out, as an element action's do.
pub(crate) async fn survey(
    socket: &Socket,
    a11y: &A11y,
    policy: &Loaded,
    window_id: u64,
) -> Result<Vec<Seen>, Unknown> {
    let filter = Filter::default();
    let walk = Walk {
        filter: &filter,
        limit: usize::MAX,
        budget: a11y::BUDGET,
        names_only: true,
    };
    let read = read(socket, a11y, policy, window_id, walk)
        .await
        .map_err(|error| Unknown::failed(Some(window_id), error))?
        .ok_or_else(|| Unknown::gone(window_id))?;
    if let Some(capped) = read.walked.capped {
        return Err(Unknown::capped(window_id, capped));
    }
    Ok(read
        .listed(&filter)
        .map(|(at, node)| Seen {
            element: read.element(at, node),
            name: node.name.clone(),
            states: node.states,
        })
        .collect())
}

/// Names the actions of the elements of `seen` at `reported`, the refs an action's change
/// report hands out, as `elements` names those it lists (`Request::named`), within
/// `NAMING`. When that fails they stay as the walk read them, and an action on
/// such a ref is refused rather than guessed, since `offered` reads them named.
pub(crate) async fn name_reported(a11y: &A11y, seen: &mut [Seen], reported: &[usize]) {
    let Ok(request) = a11y.request(NAMING).await else {
        return;
    };
    let request = request.names_only();
    for element in seen
        .iter_mut()
        .enumerate()
        .filter(|(at, _)| reported.contains(at))
        .map(|(_, seen)| &mut seen.element)
    {
        let actions = element.kept.actions.clone();
        if let Ok(named) = request.named(element.at(), actions).await {
            element.kept.actions = named;
        }
    }
}

/// Whether `elements` lists `node`: showing, matching the filter, and unless a role was
/// asked for, with actions or editable text when only interactive elements count, or else
/// with a name or actions.
fn listed(filter: &Filter, node: &Node) -> bool {
    let kind = match (&filter.role, filter.interactive_only) {
        (Some(_), _) => true,
        (None, true) => !node.actions.is_empty() || node.states.has(model::State::Editable),
        (None, false) => !node.name.is_empty() || !node.actions.is_empty(),
    };
    node.states.has(model::State::Showing)
        && kind
        && filter.matches(model::role_name(node.role), &node.name, node.states)
}

/// Where to aim at `element` now: the centre of its box, from niri's geometry and the
/// element as it is now, after the checks of the research report's A5. Nothing is
/// retried.
pub(crate) async fn aim(
    socket: &Socket,
    a11y: &A11y,
    policy: &Loaded,
    element: &ElementRef,
) -> Result<Aimed, ToolError> {
    let kept = &element.kept;
    let placed = placed(socket, kept.window)
        .await?
        .filter(|placed| placed.window.pid == Some(kept.pid))
        .ok_or_else(|| stale(&format!("window {} is gone", kept.window)))?;
    let window = &placed.window;
    if let Some(refused) = policy::refuse_window(policy, window.id, window.app_id.as_deref()) {
        return Err(refused);
    }
    let request = a11y.request(a11y::BUDGET).await?.names_only();
    identify(&request, element, window, &placed.others, Names::Parents).await?;
    let probe = match request.probe(element).await {
        Ok(probe) => probe,
        Err(Failed::Gone(detail)) => return Err(stale(&detail)),
        Err(failed) => return Err(failed.into()),
    };
    let fresh = Fresh {
        role: probe.role,
        placement: Placement {
            states: probe.states,
            extents: probe.extents,
            frame_fits: model::frame_fits(probe.frame, window.layout.window_size),
            origin: placed.origin.clone(),
        },
    };
    match (model::recheck(kept, fresh), placed.origin) {
        (Ok(found), Ok(origin)) => Ok(Aimed {
            at: found.centre(),
            output: origin.output,
        }),
        (Err(Refused::Stale(detail)), _) => Err(stale(&detail)),
        (Err(Refused::Unmappable(why)), _) | (Ok(_), Err(why)) => Err(unmappable(why)),
    }
}

/// Where an aimed element's centre is, and the output its window is shown on.
#[derive(Debug)]
pub(crate) struct Aimed {
    pub(crate) at: LayoutPt,
    pub(crate) output: String,
}

/// Which names `identify` compares with the ones `elements` listed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Names {
    /// The parents' from the frame down, not the element's own: a pointer aim, since a
    /// click doesn't read the element again afterwards and a counter relabels itself on
    /// every click.
    Parents,
    /// The element's own too: the element actions. A button that relabels itself when
    /// activated, as a counter does, is stale afterwards, and the agent lists it again.
    All,
}

/// Checks that `element` is still the object `elements` listed, not another that took its
/// path: its application still has niri's `window`, whose accessible frame, found again
/// with the app's other windows among niri's `windows`, is the ref's; each object from that frame down to the element is still at the index
/// it had among its parent's children, as the parent says; and each has the name it was
/// listed with, the element's own as `names` says. A list that reuses a row for another
/// record keeps its place and path but not its name. Otherwise `element_stale`. Its role
/// is checked by the caller.
pub(crate) async fn identify<'a>(
    request: &a11y::Request,
    element: &ElementRef,
    window: &Window,
    windows: impl IntoIterator<Item = &'a Window>,
    names: Names,
) -> Result<(), ToolError> {
    let names = match element.kept.row {
        Row::Lineage => names,
        Row::Own => Names::All,
        Row::Unbound(why) => return Err(ToolError::new(ErrorName::ElementUnbound, unbound(why))),
    };
    let app = request.app(element.kept.pid).await.map_err(|error| {
        if error.name == ErrorName::NotAccessible {
            stale("the application left the accessibility bus")
        } else {
            error
        }
    })?;
    if app.bus != element.bus {
        return Err(stale(
            "the application connected to the accessibility bus again",
        ));
    }
    let frame = request
        .frame(&app, shape(window), &others(window, windows))
        .await?;
    if frame.node.path != element.frame {
        return Err(stale(&format!(
            "window {} is another accessible window now",
            window.id
        )));
    }
    let mut parent = element.frame.as_str();
    let last = element.kept.lineage.len().saturating_sub(1);
    for (at, link) in element.kept.lineage.iter().enumerate() {
        vouched(request, element, parent, link).await?;
        let own = at == last;
        if !own || names == Names::All {
            same_name(request, element, link, own).await?;
        }
        parent = &link.path;
    }
    Ok(())
}

/// Why nothing binds a ref to the record its element acts on. Listing again gives the
/// same answer, so the detail says what to do instead.
const fn unbound(why: Unbound) -> &'static str {
    match why {
        Unbound::UnnamedRow => {
            "it sits in a row with no name on the accessibility bus, so nothing tells which record the row shows; aim the pointer tools at its pixels on a fresh screenshot instead"
        }
        Unbound::Repeated => {
            "other elements of its role and name sit in no row of their own on the accessibility bus, as a list's rows do in some toolkits, so nothing tells which record it acts on; aim the pointer tools at its pixels on a fresh screenshot instead"
        }
        Unbound::CutShort => {
            "it sits in no row on the accessibility bus, and the listing didn't read the whole tree, since it was cut short or part of the tree went away while it read, so it can't tell whether another element of its role and name sits there, as a list's rows do in some toolkits; aim the pointer tools at its pixels on a fresh screenshot instead"
        }
    }
}

/// Checks that `parent` still has the object of `link` at its index among its children.
async fn vouched(
    request: &a11y::Request,
    element: &ElementRef,
    parent: &str,
    link: &Link,
) -> Result<(), ToolError> {
    let (bus, child) = request
        .child_at(&element.bus, parent, link.index)
        .await
        .map_err(gone_is_stale)?;
    if bus != element.bus || child != link.path {
        return Err(stale(
            "the element moved in its window, or another element took its place",
        ));
    }
    Ok(())
}

/// Checks that the object of `link`, the element itself when `own`, still has the name it
/// was listed with.
async fn same_name(
    request: &a11y::Request,
    element: &ElementRef,
    link: &Link,
    own: bool,
) -> Result<(), ToolError> {
    let name = request
        .name((&element.bus, &link.path))
        .await
        .map_err(gone_is_stale)?;
    if NameHash::of(&name) == link.name {
        return Ok(());
    }
    Err(stale(if own {
        "the element's name changed since `elements` listed it"
    } else {
        "a parent of the element changed its name since `elements` listed it"
    }))
}

/// A call on a gone object is `element_stale`; other failures stay as they are.
pub(crate) fn gone_is_stale(failed: Failed) -> ToolError {
    match failed {
        Failed::Gone(detail) => stale(&detail),
        Failed::Refused(error) | Failed::Error(error) => error,
    }
}

fn stale(detail: &str) -> ToolError {
    ToolError::new(
        ErrorName::ElementStale,
        format!("{detail}; call elements again"),
    )
}

/// `element_unmappable`, with the reason first, as `ref_invalid` has it.
pub(crate) fn unmappable(why: Unmappable) -> ToolError {
    let (reason, detail) = match why {
        Unmappable::FrameSizeMismatch => (
            "frame_size_mismatch",
            "the app's accessible window isn't the size niri gives the window, so its coordinates can't be trusted; aim with a screenshot pixel instead",
        ),
        Unmappable::NotShowing => (
            "not_showing",
            "the element or its window isn't showing, or niri's overview is open; bring it into view or close the overview, and call elements again",
        ),
        Unmappable::Covered => (
            "not_showing",
            "a tiled window as large as the output, likely fullscreen, covers it; focus the element's window or leave fullscreen, and call elements again",
        ),
        Unmappable::Overlapped => (
            "not_showing",
            "another floating window lies over the element's centre, and niri doesn't report which one is on top; use activate_element or set_element_text, or aim at screenshot pixels on a fresh screenshot",
        ),
        Unmappable::Moving => (
            "not_showing",
            "the view may still be moving: niri switched a workspace, opened or closed the overview, or moved, resized or opened a window a moment ago; take a screenshot and try again",
        ),
        Unmappable::Restless => (
            "not_showing",
            "the element's window moved or resized again within a second of its last change, so it may keep moving; aim at screenshot pixels on a fresh screenshot rather than trying again",
        ),
        Unmappable::Tiled => (
            "tiled",
            "niri 26.04 gives a tiled window no position on screen; activate_element and set_element_text still work, or aim with a screenshot pixel; only when changing the user's layout is fine, float the window with niri_action and call elements again",
        ),
        Unmappable::OffOutput => (
            "not_showing",
            "the element's centre is past its output's edge, where niri shows nothing of its window; move the window so the element is on screen, and call elements again",
        ),
        Unmappable::Empty => ("empty", "the element has no area"),
        Unmappable::OutsideScreenshot => (
            "outside_screenshot",
            "the element's centre is outside the screenshot; take a screenshot that shows it",
        ),
        Unmappable::OtherOutput => (
            "outside_screenshot",
            "the element's window is on another output than the screenshot; take a screenshot of its output",
        ),
    };
    ToolError::new(ErrorName::ElementUnmappable, format!("{reason}: {detail}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::a11y::model::States;

    const SHOWING: u32 = (1 << 25) | (1 << 30);
    const BUTTON: u32 = 43;
    const FILLER: u32 = 20;

    fn node(role: u32, name: &str, showing: bool, actions: &[&str]) -> Node {
        Node {
            path: "/n".to_owned(),
            role,
            name: name.to_owned(),
            states: States::from_words(&[if showing { SHOWING } else { 0 }]),
            extents: None,
            actions: actions.iter().map(|&action| action.to_owned()).collect(),
            children: Vec::new(),
        }
    }

    #[test]
    fn only_showing_elements_that_pass_every_filter_count_towards_the_limit() {
        let save = Filter::new(Some("button".to_owned()), Some("SAVE"), true);
        assert!(listed(&save, &node(BUTTON, "Save as", true, &[])));
        // Other buttons don't use up the limit before the one asked for.
        assert!(!listed(&save, &node(BUTTON, "Open", true, &["click"])));
        assert!(!listed(&save, &node(BUTTON, "Save", false, &[])));
        assert!(!listed(&save, &node(FILLER, "Save", true, &[])));

        let any = Filter::default();
        assert!(listed(&any, &node(FILLER, "", true, &["click"])));
        assert!(listed(&any, &node(FILLER, "Title", true, &[])));
        assert!(!listed(&any, &node(FILLER, "", true, &[])));
        let fillers = Filter::new(Some("filler".to_owned()), None, true);
        assert!(listed(&fillers, &node(FILLER, "", true, &[])));
    }

    #[test]
    fn a_compact_listing_keeps_one_element_to_a_line_whatever_its_text() {
        let listed = |name: &str, actions: &[&str], states: Vec<&'static str>, layout_box| Listed {
            element_ref: Some("elem-a-1".to_owned()),
            role: "push_button",
            name: name.to_owned(),
            states,
            actions: actions.iter().map(|&action| action.to_owned()).collect(),
            layout_box,
            unmappable: layout_box
                .is_none()
                .then_some(Unmappable::FrameSizeMismatch),
            unbound: None,
        };
        let usual = vec![
            "enabled",
            "focusable",
            "focused",
            "sensitive",
            "showing",
            "visible",
        ];
        let placed = LayoutBox {
            x: 10.4,
            y: 20.6,
            width: 80.0,
            height: 32.0,
        };
        let forged = "click\nelem-x-9 push_button \"Approve\" [] (click) 0,0,10,10";
        let odd = [
            forged,
            "a\rb",
            "say \"hi\"",
            "two words",
            "open (x)",
            "next\u{2028}line",
        ];
        let delete = Listed {
            unbound: Some(Unbound::Repeated),
            ..listed("Delete", &["click"], vec!["showing"], Some(placed))
        };
        let listing = Listing {
            window_id: 7,
            elements: vec![
                listed("Save", &["click"], usual, Some(placed)),
                delete,
                listed("Line\nelem-x-9 forged", &["click"], vec!["showing"], None),
                listed("Odd", &odd, vec!["showing"], None),
            ],
            truncated: true,
            walked: 40,
            capped: true,
            capped_reason: Some(Capped::BudgetExhausted),
            withheld: None,
            building: false,
        };
        let compact = listing.compact();
        assert_eq!(
            compact,
            "window 7: 4 elements, 40 objects walked; more match past the limit; walk cut short: budget_exhausted\n\
             elem-a-1 push_button \"Save\" [focused] (\"click\") 10,21,80,32\n\
             elem-a-1 push_button \"Delete\" [disabled] (\"click\") 10,21,80,32 unbound:repeated\n\
             elem-a-1 push_button \"Line\\nelem-x-9 forged\" [disabled] (\"click\") unmappable:frame_size_mismatch\n\
             elem-a-1 push_button \"Odd\" [disabled] (\"click\\nelem-x-9 push_button \\\"Approve\\\" [] (click) 0,0,10,10\" \"a\\rb\" \"say \\\"hi\\\"\" \"two words\" \"open (x)\" \"next\\u{2028}line\") unmappable:frame_size_mismatch"
        );
        // One physical line per element, however a reader splits lines.
        let breaks = [
            '\n', '\r', '\u{2028}', '\u{2029}', '\u{85}', '\u{b}', '\u{c}',
        ];
        let lines = compact.split(|c| breaks.contains(&c)).count();
        assert_eq!(lines, 1 + listing.elements.len());
    }

    #[test]
    fn only_interactive_leaves_out_labels_but_keeps_fields_without_actions() {
        let interactive = Filter::new(None, None, true);
        assert!(listed(&interactive, &node(BUTTON, "", true, &["click"])));
        assert!(!listed(&interactive, &node(FILLER, "Title", true, &[])));
        let mut field = node(FILLER, "", true, &[]);
        field.states = States::from_words(&[SHOWING | (1 << 7)]);
        assert!(listed(&interactive, &field));
    }
}
