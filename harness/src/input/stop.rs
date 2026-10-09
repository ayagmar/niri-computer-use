//! The stop flag during input (plan §11): a stop cancels the running call, a drag's button
//! is released on the way out, and a `wtype` already typing finishes. Either way the
//! input-dirty marker is gone afterwards, and the lease can be taken again after `resume`.

use serde_json::json;

use super::{Shot, WAIT, Wev};
use crate::failure::{Context as _, Failure, Result};
use crate::keyboard;
use crate::mcp::{Client, field, structured};
use crate::session::Session;
use crate::wev::{self, Pointer};

/// A stop has to land while the drag runs, about a third of a second; a stop that lands
/// after it proves nothing, so the check tries again.
const ATTEMPTS: usize = 3;
const TEXT: &str = "abcdefghij";

pub(super) fn run(
    session: &mut Session<'_>,
    client: &mut Client,
    wev: &Wev<'_>,
    server: &str,
) -> Result<()> {
    mid_drag(session, client, wev, server)?;
    mid_typing(session, client, wev, server)
}

fn mid_drag(
    session: &mut Session<'_>,
    client: &mut Client,
    wev: &Wev<'_>,
    server: &str,
) -> Result<()> {
    for attempt in 1..=ATTEMPTS {
        let shot = Shot::take(session, client, 4000)?;
        let from = shot.pixel(wev.in_layout((50.0, 50.0)));
        let to = shot.pixel(wev.in_layout((350.0, 250.0)));
        let offset = wev.offset()?;
        let id = client.start_call(
            "drag",
            json!({
                "screenshot_ref": shot.id,
                "from": {"x": from.0, "y": from.1},
                "to": {"x": to.0, "y": to.1}
            }),
        )?;
        session.wait_until("m4-drag-press", "the drag's press in wev", WAIT, |_| {
            Ok(wev
                .since(offset)?
                .contains(&Pointer::Button {
                    code: 272,
                    pressed: true,
                })
                .then_some(()))
        })?;
        stop(session, server)?;
        let result = client.result(session, id)?;
        let stopped = field(&result, "/structuredContent/error") == "stopped";
        released_and_clear(session, wev, offset)?;
        resume(session, client, server)?;
        if stopped {
            return session.log(&format!(
                "M4 stop mid-drag (attempt {attempt}): stopped, button released, marker gone, lease taken again"
            ));
        }
        session.log(&format!(
            "M4 stop mid-drag attempt {attempt}: the drag finished first: {result}"
        ))?;
    }
    Err(Failure::new(format!(
        "M4 stop mid-drag: no stop landed during the drag in {ATTEMPTS} attempts"
    )))
}

/// `wev` saw the left button released after `offset`, and the marker is gone.
fn released_and_clear(session: &mut Session<'_>, wev: &Wev<'_>, offset: usize) -> Result<()> {
    session.wait_until("m4-drag-release", "the release in wev", WAIT, |_| {
        Ok(wev
            .since(offset)?
            .contains(&Pointer::Button {
                code: 272,
                pressed: false,
            })
            .then_some(()))
    })?;
    marker_gone(session)
}

/// A stop while `wtype` types: the call says `stopped`, and `wtype` still types the whole
/// text, then removes the marker.
fn mid_typing(
    session: &mut Session<'_>,
    client: &mut Client,
    wev: &Wev<'_>,
    server: &str,
) -> Result<()> {
    let text = TEXT.repeat(10);
    let offset = wev.offset()?;
    let id = client.start_call(
        "type_text",
        json!({"text": text, "expect": {"app_id": "wev"}}),
    )?;
    session.wait_until("m4-typing", "the first key in wev", WAIT, |_| {
        let seen = keyboard::since(wev.log, offset)?;
        Ok((!wev::keyboard::trace(&seen)?.keys.is_empty()).then_some(()))
    })?;
    stop(session, server)?;
    let result = client.result(session, id)?;
    if field(&result, "/structuredContent/error") != "stopped" {
        return Err(Failure::new(format!(
            "M4 stop mid-typing: expected stopped, saw {result}"
        )));
    }
    let seen = keyboard::observed(session, wev.log, offset, text.chars().count(), false)?;
    keyboard::text(&wev::keyboard::trace(&seen)?, &text)?;
    marker_gone(session)?;
    resume(session, client, server)?;
    session.log("M4 stop mid-typing: stopped, wtype typed all 100 characters, marker gone, lease taken again")
}

fn stop(session: &Session<'_>, server: &str) -> Result<()> {
    session.run(server, &["stop".into()]).map(drop)
}

pub(super) fn marker_gone(session: &mut Session<'_>) -> Result<()> {
    let marker = session.control_dir()?.join("input-dirty");
    session.wait_until("m4-marker", "the input-dirty marker removed", WAIT, |_| {
        Ok((!marker.try_exists().context("check the marker")?).then_some(()))
    })
}

/// Clears the stop and takes the lease again, once the server has seen the resume.
fn resume(session: &mut Session<'_>, client: &mut Client, server: &str) -> Result<()> {
    session.run(server, &["resume".into()])?;
    session.wait_until("m4-resume", "the lease taken again", WAIT, |session| {
        let result = client.call(session, "acquire_desktop", json!({}))?;
        Ok(structured(&result).ok().map(drop))
    })?;
    Ok(())
}
