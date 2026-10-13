//! M9's acceptance through the server: `elements` lists the fixtures' buttons and the
//! pointer tools aim at them with `element`, through the unmodified pointer path. Each
//! activation is counted by the fixture itself, so a click that lands elsewhere, or twice,
//! fails the run. M9b's element actions follow, in `actions`. The nested Noctalia is the
//! lock source, as in M4.

mod actions;
mod gate;
mod lists;

use std::fs;
use std::time::{Duration, Instant};

use niri_ipc::{Action, PositionChange, Request, Response, Window};
use serde_json::{Value, json};

use super::{Bus, Fixture, Toolkit};
use crate::config::Decorations;
use crate::failure::{Context as _, Failure, Result};
use crate::mcp::{self, Client, field, structured};
use crate::session::{Session, pause};

/// Within what is left of the run's deadline.
const NOCTALIA_DEADLINE: Duration = Duration::from_secs(225);
const SERVER_DEADLINE: Duration = Duration::from_secs(220);
const READY: Duration = Duration::from_secs(20);
const COUNTED: Duration = Duration::from_secs(2);
const WAIT: Duration = Duration::from_secs(5);
/// The acceptance rule: every one of a hundred clicks activates the button once.
const ROUNDS: u32 = 100;
/// Screenshot refs last 60 s; a new one is taken well before.
const SHOT_AGE: Duration = Duration::from_secs(30);
/// How long a refused click has to show up as an activation anyway.
const SETTLE: Duration = Duration::from_millis(500);
/// Longer than the server refuses element aims on an output after niri switches its
/// workspace or opens or closes the overview.
const VIEW_SETTLE: Duration = Duration::from_millis(1100);
/// The start of a refusal's detail for an element whose window isn't shown.
const UNSHOWN: &str = "not_showing: the element or its window isn't showing";
/// The server's budget for a request on the accessibility bus.
const BUDGET: Duration = Duration::from_secs(3);
/// How soon a closed window's element must be refused, by the server's own clock.
const STALE_WITHIN_MS: u64 = 50;

pub(super) fn run(
    session: &mut Session<'_>,
    bus: &Bus,
    server: &str,
    decorations: Decorations,
) -> Result<()> {
    let noctalia = session.start(
        "noctalia",
        &[],
        session.artifact("noctalia.log"),
        NOCTALIA_DEADLINE,
    )?;
    policy(session)?;
    let client = Client::start(session, server, "harness-m9", SERVER_DEADLINE)?;
    let mut checks = Checks {
        session,
        client,
        shot: None,
    };
    checks.ready()?;
    checks.gtk4(bus, decorations)?;
    checks.gtk3(bus, decorations)?;
    checks.element_actions(bus, server)?;
    checks.call("release_desktop", json!({"restore_focus": false}))?;
    checks.client.stop()?;
    noctalia.stop().map(drop)
}

/// The policy file: the `app_id` the GTK 4 fixture can rename itself to is denied.
fn policy(session: &Session<'_>) -> Result<()> {
    let dir = session.test_dir().config().join("niri-computer-use");
    fs::create_dir_all(&dir).context(format!("create {}", dir.display()))?;
    let policy = format!("deny_input_app_ids = [\"{}\"]\n", actions::DENIED);
    let path = dir.join("policy.toml");
    fs::write(&path, &policy).context(format!("write {}", path.display()))?;
    fs::write(session.artifact("policy.toml"), policy).context("copy the policy file")
}

/// The server and the screenshot ref the clicks go through.
struct Checks<'a, 'b> {
    session: &'a mut Session<'b>,
    client: Client,
    shot: Option<(String, Instant)>,
}

impl Checks<'_, '_> {
    /// The server sees the accessibility bus, lists `elements`, and holds the lease.
    fn ready(&mut self) -> Result<()> {
        let status = mcp::ready(self.session, &mut self.client, "m9-ready", READY)?;
        let accessibility = field(&status, "/accessibility");
        expect(
            field(accessibility, "/available") == true,
            "status.accessibility.available",
            accessibility,
        )?;
        let tools = self.client.tools(self.session)?;
        if !tools.iter().any(|tool| tool == "elements") {
            return Err(Failure::new(format!("M9: no elements tool in {tools:?}")));
        }
        self.call("acquire_desktop", json!({}))?;
        self.session
            .log(&format!("M9: server ready, accessibility {accessibility}"))
    }

