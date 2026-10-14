//! Counts input reaching a second nested wev after a deliberate focus takeover.

use std::ffi::OsString;

use niri_ipc::{Action, Request, Response};
use serde_json::json;

use super::{WAIT, WEV_DEADLINE, Wev};
use crate::failure::{Failure, Result};
use crate::keyboard;
use crate::mcp::{Client, field, structured};
use crate::session::Session;
use crate::wev::keyboard::trace;

pub(super) fn run(session: &mut Session<'_>, client: &mut Client, wev: &Wev<'_>) -> Result<()> {
    let state = structured(&client.call(session, "desktop_state", json!({}))?)?;
    let original = field(&state, "/focused_window")
        .as_u64()
        .ok_or_else(|| Failure::new("M7 source wev not focused"))?;
    let observer_log = session.artifact("wev-focus.log");
    let args = ["-oL", "wev"].map(OsString::from);
    let observer = session.start("stdbuf", &args, observer_log.clone(), WEV_DEADLINE)?;
    let other = session.wait_until("m7-observer", "the second wev window", WAIT, |session| {
        let Response::Windows(windows) = session.request(&Request::Windows)? else {
            return Err(Failure::new("no nested windows reply"));
        };
        Ok(windows
            .iter()
            .find(|window| window.id != original && window.app_id.as_deref() == Some("wev"))
            .map(|window| window.id))
    })?;
    let initial_map = session.wait_until(
        "m7-observer-map",
        "the observer's initial keymap",
        WAIT,
        |_| {
            Ok(trace(&keyboard::read(&observer_log)?)?
                .keymaps
                .last()
                .copied())
        },
    )?;
    structured(&client.call(session, "focus", json!({"window_id": original}))?)?;
    let offset = wev.offset()?;
    let other_offset = keyboard::offset(&observer_log)?;
    let id = client.start_call(
        "type_text",
        json!({"text": "a".repeat(1000), "submit": true, "expect": {"window_id": original}}),
    )?;
    session.wait_until("m7-focus-first", "typing in the first wev", WAIT, |_| {
        Ok((!trace(&keyboard::since(wev.log, offset)?)?.keys.is_empty()).then_some(()))
    })?;
    session.request(&Request::Action(Action::FocusWindow { id: other }))?;
    let result = structured(&client.result(session, id)?)?;
    if field(&result, "/observed") != "interrupted" || field(&result, "/submitted") != false {
        return Err(Failure::new(format!(
            "M7 native failed focus takeover: {result}"
        )));
    }
    let seen = keyboard::since(&observer_log, other_offset)?;
    if trace(&keyboard::read(&observer_log)?)?
        .keymaps
        .iter()
        .any(|map| *map != initial_map)
    {
        return Err(Failure::new(
            "M7 native changed the observer's keymap identity",
        ));
    }
    let source = keyboard::since(wev.log, offset)?;
    let (source_count, landed) = verify_delivery(&source, &seen)?;
    session.log(&format!("M7 native focus takeover: {landed} characters landed in second wev after focus change, {source_count} in original, no Return; {} tool-completed characters", field(&result, "/typed")))?;
    super::stop::marker_gone(session)?;
    observer.stop()?;
    structured(&client.call(session, "focus", json!({"window_id": original}))?)?;
    Ok(())
}

/// What reached each wev: every key typed `a`, and at most one after the takeover.
fn verify_delivery(source: &str, seen: &str) -> Result<(usize, usize)> {
    let original = trace(source)?;
    let target = trace(seen)?;
    let source_count = original.keys.iter().filter(|key| key.pressed).count();
    let landed = target.keys.iter().filter(|key| key.pressed).count();
    if landed > 1 {
        return Err(Failure::new(format!(
            "M7 native sent {landed} characters after focus takeover"
        )));
    }
    keyboard::text(&original, &"a".repeat(source_count))?;
    keyboard::text(&target, &"a".repeat(landed))?;
    Ok((source_count, landed))
}
