//! Held modifiers across pointer gestures, including stop/cancel and kill + recover.

use rustix::process::Signal;
use serde_json::{Value, json};

use super::{SERVER_DEADLINE, Shot, WAIT, Wev, stop};
use crate::failure::{Context as _, Failure, Result};
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
    let killed = super::guardian::kill(session, client)?;
    super::guardian::released(session, killed, "held drag button and ctrl+shift", || {
        Ok(released(&keyboard::since(wev.log, offset)?).is_ok())
    })?;
    recover(session, wev, server, offset, "harness-m7-recover")?;
    held_at_pointer(&keyboard::since(wev.log, offset)?, "button:")?;
    session.log("M7 SIGKILL during held drag: the guardian released button and modifiers; the marker blocked B until recover; B acquired")
}

fn recover(
    session: &mut Session<'_>,
    wev: &Wev<'_>,
    server: &str,
    offset: usize,
    name: &str,
) -> Result<()> {
    let mut next = Client::start(session, server, name, SERVER_DEADLINE)?;
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
    structured(&next.call(session, "acquire_desktop", json!({}))?)?;
    structured(&next.call(session, "release_desktop", json!({"restore_focus": false}))?)?;
    next.stop()
}

/// A native typing call to kill: `text` repeated, `name` to tell its server's logs apart,
/// and the layout niri switches to, if any, while the serving process is stopped after the
/// first key, so the call never sees the switch.
#[derive(Debug, Clone, Copy)]
pub(super) struct Crash<'a> {
    pub(super) text: &'a str,
    pub(super) name: &'a str,
    pub(super) switch_to: Option<u8>,
}

/// Kills a server typing `crash.text` repeated, after its first key, and requires its
/// guardian, before `recover`, to release that key's original code with zero modifiers in
/// layout 0, then runs `before_recover` and `recover`, which must do the same.
pub(super) fn typing_crash(
    session: &mut Session<'_>,
    wev: &Wev<'_>,
    server: &str,
    crash: Crash<'_>,
    before_recover: impl FnOnce(&mut Session<'_>) -> Result<()>,
) -> Result<()> {
    let Crash {
        text,
        name,
        switch_to,
    } = crash;
    let mut client = Client::start_command(
        session,
        "env",
        &[
            "NIRI_COMPUTER_USE_KEYBOARD=native".into(),
            server.into(),
            "serve".into(),
        ],
        &format!("harness-m7-killed-{name}"),
        SERVER_DEADLINE,
    )?;
    structured(&client.call(session, "acquire_desktop", json!({}))?)?;
    let engine = super::guardian::engine(session, &mut client)?;
    let offset = wev.offset()?;
    client.start_call(
        "type_text",
        json!({"text": text.repeat(1000), "expect": {"app_id": "wev"}}),
    )?;
    session.wait_until("m7-kill-key", "the first native key press", WAIT, |_| {
        Ok(trace(&keyboard::since(wev.log, offset)?)?
            .keys
            .iter()
            .any(|key| key.pressed)
            .then_some(()))
    })?;
    if let Some(index) = switch_to {
        let serving = match engine {
            Some(pid) => i32::try_from(pid).context("the engine's PID")?,
            None => client.pid(),
        };
        super::native_unicode::signal(serving, Signal::STOP)?;
        super::native_unicode::switch_layout(session, index)?;
    }
    let killed = super::guardian::kill_known(client, engine)?;
    let at_kill = keyboard::since(wev.log, offset)?;
    let first = trace(&at_kill)?
        .keys
        .into_iter()
        .find(|key| key.pressed)
        .ok_or_else(|| Failure::new("M7 kill lacked an observed press"))?;
    super::guardian::released(session, killed, &format!("typing {text:?}"), || {
        Ok(released(&keyboard::since(wev.log, offset)?).is_ok())
    })?;
    before_recover(session)?;
    let recovery_offset = wev.offset()?;
    recover(
        session,
        wev,
        server,
        offset,
        &format!("harness-m7-recover-{name}"),
    )?;
    let after = keyboard::since(wev.log, recovery_offset)?;
    let observed = trace(&after)?;
    if !observed
        .keys
        .iter()
        .any(|key| key.code == first.code && !key.pressed)
    {
        return Err(Failure::new(
            "M7 recover didn't release the original native code",
        ));
    }
    session.log(&format!("M7 SIGKILL during native typing of {text:?}: the guardian released original wev code {}, zero modifiers, recovery gate retained until recover, which released it again", first.code))
}