    /// GTK 4 fits its frame with either decoration: a hundred activations, one after its
    /// window moved, a removed widget, a stopped Qt app beside it, then its window closed.
    fn gtk4(&mut self, bus: &Bus, decorations: Decorations) -> Result<()> {
        let fixture = Fixture::start(self.session, Toolkit::Gtk4)?;
        let window = fixture.window(self.session, bus)?;
        let primary = self.button(window.id, "Primary")?;
        self.activations(&primary, "primary-count", 0)?;
        self.moved(&window, &primary)?;
        self.tiled(window.id, &primary)?;
        self.overview(window.id)?;
        self.unshown(window.id, &primary)?;
        self.covered(window.id)?;
        self.vanished(window.id)?;
        let qt = Fixture::start(self.session, Toolkit::Qt)?;
        let qt_window = qt.window(self.session, bus)?;
        self.stopped(&qt, qt_window.id, window.id)?;
        self.toolkit(Toolkit::Qt, qt_window.id, decorations)?;
        qt.stop()?;
        self.closed(window.id, &primary)?;
        fixture.stop()
    }

    fn gtk3(&mut self, bus: &Bus, decorations: Decorations) -> Result<()> {
        let fixture = Fixture::start(self.session, Toolkit::Gtk3)?;
        let window = fixture.window(self.session, bus)?;
        self.toolkit(Toolkit::Gtk3, window.id, decorations)?;
        fixture.stop()
    }

    /// GTK 3 and Qt: a hundred activations with server-side decorations; with their own,
    /// the frame doesn't fit, and a click is refused without activating anything.
    fn toolkit(&mut self, toolkit: Toolkit, window: u64, decorations: Decorations) -> Result<()> {
        let (name, counter) = match toolkit {
            Toolkit::Gtk3 => ("Gtk3", "gtk3-count"),
            Toolkit::Qt => ("Qt", "qt-count"),
            Toolkit::Gtk4 => ("Primary", "primary-count"),
        };
        match decorations {
            Decorations::Server => {
                let button = self.button(window, name)?;
                self.activations(&button, counter, 0)
            }
            Decorations::Client => self.unmappable(window, name, counter),
        }
    }

    /// Clicks `button` a hundred times, each counted once by the fixture, starting from
    /// `from` activations. niri may have opened or placed the window a moment ago, so the
    /// first click waits for the view to settle.
    fn activations(&mut self, button: &Listed, counter: &str, from: u32) -> Result<()> {
        pause(VIEW_SETTLE);
        let started = Instant::now();
        for count in from + 1..=from + ROUNDS {
            self.click(&button.id)?;
            self.counted(counter, count)?;
        }
        self.session.log(&format!(
            "M9: {ROUNDS}/{ROUNDS} activations of {} at {:?}, in {:?}",
            button.id,
            button.layout_box,
            started.elapsed()
        ))
    }

    /// niri moves the floating window. Right away the view may still be moving, so the
    /// element listed before is refused; a moment later it hits, placed from niri's geometry
    /// now.
    fn moved(&mut self, window: &Window, primary: &Listed) -> Result<()> {
        let before = window.layout.tile_pos_in_workspace_view;
        self.session
            .request(&Request::Action(Action::MoveFloatingWindow {
                id: Some(window.id),
                x: PositionChange::AdjustFixed(37.0),
                y: PositionChange::AdjustFixed(11.0),
            }))?;
        let id = window.id;
        self.session
            .wait_until("m9-moved", "the moved window", WAIT, |session| {
                let moved = super::window(session, id)?
                    .and_then(|now| now.layout.tile_pos_in_workspace_view)
                    .filter(|&now| Some(now) != before);
                Ok(moved)
            })?;
        self.shot = None;
        let moving = self.refused_click(&primary.id)?;
        expect(
            field(&moving, "/error") == "element_unmappable"
                && field(&moving, "/detail").as_str().is_some_and(|detail| {
                    detail.starts_with("not_showing: the view may still be moving")
                }),
            "element_unmappable: the view may still be moving, just after the move",
            &moving,
        )?;
        pause(VIEW_SETTLE);
        self.click(&primary.id)?;
        self.counted("primary-count", ROUNDS + 1)?;
        self.session.log(&format!(
            "M9: moved by (+37, +11), refused at once: {moving}; a moment later the element listed before hit"
        ))
    }

