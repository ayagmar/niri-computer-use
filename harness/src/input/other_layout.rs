//! A native `key` whose keysym only another layout has: with niri's config gaining a
//! Russian layout and niri switched to it, `ctrl+a` must go out in the first layout, with
//! niri's active layout left in place, in `wev` and in the GTK entry.

use std::fs;

use serde_json::json;

use super::native_unicode::{current_layout, layout_count, left_in, switch_layout, write_config};
use super::{WAIT, Wev};
use crate::failure::{Context as _, Failure, Result};
use crate::keyboard;
use crate::mcp::{Client, field, structured};
use crate::session::Session;
use crate::wev::keyboard::trace;

const US_RU: &str =
    "\ninput {\n    keyboard {\n        xkb {\n            layout \"us,ru\"\n        }\n    }\n}\n";
/// Control's bit in the evdev keymaps niri compiles, as `wev` prints it.
const CTRL: u32 = 4;

/// `ctrl+a` into `wev` with the Russian layout active must end `sent`, decode as `a`
/// with Control, leave `wev`'s last modifiers in the Russian layout, and leave niri's
/// active layout there: a niri whose active layout followed the virtual keyboard's would
/// end the call `interrupted`.
pub(super) fn in_wev(session: &mut Session<'_>, client: &mut Client, wev: &Wev<'_>) -> Result<()> {
    in_russian(session, |session| {
        let offset = wev.offset()?;
        let result = structured(&client.call(
            session,
            "key",
            json!({"keys": ["ctrl+a"], "expect": {"app_id": "wev"}}),
        )?)?;
        if field(&result, "/observed") != "sent" {
            return Err(Failure::new(format!(
                "M7 other layout: ctrl+a in wev answered {result}"
            )));
        }
        let seen = keyboard::observed(session, wev.log, offset, 1, false)?;
        let seen = trace(&seen)?;
        let decoded = seen.keys.first().is_some_and(|key| {
            key.symbol == "a"
                && key
                    .modifiers
                    .is_some_and(|mods| mods.depressed == CTRL && mods.group == 0)
        });
        if seen.keys.len() != 2 || !decoded {
            return Err(Failure::new(format!(
                "M7 other layout: expected one a pair with Control in the first layout in wev; got {seen:?}"
            )));
        }
        left_in(wev, offset, 1, "after a key from the first layout")?;
        still_russian(session)
    })?;
    session.log("M7 native key from another layout: ctrl+a with the Russian layout active went out as a with Control in wev, which was left in the Russian layout, and niri's active layout stayed Russian")
}

/// `ctrl+a`, then `BackSpace`, with the Russian layout active must empty the GTK entry.
pub(super) fn in_entry(session: &mut Session<'_>, client: &mut Client) -> Result<()> {
    in_russian(session, |session| {
        super::text_entry::typed(session, client, "abc")?;
        super::text_entry::clear(session, client)?;
        still_russian(session)
    })?;
    session.log("M7 native key from another layout: ctrl+a then BackSpace with the Russian layout active emptied the GTK entry, and niri's active layout stayed Russian")
}

/// Runs `check` with niri's config holding `us,ru` and niri switched to Russian, then
/// puts the config back, also when the switch or `check` failed, so later steps keep
/// one layout.
fn in_russian(
    session: &mut Session<'_>,
    check: impl FnOnce(&mut Session<'_>) -> Result<()>,
) -> Result<()> {
    let config = session.test_dir().niri_config();
    let original = fs::read_to_string(&config).context(format!("read {}", config.display()))?;
    write_config(&config, &format!("{original}{US_RU}"))?;
    let checked = session
        .wait_until("m7-us-ru", "niri's two layouts", WAIT, |session| {
            Ok((layout_count(session)? == 2).then_some(()))
        })
        .and_then(|()| switch_layout(session, 1))
        .and_then(|()| check(session));
    write_config(&config, &original)?;
    session.wait_until("m7-us", "niri's one layout", WAIT, |session| {
        Ok((layout_count(session)? == 1).then_some(()))
    })?;
    checked
}

fn still_russian(session: &mut Session<'_>) -> Result<()> {
    match current_layout(session)? {
        1 => Ok(()),
        other => Err(Failure::new(format!(
            "M7 other layout: niri's active layout became {other} after the call"
        ))),
    }
}
