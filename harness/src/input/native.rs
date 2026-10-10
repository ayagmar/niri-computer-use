//! Experimental native acceptance, separate from unchanged wtype M4 criteria.

use std::time::{Duration, Instant};

use serde_json::json;

use super::{SERVER_DEADLINE, WAIT, Wev, stop};
use crate::failure::{Failure, Result};
use crate::keyboard;
use crate::mcp::{Client, field, structured};
use crate::session::Session;
use crate::wev::keyboard::{Modifiers, trace};

pub(super) fn run(
    session: &mut Session<'_>,
    owner: &mut Client,
    wev: &Wev<'_>,
    server: &str,
) -> Result<()> {
    structured(&owner.call(session, "release_desktop", json!({"restore_focus": false}))?)?;
    let mut native = Client::start_command(
        session,
        "env",
        &[
            "NIRI_COMPUTER_USE_KEYBOARD=native".into(),
            server.into(),
            "serve".into(),
        ],
        "harness-m7",
        SERVER_DEADLINE,
    )?;
    structured(&native.call(session, "acquire_desktop", json!({}))?)?;
    normal(session, &mut native, wev)?;
    let window = super::wev_window(session, &mut native)?;
    super::mismatch(session, &mut native, wev, window)?;
    super::routing(session, &mut native, wev, window)?;
    super::native_unicode::run(session, &mut native, wev)?;
    measure(session, &mut native, wev, "native")?;
    let pacer = super::pacer::start(session)?;
    super::exposure::run(session, &mut native, wev, "native")?;
    interruption(session, &mut native, wev, server, false)?;
    interruption(session, &mut native, wev, server, true)?;
    super::native_gestures::normal(session, &mut native, wev)?;
    super::native_gestures::interrupt(session, &mut native, wev, server, false)?;
    super::native_gestures::interrupt(session, &mut native, wev, server, true)?;
    super::native_gestures::crash(session, native, wev, server)?;
    super::native_gestures::typing_crash(session, wev, server, ("A", "typing"), |_| Ok(()))?;
    super::native_unicode::crash(session, wev, server)?;
    pacer.stop()?;
    super::guardian::reconnect(session, owner)?;
    structured(&owner.call(session, "acquire_desktop", json!({}))?)?;
    super::native_unicode::wtype_comparison(session, owner, wev)?;
    measure(session, owner, wev, "wtype")?;
    super::exposure::run(session, owner, wev, "wtype")
}

fn normal(session: &mut Session<'_>, client: &mut Client, wev: &Wev<'_>) -> Result<()> {
    for _ in 0..20 {
        let start = wev.offset()?;
        structured(&client.call(
            session,
            "key",
            json!({"keys": ["ctrl+a"], "expect": {"app_id": "wev"}}),
        )?)?;
        let seen = keyboard::observed(session, wev.log, start, 1, true)?;
        keyboard::chord(&trace(&seen)?, "a", 4, 1)?;
        let text_start = wev.offset()?;
        structured(&client.call(
            session,
            "type_text",
            json!({"text": "Hello", "expect": {"app_id": "wev"}}),
        )?)?;
        let text_seen = keyboard::observed(session, wev.log, text_start, 5, true)?;
        keyboard::text(&trace(&text_seen)?, "Hello")?;
    }
    stop::marker_gone(session)?;
    session.log("M7 native C5 equivalent: 20 Ctrl+a and 20 Hello calls, exact pairs and zero final modifiers")
}

fn measure(
    session: &mut Session<'_>,
    client: &mut Client,
    wev: &Wev<'_>,
    backend: &str,
) -> Result<()> {
    for length in [100, 1000] {
        let text = "Hello0123!".repeat(length / 10);
        let mut timings = Vec::new();
        for _ in 0..3 {
            let offset = wev.offset()?;
            let start = Instant::now();
            structured(&client.call(
                session,
                "type_text",
                json!({"text": text, "expect": {"app_id": "wev"}}),
            )?)?;
            let seen = keyboard::observed(session, wev.log, offset, length, backend == "native")?;
            keyboard::text(&trace(&seen)?, &text)?;
            if trace(&wev.read()?)?.modifiers != Some(Modifiers::default()) {
                return Err(Failure::new("M7 benchmark left modifiers held"));
            }
            timings.push(start.elapsed());
        }
        timings.sort_unstable();
        session.log(&format!(
            "M7 {backend} {length} characters tool-to-wev timings (min/median/max): {timings:?}"
        ))?;
    }
    Ok(())
}

fn interruption(
    session: &mut Session<'_>,
    client: &mut Client,
    wev: &Wev<'_>,
    server: &str,
    cancel: bool,
) -> Result<()> {
    let offset = wev.offset()?;
    let id = client.start_call(
        "type_text",
        json!({"text": "A".repeat(1000), "submit": true, "expect": {"app_id": "wev"}}),
    )?;
    session.wait_until("m7-first-key", "native typing started", WAIT, |_| {
        let seen = keyboard::since(wev.log, offset)?;
        Ok((!trace(&seen)?.keys.is_empty()).then_some(()))
    })?;
    let at_signal = keyboard::since(wev.log, offset)?;
    let before = trace(&at_signal)?
        .keys
        .iter()
        .filter(|key| key.pressed)
        .count();
    if cancel {
        client.cancel(id)?;
    } else {
        session.run(server, &["stop".into()])?;
    }
    let ack_log = keyboard::since(wev.log, offset)?;
    let at_ack = trace(&ack_log)?
        .keys
        .iter()
        .filter(|key| key.pressed)
        .count();
    if !cancel {
        let result = client.result(session, id)?;
        if field(&result, "/structuredContent/error") != "stopped" {
            return Err(Failure::new(format!("M7: stop missed typing: {result}")));
        }
    }
    stop::marker_gone(session)?;
    let seen = keyboard::since(wev.log, offset)?;
    let observed = trace(&seen)?;
    let count = observed.keys.iter().filter(|key| key.pressed).count();
    if count == 0 || count >= 1000 || observed.modifiers != Some(Modifiers::default()) {
        return Err(Failure::new(format!(
            "M7: interruption must end early with zero modifiers: {count}"
        )));
    }
    keyboard::text(&observed, &"A".repeat(count))?;
    if !cancel && count.saturating_sub(at_ack) > 1 {
        return Err(Failure::new(
            "M7 native sent more than one character after stop command acknowledgement",
        ));
    }
    session.log(&format!(
        "M7 native post-{}-observation characters: {}",
        if cancel {
            "cancel-notification"
        } else {
            "stop-command-ack"
        },
        count.saturating_sub(at_ack)
    ))?;
    session.still_absent("m7-interrupted", Duration::from_millis(100), || {
        Ok(keyboard::since(wev.log, offset)?.len() != seen.len())
    })?;
    session.log(&format!("M7 native {}: {count} characters, {} after pre-signal observation (50 ms polling, not stop acknowledgement), no Return, balanced pairs, zero modifiers", if cancel { "cancel" } else { "stop" }, count.saturating_sub(before)))?;
    if !cancel {
        session.run(server, &["resume".into()])?;
        session.wait_until("m7-resume", "native lease reacquired", WAIT, |session| {
            Ok(
                structured(&client.call(session, "acquire_desktop", json!({}))?)
                    .ok()
                    .map(drop),
            )
        })?;
    }
    Ok(())
}