    /// Tiled, the window has no place niri reports: its elements are `tiled`, and the ref
    /// listed while it floated is refused without a click. Floating again, it hits.
    fn tiled(&mut self, window: u64, primary: &Listed) -> Result<()> {
        self.floating(window, false)?;
        let listed = self.primary_until(window, "Primary unmappable: tiled", |element| {
            field(element, "/unmappable") == "tiled"
        })?;
        expect(field(&listed, "/layout_box").is_null(), "no box", &listed)?;
        let refused = self.refused_click(&primary.id)?;
        expect(
            field(&refused, "/error") == "element_unmappable"
                && field(&refused, "/detail")
                    .as_str()
                    .is_some_and(|detail| detail.starts_with("tiled: ")),
            "element_unmappable: tiled for a click",
            &refused,
        )?;
        let session = &*self.session;
        session.still_absent("m9-tiled-no-activation", SETTLE, || {
            Ok(read_count(session, "primary-count")? != Some(ROUNDS + 1))
        })?;
        self.floating(window, true)?;
        self.primary_until(window, "Primary with a box", |element| {
            !field(element, "/layout_box").is_null()
        })?;
        pause(VIEW_SETTLE);
        let floating = self.button(window, "Primary")?;
        self.shot = None;
        self.click(&floating.id)?;
        self.counted("primary-count", ROUNDS + 2)?;
        self.session.log(&format!(
            "M9: tiled, Primary unmappable and its click refused: {refused}; floating again, it hit"
        ))
    }

    /// With niri's overview open, the workspaces are zoomed out: `Primary` is
    /// `not_showing`, and its ref is refused without a click. Closed again, it hits.
    fn overview(&mut self, window: u64) -> Result<()> {
        self.overview_open(true)?;
        pause(VIEW_SETTLE);
        let listed = self.primary_until(window, "Primary unmappable: not_showing", |element| {
            field(element, "/unmappable") == "not_showing"
        })?;
        let refused = self.refused_click(&element_ref(&listed)?)?;
        expect(
            field(&refused, "/error") == "element_unmappable"
                && field(&refused, "/detail")
                    .as_str()
                    .is_some_and(|detail| detail.starts_with(UNSHOWN)),
            "element_unmappable: not_showing for a click in the overview",
            &refused,
        )?;
        let session = &*self.session;
        session.still_absent("m9-overview-no-activation", SETTLE, || {
            Ok(read_count(session, "primary-count")? != Some(ROUNDS + 2))
        })?;
        self.overview_open(false)?;
        pause(VIEW_SETTLE);
        let closed = self.button(window, "Primary")?;
        self.shot = None;
        self.click(&closed.id)?;
        self.counted("primary-count", ROUNDS + 3)?;
        self.session.log(&format!(
            "M9: overview open, Primary unmappable and its click refused: {refused}; closed, it hit"
        ))
    }

