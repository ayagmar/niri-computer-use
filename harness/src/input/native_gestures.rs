//! Held modifiers across pointer gestures, including stop/cancel and kill + recover.

use serde_json::{Value, json};

use super::{SERVER_DEADLINE, Shot, WAIT, Wev, stop};
use crate::failure::{Failure, Result};
use crate::keyboard;
use crate::mcp::{Client, field, structured};
use crate::session::Session;
use crate::wev::{
    self, Pointer,
    keyboard::{Modifiers, trace},
};

fn arguments(
    session: &mut Session<'_>,
    client: &mut Client,
    wev: &Wev<'_>,
    tool: &str,
) -> Result<Value> {
    let shot = Shot::take(session, client, 4000)?;
    let from = shot.pixel(wev.in_layout((50.0, 50.0)));
    let to = shot.pixel(wev.in_layout((350.0, 250.0)));
    Ok(match tool {
        "drag" => {
            json!({"screenshot_ref": shot.id, "keys": ["ctrl", "shift"], "from": {"x": from.0, "y": from.1}, "to": {"x": to.0, "y": to.1}})
        }
        "scroll" => {
            json!({"screenshot_ref": shot.id, "keys": ["ctrl", "shift"], "x": from.0, "y": from.1, "notches_y": 2})
        }
        _ => {
            json!({"screenshot_ref": shot.id, "keys": ["ctrl", "shift"], "x": from.0, "y": from.1})
        }
    })
}

pub(super) fn normal(session: &mut Session<'_>, client: &mut Client, wev: &Wev<'_>) -> Result<()> {
    for tool in ["click", "drag", "scroll"] {
        let args = arguments(session, client, wev, tool)?;
        let offset = wev.offset()?;
        structured(&client.call(session, tool, args)?)?;
        let seen = keyboard::since(wev.log, offset)?;
        held_at_pointer(
            &seen,
            if tool == "scroll" {
                "axis_source:"
            } else {
                "button:"
            },
        )?;
        released(&seen)?;
        stop::marker_gone(session)?;
        session.log(&format!(
            "M7 held ctrl+shift {tool}: modifiers 5 at pointer event, zero at end, marker gone"
        ))?;
    }
    Ok(())
}

fn held_at_pointer(log: &str, token: &str) -> Result<()> {
    let at = log
        .find(token)
        .ok_or_else(|| Failure::new("M7 held gesture has no pointer event"))?;
    let prefix = log.get(..at).unwrap_or_default();
    if trace(prefix)?
        .modifiers
        .is_none_or(|mods| mods.depressed != 5)
    {
        return Err(Failure::new("M7 pointer event did not have Control+Shift"));
    }
    Ok(())
}

fn released(log: &str) -> Result<()> {
    if trace(log)?.modifiers != Some(Modifiers::default()) {
        return Err(Failure::new("M7 modifiers remain held"));
    }
    let mut held = std::collections::BTreeSet::new();
    for event in wev::pointer_trace(log)? {
        if let Pointer::Button { code, pressed } = event {
            if pressed {
                held.insert(code);
            } else {
                held.remove(&code);
            }
        }
    }
    if !held.is_empty() {
        return Err(Failure::new("M7 pointer button remains held"));
    }
    let mut keys = std::collections::BTreeSet::new();
    for key in trace(log)?.keys {
        if key.pressed {
            keys.insert(key.code);
        } else {
            keys.remove(&key.code);
        }
    }
    if !keys.is_empty() {
        return Err(Failure::new("M7 keyboard key remains held"));
    }
    Ok(())
}

pub(super) fn interrupt(
    session: &mut Session<'_>,
    client: &mut Client,
    wev: &Wev<'_>,
    server: &str,
    cancel: bool,
) -> Result<()> {
    let args = arguments(session, client, wev, "drag")?;
    let offset = wev.offset()?;
    let id = client.start_call("drag", args)?;
    pressed(session, wev, offset)?;
    if cancel {
        client.cancel(id)?;
    } else {
        session.run(server, &["stop".into()])?;
        let result = client.result(session, id)?;
        if field(&result, "/structuredContent/error") != "stopped" {
            return Err(Failure::new(format!("M7 stop missed held drag: {result}")));
        }
    }
    stop::marker_gone(session)?;
    let seen = keyboard::since(wev.log, offset)?;
    held_at_pointer(&seen, "button:")?;
    released(&seen)?;
    if !cancel {
        session.run(server, &["resume".into()])?;
        session.wait_until("m7-held-resume", "lease reacquired", WAIT, |session| {
            Ok(
                structured(&client.call(session, "acquire_desktop", json!({}))?)
                    .ok()
                    .map(drop),
            )
        })?;
    }
    session.log(&format!(
        "M7 held drag {}: button and modifiers released, marker gone",
        if cancel { "cancel" } else { "stop" }
    ))
}

fn pressed(session: &mut Session<'_>, wev: &Wev<'_>, offset: usize) -> Result<()> {
    session.wait_until("m7-held-press", "the held drag's press", WAIT, |_| {
        Ok(wev
            .since(offset)?
            .contains(&Pointer::Button {
                code: 272,
                pressed: true,
            })
            .then_some(()))
    })
}

pub(super) fn crash(
    session: &mut Session<'_>,
    mut client: Client,
    wev: &Wev<'_>,
    server: &str,
) -> Result<()> {
    let args = arguments(session, &mut client, wev, "drag")?;
    let offset = wev.offset()?;
    client.start_call("drag", args)?;
    pressed(session, wev, offset)?;
    client.stop()?;
    let mut next = Client::start(session, server, "harness-m7-recover", SERVER_DEADLINE)?;
    let refused = next.call(session, "acquire_desktop", json!({}))?;
    if field(&refused, "/structuredContent/error") != "recovery_required" {
        return Err(Failure::new(format!(
            "M7 kill didn't leave recovery gate: {refused}"
        )));
    }
    let output = session.run(
        "sh",
        &[
            "-c".into(),
            format!("printf 'yes\\n' | '{server}' recover").into(),
        ],
    )?;
    if !String::from_utf8_lossy(&output.stdout).contains("Sent native key releases") {
        return Err(Failure::new("M7 recover did not release native modifiers"));
    }
    stop::marker_gone(session)?;
    session.wait_until(
        "m7-held-recovered",
        "button and modifiers released",
        WAIT,
        |_| Ok(released(&keyboard::since(wev.log, offset)?).ok().map(drop)),
    )?;
    held_at_pointer(&keyboard::since(wev.log, offset)?, "button:")?;
    structured(&next.call(session, "acquire_desktop", json!({}))?)?;
    structured(&next.call(session, "release_desktop", json!({"restore_focus": false}))?)?;
    next.stop()?;
    session.log("M7 SIGKILL during held drag: marker blocked B; recover released button and modifiers; B acquired. No automatic crash release claim.")
}
