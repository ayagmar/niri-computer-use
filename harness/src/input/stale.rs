//! A click aimed through a screenshot taken before the window it shows moved, as when the
//! user moves or resizes a window while the agent looks at its last screenshot: the
//! harness moves the floating `wev` 250 px right with niri's own action, and a click at the
//! pixel that showed `wev`'s centre must fail with `ref_invalid` `layout_changed` and reach
//! nothing. `wev` then moves back where the other checks expect it.

use std::time::Duration;

use niri_ipc::{Action, PositionChange, Request, Response};
use serde_json::json;

use super::{Shot, WAIT, Wev, wev_window};
use crate::failure::{Failure, Result};
use crate::mcp::{Client, field, structured};
use crate::session::Session;
use crate::wev::Pointer;

/// How far the harness moves `wev`, in logical pixels: past half its 400 px width, so its
/// old centre is off it.
const SHIFT: f64 = 250.0;

pub(super) fn run(session: &mut Session<'_>, client: &mut Client, wev: &Wev<'_>) -> Result<()> {
    let id = wev_window(session, client)?;
    let shot = Shot::take(session, client, 4000)?;
    let pixel = shot.pixel(wev.in_layout((200.0, 150.0)));
    shift(session, id, SHIFT)?;
    let offset = wev.offset()?;
    let arguments = json!({"screenshot_ref": shot.id, "x": pixel.0, "y": pixel.1});
    let result = client.call(session, "click", arguments)?;
    let error = field(&result, "/structuredContent/error");
    let detail = field(&result, "/structuredContent/detail")
        .as_str()
        .unwrap_or_default()
        .to_owned();
    session.log(&format!(
        "M4 stale ref: click after wev moved {SHIFT} px: {result}"
    ))?;
    if error != "ref_invalid" || !detail.starts_with("layout_changed") {
        return Err(Failure::new(format!(
            "M4 stale ref: expected ref_invalid layout_changed, saw {result}"
        )));
    }
    session.still_absent("m4-stale", Duration::from_secs(1), || {
        let trace = wev.since(offset)?;
        Ok(trace
            .iter()
            .any(|event| matches!(event, Pointer::Button { .. })))
    })?;
    shift(session, id, -SHIFT)?;
    structured(&client.call(
        session,
        "wait_for",
        json!({"until": "screen_stable", "timeout_ms": 3000}),
    )?)?;
    session.log("M4 stale ref: refused, nothing reached wev; wev moved back")
}

/// Moves floating window `id` by `dx` and waits until niri reports it there.
fn shift(session: &mut Session<'_>, id: u64, dx: f64) -> Result<()> {
    let (x, y) = place(session, id)?;
    session.request(&Request::Action(Action::MoveFloatingWindow {
        id: Some(id),
        x: PositionChange::AdjustFixed(dx),
        y: PositionChange::AdjustFixed(0.0),
    }))?;
    session.wait_until("m4-stale-move", "wev at its new place", WAIT, |session| {
        Ok((place(session, id)? == (x + dx, y)).then_some(()))
    })
}

/// Where niri reports floating window `id` in its workspace's view.
fn place(session: &mut Session<'_>, id: u64) -> Result<(f64, f64)> {
    let Response::Windows(windows) = session.request(&Request::Windows)? else {
        return Err(Failure::new("M4 stale ref: no windows reply"));
    };
    windows
        .iter()
        .find(|window| window.id == id)
        .and_then(|window| window.layout.tile_pos_in_workspace_view)
        .ok_or_else(|| Failure::new(format!("M4 stale ref: window {id} has no floating place")))
}