    /// On a workspace that isn't shown, niri still reports the floating window's place, where
    /// the shown workspace is: `Primary` is `not_showing`, and the ref listed while it was
    /// shown is refused without a click. Shown again, the view may still be moving, so the
    /// same ref is refused at once; a moment later it hits.
    fn unshown(&mut self, window: u64, primary: &Listed) -> Result<()> {
        let place = self.workspace_shown(window, false)?;
        pause(VIEW_SETTLE);
        let listed = self.primary_until(window, "Primary unmappable: not_showing", |element| {
            field(element, "/unmappable") == "not_showing"
        })?;
        expect(field(&listed, "/layout_box").is_null(), "no box", &listed)?;
        let refused = self.refused_click(&primary.id)?;
        expect(
            field(&refused, "/error") == "element_unmappable"
                && field(&refused, "/detail")
                    .as_str()
                    .is_some_and(|detail| detail.starts_with(UNSHOWN)),
            "element_unmappable: not_showing for a click on a workspace not shown",
            &refused,
        )?;
        let session = &*self.session;
        session.still_absent("m9-unshown-no-activation", SETTLE, || {
            Ok(read_count(session, "primary-count")? != Some(ROUNDS + 3))
        })?;
        self.workspace_shown(window, true)?;
        let moving = self.refused_click(&primary.id)?;
        expect(
            field(&moving, "/error") == "element_unmappable"
                && field(&moving, "/detail").as_str().is_some_and(|detail| {
                    detail.starts_with("not_showing: the view may still be moving")
                }),
            "element_unmappable: the view may still be moving, just after the switch",
            &moving,
        )?;
        pause(VIEW_SETTLE);
        let shown = self.button(window, "Primary")?;
        self.shot = None;
        self.click(&shown.id)?;
        self.counted("primary-count", ROUNDS + 4)?;
        self.session.log(&format!(
            "M9: on a workspace not shown, niri reported the window at {place:?}; Primary unmappable and its click refused: {refused}; shown again, refused at once: {moving}; a moment later, it hit"
        ))
    }

    /// A fullscreen tiled window, focused on the same workspace, hides the floating one:
    /// `Primary` is `not_showing` because a tiled window as large as the output covers it, and its ref is
    /// refused without a click. With that window closed, it hits.
    fn covered(&mut self, window: u64) -> Result<()> {
        let decoy = Fixture::start_decoy(self.session)?;
        let cover = decoy.decoy_window(self.session)?;
        let id = Some(cover);
        for action in [
            Action::MoveWindowToTiling { id },
            Action::FullscreenWindow { id },
            Action::FocusWindow { id: cover },
        ] {
            self.session.request(&Request::Action(action))?;
        }
        let size = self.session.wait_until(
            "m9-covered",
            "the fullscreen decoy, focused",
            WAIT,
            |session| {
                let Some(now) = super::window(session, cover)? else {
                    return Ok(None);
                };
                let tiled = now.layout.pos_in_scrolling_layout.is_some();
                let filled = now.layout.tile_size.0 > 400.0;
                Ok((tiled && filled && now.is_focused).then_some(now.layout.tile_size))
            },
        )?;
        let listed = self.primary_until(window, "Primary unmappable: not_showing", |element| {
            field(element, "/unmappable") == "not_showing"
        })?;
        let refused = self.refused_click(&element_ref(&listed)?)?;
        expect(
            field(&refused, "/error") == "element_unmappable"
                && field(&refused, "/detail").as_str().is_some_and(|detail| {
                    detail.starts_with("not_showing: a tiled window as large as the output")
                }),
            "element_unmappable: a tiled window as large as the output covers it",
            &refused,
        )?;
        let session = &*self.session;
        session.still_absent("m9-covered-no-activation", SETTLE, || {
            Ok(read_count(session, "primary-count")? != Some(ROUNDS + 4))
        })?;
        decoy.stop()?;
        self.primary_until(window, "Primary with a box", |element| {
            !field(element, "/layout_box").is_null()
        })?;
        let uncovered = self.button(window, "Primary")?;
        self.shot = None;
        self.click(&uncovered.id)?;
        self.counted("primary-count", ROUNDS + 5)?;
        self.session.log(&format!(
            "M9: behind a fullscreen window of tile size {size:?}, Primary unmappable and its click refused: {refused}; uncovered, it hit"
        ))
    }

    /// Focuses the workspace below `window`'s, or back up to it, and waits until niri
    /// reports its workspace shown or not; returns the window's place as niri reports it.
    fn workspace_shown(&mut self, window: u64, shown: bool) -> Result<Option<(f64, f64)>> {
        let action = if shown {
            Action::FocusWorkspaceUp {}
        } else {
            Action::FocusWorkspaceDown {}
        };
        self.session.request(&Request::Action(action))?;
        self.session
            .wait_until("m9-workspace", "the window's workspace", WAIT, |session| {
                let Some(now) = super::window(session, window)? else {
                    return Ok(None);
                };
                let Response::Workspaces(workspaces) = session.request(&Request::Workspaces)?
                else {
                    return Ok(None);
                };
                let active = workspaces
                    .iter()
                    .any(|workspace| Some(workspace.id) == now.workspace_id && workspace.is_active);
                Ok((active == shown).then_some(now.layout.tile_pos_in_workspace_view))
            })
    }

