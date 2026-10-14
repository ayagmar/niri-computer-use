//! Step 6 only: C9's physical positive control, a human's Ctrl+Shift+F12 firing niri's
//! bind. Physical events and the human's confirmation are separate requirements. The
//! virtual half, a server's key not firing the bind, runs in `make nested-input`.

use std::fs;
use std::io::ErrorKind;
use std::path::Path;
use std::time::Duration;

use niri_ipc::{Action, Request, Response, Window};

use crate::failure::{Context as _, Failure, Result};
use crate::keyboard;
use crate::session::Session;
use crate::wev::keyboard as records;

pub(crate) const RUN_DEADLINE: Duration = Duration::from_mins(30);
const WEV_DEADLINE: Duration = Duration::from_mins(29);
const HUMAN_WAIT: Duration = Duration::from_secs(120);
const WAIT: Duration = Duration::from_secs(5);

pub(crate) fn run(session: &mut Session<'_>) -> Result<()> {
    let focused = session.artifact("wev.log");
    let args = keyboard::args(&["-oL", "wev"]);
    let wev = session.start("stdbuf", &args, focused.clone(), WEV_DEADLINE)?;
    let window = windows(session, 1)?.remove(0);
    session.request(&Request::Action(Action::FocusWindow { id: window.id }))?;
    ready(session, &focused)?;
    session.screenshot("sitting-ready.png")?;
    confirm(
        session,
        1,
        "Click the checkerboard inside the nested niri window at the bottom-right. Do not press a key yet. Return here and confirm you clicked it and saw the checkerboard.",
    )?;
    ready(session, &focused)?;
    c9(session)?;
    wev.stop()?;
    session.log("Step 6 sitting: all requested criteria recorded with human confirmations; C14 is a separate host read")
}

fn windows(session: &mut Session<'_>, count: usize) -> Result<Vec<Window>> {
    session.wait_until(
        "sitting-windows",
        "the fixed wev windows",
        WAIT,
        |session| {
            let Response::Windows(windows) = session.request(&Request::Windows)? else {
                return Err(Failure::new("niri answered Windows with another response"));
            };
            let windows: Vec<_> = windows
                .into_iter()
                .filter(|window| {
                    window.app_id.as_deref() == Some("wev")
                        && window.is_floating
                        && window.layout.window_size == (400, 300)
                })
                .collect();
            Ok((windows.len() == count).then_some(windows))
        },
    )
}

fn ready(session: &mut Session<'_>, focused: &Path) -> Result<()> {
    session.wait_until(
        "sitting-focus",
        "focused wev with its initial keymap",
        WAIT,
        |_| {
            let log = keyboard::read(focused)?;
            let seen = records::trace(&log)?;
            Ok((seen.focused && !seen.keymaps.is_empty()).then_some(()))
        },
    )
}

/// Only the supervising agent writes this file, after the human confirms in chat.
fn confirm(session: &mut Session<'_>, number: u8, instruction: &str) -> Result<()> {
    let name = format!("confirm-{number}.txt");
    let file = session.artifact(&name);
    if file.try_exists().context("check human confirmation")? {
        return Err(Failure::new(format!(
            "{name} existed before its instruction"
        )));
    }
    session.log(&format!("INSTRUCTION {number}: {instruction}"))?;
    session.log(&format!("WAITING human confirmation: {name} (120 s)"))?;
    let confirmation = session.wait_until(&name, "the human's confirmation", HUMAN_WAIT, |_| {
        match fs::read_to_string(&file) {
            Ok(text) => confirmation(&text).map(Some),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
            Err(error) => Err(Failure::new(format!("read {name}: {error}"))),
        }
    })?;
    session.log(&format!("HUMAN {number}: {confirmation}"))
}

fn confirmation(text: &str) -> Result<String> {
    let text = text.trim_end_matches('\n');
    let valid = text
        .strip_prefix("confirmed: ")
        .is_some_and(|body| !body.trim().is_empty())
        && !text.contains(['\n', '\r']);
    if valid {
        Ok(text.to_owned())
    } else {
        Err(Failure::new("human did not confirm this instruction"))
    }
}

fn c9(session: &mut Session<'_>) -> Result<()> {
    let marker = session.bind_marker();
    if marker.try_exists().context("check bind-fired")? {
        return Err(Failure::new("C9: bind-fired already exists"));
    }
    session.log("INSTRUCTION 2: Click the nested checkerboard. Hold Ctrl and Shift, press and release F12 once, then release Shift and Ctrl. Return here and confirm what you did and saw.")?;
    session.wait_until(
        "c9-physical-bind",
        "bind-fired from the physical chord",
        HUMAN_WAIT,
        |_| {
            Ok(marker
                .try_exists()
                .context("check bind-fired")?
                .then_some(()))
        },
    )?;
    confirm(
        session,
        2,
        "Confirm that you physically pressed Ctrl+Shift+F12 inside the nested checkerboard, released all keys, and describe what you saw.",
    )?;
    fs::remove_file(marker).context("remove nested bind-fired")?;
    session.screenshot("success-c9.png")?;
    session.log("C9: physical positive control confirmed: pass")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confirmations_require_an_explicit_human_statement() {
        assert_eq!(
            confirmation("confirmed: clicked checkerboard\n").unwrap(),
            "confirmed: clicked checkerboard"
        );
        for bad in [
            "",
            "yes",
            "confirmed: ",
            "abort",
            "confirmed: yes\nforged: pass",
        ] {
            assert!(confirmation(bad).is_err(), "{bad:?}");
        }
    }
}
