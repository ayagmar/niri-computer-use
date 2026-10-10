//! The two-server crash (plan §13): server A, or in shared mode the engine, is killed
//! mid-`type_text` and mid-drag.
//! Server B must refuse with `recovery_required` until the user's `recover` has run, which
//! finds `wtype` already done in the first case and sends the button's release from a
//! fresh pointer in the second. In the second, A's crash guardian has already released
//! the button, without `recover`.

use serde_json::json;

use super::{SERVER_DEADLINE, Shot, WAIT, Wev, stop};
use crate::failure::{Failure, Result};
use crate::keyboard;
use crate::mcp::{Client, field, structured};
use crate::session::Session;
use crate::wev::{self, Pointer};

const TEXT: &str = "0123456789";

pub(super) fn run(session: &mut Session<'_>, wev: &Wev<'_>, server: &str) -> Result<()> {
    mid_typing(session, wev, server)?;
    mid_drag(session, wev, server)
}

/// Starts server A with the lease.
fn server_a(session: &mut Session<'_>, server: &str, name: &str) -> Result<Client> {
    let mut a = Client::start(session, server, name, SERVER_DEADLINE)?;
    structured(&a.call(session, "acquire_desktop", json!({}))?)?;
    Ok(a)
}

/// Server B's `acquire_desktop` must name the marker.
fn refused(session: &mut Session<'_>, server: &str, name: &str) -> Result<Client> {
    let mut b = Client::start(session, server, name, SERVER_DEADLINE)?;
    let result = b.call(session, "acquire_desktop", json!({}))?;
    if field(&result, "/structuredContent/error") != "recovery_required" {
        return Err(Failure::new(format!(
            "M4 crash: server B expected recovery_required, saw {result}"
        )));
    }
    Ok(b)
}

/// `recover` with a `yes`, which must say `said`; then B takes the lease.
pub(super) fn recover(
    session: &mut Session<'_>,
    b: &mut Client,
    server: &str,
    said: &str,
) -> Result<()> {
    let script = format!("printf 'yes\\n' | '{server}' recover");
    let output = session.run("sh", &["-c".into(), script.into()])?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    if !stdout.contains(said) {
        return Err(Failure::new(format!(
            "M4 crash: recover didn't say {said:?}: {stdout}"
        )));
    }
    stop::marker_gone(session)?;
    structured(&b.call(session, "acquire_desktop", json!({}))?)?;
    structured(&b.call(session, "release_desktop", json!({"restore_focus": false}))?).map(drop)
}

fn mid_typing(session: &mut Session<'_>, wev: &Wev<'_>, server: &str) -> Result<()> {
    let mut a = server_a(session, server, "harness-m4-a")?;
    let text = TEXT.repeat(10);
    let offset = wev.offset()?;
    a.start_call(
        "type_text",
        json!({"text": text, "expect": {"app_id": "wev"}}),
    )?;
    session.wait_until("m4-crash-typing", "the first key in wev", WAIT, |_| {
        let seen = keyboard::since(wev.log, offset)?;
        Ok((!wev::keyboard::trace(&seen)?.keys.is_empty()).then_some(()))
    })?;
    super::guardian::kill(session, a)?;
    let mut b = refused(session, server, "harness-m4-b")?;
    // wtype is in a group of its own, so it outlives server A and types everything.
    let seen = keyboard::observed(session, wev.log, offset, text.chars().count(), false)?;
    keyboard::text(&wev::keyboard::trace(&seen)?, &text)?;
    recover(session, &mut b, server, "has already exited")?;
    b.stop()?;
    session.log("M4 crash mid-typing: B refused with recovery_required, wtype typed everything, recover cleared the marker, B took the lease")
}

fn mid_drag(session: &mut Session<'_>, wev: &Wev<'_>, server: &str) -> Result<()> {
    let mut a = server_a(session, server, "harness-m4-c")?;
    let shot = Shot::take(session, &mut a, 4000)?;
    let from = shot.pixel(wev.in_layout((50.0, 50.0)));
    let to = shot.pixel(wev.in_layout((350.0, 250.0)));
    let offset = wev.offset()?;
    a.start_call(
        "drag",
        json!({
            "screenshot_ref": shot.id,
            "from": {"x": from.0, "y": from.1},
            "to": {"x": to.0, "y": to.1}
        }),
    )?;
    let press = Pointer::Button {
        code: 272,
        pressed: true,
    };
    let release = Pointer::Button {
        code: 272,
        pressed: false,
    };
    session.wait_until("m4-crash-drag", "the drag's press in wev", WAIT, |_| {
        Ok(wev.since(offset)?.contains(&press).then_some(()))
    })?;
    let killed = super::guardian::kill(session, a)?;
    let mut b = refused(session, server, "harness-m4-d")?;
    // niri releases nothing when a pointer goes (C8): A's guardian sends the release.
    super::guardian::released(session, killed, "M4 drag button", || {
        Ok(wev.since(offset)?.contains(&release))
    })?;
    recover(
        session,
        &mut b,
        server,
        "Sent the release of pointer buttons [272]",
    )?;
    session.wait_until("m4-crash-release", "recover's release in wev", WAIT, |_| {
        Ok(wev.since(offset)?.contains(&release).then_some(()))
    })?;
    b.stop()?;
    session.log("M4 crash mid-drag: the guardian released the button, B refused with recovery_required until recover, which sent the release again, B took the lease")
}