    /// Opens or closes niri's overview, and waits until niri reports it so.
    fn overview_open(&mut self, open: bool) -> Result<()> {
        let action = if open {
            Action::OpenOverview {}
        } else {
            Action::CloseOverview {}
        };
        self.session.request(&Request::Action(action))?;
        self.session
            .wait_until("m9-overview", "the overview", WAIT, |session| {
                let now = session.request(&Request::OverviewState)?;
                Ok(
                    matches!(now, Response::OverviewState(state) if state.is_open == open)
                        .then_some(()),
                )
            })
    }

    /// Lists `window`'s `Primary` until it `holds`, as the window settles in its new
    /// layout; returns the element.
    fn primary_until(
        &mut self,
        window: u64,
        what: &str,
        holds: impl Fn(&Value) -> bool,
    ) -> Result<Value> {
        let client = &mut self.client;
        self.session
            .wait_until("m9-layout-listed", what, WAIT, |session| {
                let listing = elements(session, client, window, "Primary")?;
                let element = field(&listing, "/elements/0");
                Ok(holds(element).then(|| element.clone()))
            })
    }

    /// Moves `window` to the floating layout or the scrolling one, and waits until niri
    /// reports it there.
    fn floating(&mut self, window: u64, floating: bool) -> Result<()> {
        let id = Some(window);
        let action = if floating {
            Action::MoveWindowToFloating { id }
        } else {
            Action::MoveWindowToTiling { id }
        };
        self.session.request(&Request::Action(action))?;
        self.session
            .wait_until("m9-layout", "the window's layout", WAIT, |session| {
                let now = super::window(session, window)?;
                Ok(now.filter(|now| now.is_floating == floating).map(drop))
            })
    }

    /// A button that removes itself: the click's `element_changes` says it was removed,
    /// and once it is gone its ref is stale.
    fn vanished(&mut self, window: u64) -> Result<()> {
        let vanish = self.button(window, "Vanish")?;
        let shot = self.screenshot_ref()?;
        let arguments =
            json!({"screenshot_ref": shot, "element": vanish.id, "element_changes": true});
        let outcome = self.call("click", arguments)?;
        expect(
            field(&outcome, "/observed") == "sent",
            "the click sent",
            &outcome,
        )?;
        let told = "click with element_changes: GTK 4's Vanish button removed itself";
        crate::changes::told(self.session, told, &outcome, |changes| {
            field(changes, "/elements/removed")
                .as_array()
                .is_some_and(|removed| {
                    removed.contains(&json!({"role": "button", "name": "Vanish"}))
                })
        })?;
        let what = "the Vanish button gone from elements";
        self.session
            .wait_until("m9-vanished", what, WAIT, |session| {
                let listing = elements(session, &mut self.client, window, "Vanish")?;
                Ok(field(&listing, "/elements")
                    .as_array()
                    .is_some_and(Vec::is_empty)
                    .then_some(()))
            })?;
        let refused = self.refused_click(&vanish.id)?;
        expect(
            field(&refused, "/error") == "element_stale",
            "element_stale for a removed widget",
            &refused,
        )?;
        self.session
            .log(&format!("M9: removed widget refused: {refused}"))
    }

    /// With the Qt app stopped, `elements` on the GTK window still answers within the
    /// budget, and on Qt's window fails with `deadline_exceeded` within it.
    fn stopped(&mut self, qt: &Fixture, qt_window: u64, gtk_window: u64) -> Result<()> {
        signal(self.session, qt, "-STOP")?;
        let outcome = self.while_stopped(qt_window, gtk_window);
        signal(self.session, qt, "-CONT")?;
        outcome
    }

    fn while_stopped(&mut self, qt_window: u64, gtk_window: u64) -> Result<()> {
        let started = Instant::now();
        let gtk = elements(self.session, &mut self.client, gtk_window, "Primary")?;
        let gtk_took = started.elapsed();
        expect(
            gtk_took < BUDGET && field(&gtk, "/elements/0/role") == "button",
            "the GTK window listed within 3 s while Qt is stopped",
            &json!({"took_ms": gtk_took.as_millis(), "listing": gtk}),
        )?;
        let asked = Instant::now();
        let result = self.client.call(
            self.session,
            "elements",
            json!({"window_id": qt_window, "role": "button"}),
        )?;
        let qt_took = asked.elapsed();
        let error = field(&result, "/structuredContent");
        expect(
            field(error, "/error") == "deadline_exceeded" && qt_took < BUDGET,
            "deadline_exceeded within 3 s for the stopped Qt app",
            &json!({"took_ms": qt_took.as_millis(), "result": result}),
        )?;
        self.session.log(&format!(
            "M9: Qt stopped: GTK listed in {gtk_took:?}, Qt refused in {qt_took:?}: {error}"
        ))
    }

    /// `close_window`, then a click on an element of that window: `element_stale`, within
    /// 50 ms by the audit log's duration.
    fn closed(&mut self, window: u64, primary: &Listed) -> Result<()> {
        let closed = self.call("close_window", json!({"id": window}))?;
        expect(
            field(&closed, "/observed") == "closed",
            "the GTK 4 window closed",
            &closed,
        )?;
        let refused = self.refused_click(&primary.id)?;
        let took = last_duration_ms(self.session)?;
        expect(
            field(&refused, "/error") == "element_stale" && took < STALE_WITHIN_MS,
            "element_stale within 50 ms after close_window",
            &json!({"duration_ms": took, "refused": refused}),
        )?;
        self.session.log(&format!(
            "M9: closed window's element refused in {took} ms: {refused}"
        ))
    }

    /// With client-side decorations the frame doesn't fit: no box, the click is refused,
    /// and the fixture counts nothing.
    fn unmappable(&mut self, window: u64, name: &str, counter: &str) -> Result<()> {
        let listing = elements(self.session, &mut self.client, window, name)?;
        let element = field(&listing, "/elements/0");
        expect(
            field(element, "/layout_box").is_null()
                && field(element, "/unmappable") == "frame_size_mismatch",
            "frame_size_mismatch with client-side decorations",
            element,
        )?;
        let id = element_ref(element)?;
        let refused = self.refused_click(&id)?;
        expect(
            field(&refused, "/error") == "element_unmappable"
                && field(&refused, "/detail")
                    .as_str()
                    .is_some_and(|detail| detail.starts_with("frame_size_mismatch")),
            "element_unmappable for a click",
            &refused,
        )?;
        let session = &*self.session;
        session.still_absent("m9-no-activation", SETTLE, || {
            Ok(read_count(session, counter)? != Some(0))
        })?;
        self.session
            .log(&format!("M9: {name} unmappable, click refused: {refused}"))
    }

    /// The one button named `name` in `window`, which must have a box.
    fn button(&mut self, window: u64, name: &str) -> Result<Listed> {
        let listing = elements(self.session, &mut self.client, window, name)?;
        let found = field(&listing, "/elements")
            .as_array()
            .cloned()
            .unwrap_or_default();
        let [element] = found.as_slice() else {
            return Err(Failure::new(format!(
                "M9: expected one {name} button in window {window}; saw {listing}"
            )));
        };
        let layout_box = field(element, "/layout_box").clone();
        expect(!layout_box.is_null(), "a layout box", element)?;
        self.session.log(&format!(
            "M9: window {window} {name}: {} of {} objects walked",
            field(element, "/role"),
            field(&listing, "/walked")
        ))?;
        Ok(Listed {
            id: element_ref(element)?,
            layout_box,
        })
    }

    /// Clicks element `id`, which must be `sent`.
    fn click(&mut self, id: &str) -> Result<()> {
        let shot = self.screenshot_ref()?;
        let outcome = self.call("click", json!({"screenshot_ref": shot, "element": id}))?;
        expect(
            field(&outcome, "/observed") == "sent",
            "the click sent",
            &outcome,
        )
    }

    /// Clicks element `id`, which must fail; returns the error.
    fn refused_click(&mut self, id: &str) -> Result<Value> {
        let shot = self.screenshot_ref()?;
        let result = self.client.call(
            self.session,
            "click",
            json!({"screenshot_ref": shot, "element": id}),
        )?;
        expect(
            field(&result, "/isError") == true,
            "a refused click",
            &result,
        )?;
        Ok(field(&result, "/structuredContent").clone())
    }

    /// The focused output's latest screenshot ref, renewed before it gets old.
    fn screenshot_ref(&mut self) -> Result<String> {
        if let Some((id, taken)) = &self.shot
            && taken.elapsed() < SHOT_AGE
        {
            return Ok(id.clone());
        }
        let metadata = self.call("screenshot", json!({"target": "focused_output"}))?;
        let id = field(&metadata, "/screenshot_ref")
            .as_str()
            .ok_or_else(|| Failure::new(format!("M9: no screenshot_ref in {metadata}")))?
            .to_owned();
        self.shot = Some((id.clone(), Instant::now()));
        Ok(id)
    }

    /// Waits until the fixture's counter reads `count`.
    fn counted(&mut self, counter: &str, count: u32) -> Result<()> {
        let what = format!("{counter} at {count}");
        self.session
            .wait_until("m9-counted", &what, COUNTED, |session| {
                Ok((read_count(session, counter)? == Some(count)).then_some(()))
            })
    }

    fn call(&mut self, tool: &str, arguments: Value) -> Result<Value> {
        structured(&self.client.call(self.session, tool, arguments)?)
    }
}

/// A listed element: its ref and its box.
#[derive(Debug)]
struct Listed {
    id: String,
    layout_box: Value,
}

/// `elements` for the buttons of `window` whose name contains `name`.
fn elements(
    session: &mut Session<'_>,
    client: &mut Client,
    window: u64,
    name: &str,
) -> Result<Value> {
    let arguments = json!({
        "window_id": window, "role": "button", "name_contains": name, "format": "json",
    });
    structured(&client.call(session, "elements", arguments)?)
}

fn element_ref(element: &Value) -> Result<String> {
    field(element, "/element_ref")
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| Failure::new(format!("M9: no element_ref in {element}")))
}

/// Sends `fixture` a signal through `kill`.
fn signal(session: &Session<'_>, fixture: &Fixture, signal: &str) -> Result<()> {
    session
        .run("kill", &[signal.into(), fixture.pid().to_string().into()])
        .map(drop)
}

/// A fixture's activation count, once it wrote one.
fn read_count(session: &Session<'_>, counter: &str) -> Result<Option<u32>> {
    let path = session.test_dir().root().join(counter);
    match fs::read_to_string(&path) {
        Ok(text) => count(&text).context(format!("parse {}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(Failure::new(format!("read {}: {error}", path.display()))),
    }
}

/// The count in a counter file's `text`. The Qt fixture writes its counter with an
/// `XMLHttpRequest` `PUT`, which empties the file before writing it, so an empty file is
/// one being written: no count yet.
fn count(text: &str) -> std::result::Result<Option<u32>, std::num::ParseIntError> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    text.parse().map(Some)
}

/// The duration of the server's last logged call, from its audit log, which is kept with
/// the artifacts.
fn last_duration_ms(session: &Session<'_>) -> Result<u64> {
    let path = session
        .test_dir()
        .state()
        .join("niri-computer-use/audit.jsonl");
    let audit = fs::read_to_string(&path).context(format!("read {}", path.display()))?;
    fs::write(session.artifact("audit.jsonl"), &audit).context("copy the audit log")?;
    let last: Value = audit
        .lines()
        .last()
        .map(serde_json::from_str)
        .transpose()
        .context("parse the audit log's last line")?
        .unwrap_or(Value::Null);
    field(&last, "/duration_ms")
        .as_u64()
        .ok_or_else(|| Failure::new(format!("M9: no duration_ms in {last}")))
}

fn expect(holds: bool, what: &str, saw: &Value) -> Result<()> {
    if holds {
        Ok(())
    } else {
        Err(Failure::new(format!("M9: expected {what}; saw {saw}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_counter_file_caught_while_qt_rewrites_it_has_no_count_yet() {
        assert_eq!(count(""), Ok(None));
        assert_eq!(count("12\n"), Ok(Some(12)));
        assert!(count("1x").is_err());
    }
}
