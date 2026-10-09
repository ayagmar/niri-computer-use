//! Step 6 only. Physical events and the human's confirmation are separate requirements.

use std::fs;
use std::io::ErrorKind;
use std::os::unix::process::ExitStatusExt as _;
use std::path::Path;
use std::time::{Duration, Instant};

use niri_ipc::{Action, Request, Response, Window};

use crate::failure::{Context as _, Failure, Result};
use crate::keyboard::{self, WTYPE_DEADLINE};
use crate::runner::Process;
use crate::session::Session;
use crate::wev::keyboard as records;

pub(crate) const RUN_DEADLINE: Duration = Duration::from_mins(30);
const WEV_DEADLINE: Duration = Duration::from_mins(29);
const HUMAN_WAIT: Duration = Duration::from_secs(120);
const WAIT: Duration = Duration::from_secs(5);
const OBSERVE: Duration = Duration::from_millis(500);

pub(crate) fn run(session: &mut Session<'_>) -> Result<()> {
    let focused = session.artifact("wev.log");
    let observer = session.artifact("wev-unfocused.log");
    let args = keyboard::args(&["-oL", "wev"]);
    let first = session.start("stdbuf", &args, focused.clone(), WEV_DEADLINE)?;
    let window = windows(session, 1)?.remove(0);
    let second = session.start("stdbuf", &args, observer.clone(), WEV_DEADLINE)?;
    windows(session, 2)?;
    session.request(&Request::Action(Action::FocusWindow { id: window.id }))?;
    ready(session, &focused, &observer)?;
    session.screenshot("sitting-ready.png")?;
    confirm(
        session,
        1,
        "Click the checkerboard inside the nested niri window at the bottom-right. Do not press a key yet. Return here and confirm you clicked it and saw the checkerboard.",
    )?;
    ready(session, &focused, &observer)?;
    c6(session, &focused, &observer)?;
    second.stop()?;
    c7(session, &focused)?;
    c9(session, &focused)?;
    first.stop()?;
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

fn ready(session: &mut Session<'_>, focused: &Path, observer: &Path) -> Result<()> {
    session.wait_until(
        "sitting-focus",
        "focused wev and unfocused observer with initial keymaps",
        WAIT,
        |_| {
            let a = keyboard::read(focused)?;
            let b = keyboard::read(observer)?;
            let (a, b) = (records::trace(&a)?, records::trace(&b)?);
            Ok(
                (a.focused && !b.focused && !a.keymaps.is_empty() && !b.keymaps.is_empty())
                    .then_some(()),
            )
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

fn physical_key(log: &str, symbols: &[&str]) -> Result<bool> {
    let seen = records::trace(log)?;
    Ok(seen.keys.iter().enumerate().any(|(index, press)| {
        press.time != 0
            && press.pressed
            && symbols.contains(&press.symbol)
            && seen
                .keys
                .iter()
                .skip(index + 1)
                .any(|release| !release.pressed && release.time != 0 && release.code == press.code)
    }))
}

fn c6(session: &mut Session<'_>, focused: &Path, observer: &Path) -> Result<()> {
    let initial = records::trace(&keyboard::read(observer)?)?
        .keymaps
        .first()
        .copied()
        .ok_or_else(|| Failure::new("C6: observer has no initial keymap"))?;
    let (a, b) = (keyboard::offset(focused)?, keyboard::offset(observer)?);
    let corpus = "a".repeat(100) + &"b".repeat(50);
    session.log("COMMAND C6: wtype -; stdin 100 a + 50 b, one call, two wtype batches")?;
    keyboard::send(session, &["-"], Some(&corpus))?;
    let typed = keyboard::observed(session, focused, a, 150, false)?;
    keyboard::text(&records::trace(&typed)?, &corpus)?;
    let broadcast =
        session.wait_until("c6-broadcast", "a broadcast to unfocused wev", WAIT, |_| {
            let log = keyboard::since(observer, b)?;
            let seen = records::trace(&log)?;
            if seen.entered || !seen.keys.is_empty() {
                return Err(Failure::new("C6: observer received focus or keys"));
            }
            Ok((!seen.keymaps.is_empty()).then_some(seen.keymaps))
        })?;
    session.log(&format!(
        "C6: initial {initial:?}; broadcast {broadcast:?}; exactly 150 decoded pairs"
    ))?;
    let (physical_start, restore_start) = (keyboard::offset(focused)?, keyboard::offset(observer)?);
    session.log("INSTRUCTION 2: Click the nested checkerboard, press and release physical x once, then return here and confirm what you did and saw. The checkerboard does not display typed text.")?;
    session.wait_until("c6-physical-x", "a physical x pair", HUMAN_WAIT, |_| {
        Ok(physical_key(&keyboard::since(focused, physical_start)?, &["x"])?.then_some(()))
    })?;
    session.wait_until(
        "c6-restored",
        "the observer's original keymap after physical x (otherwise reject wtype)",
        WAIT,
        |_| {
            let log = keyboard::since(observer, restore_start)?;
            let seen = records::trace(&log)?;
            Ok(
                (!seen.entered && seen.keys.is_empty() && seen.keymaps.last() == Some(&initial))
                    .then_some(()),
            )
        },
    )?;
    confirm(
        session,
        2,
        "Confirm that you physically pressed and released x in the nested checkerboard and describe what you saw.",
    )?;
    session.log("C6: original keymap format and size restored in unfocused wev; physical x observed and human confirmed: pass")
}

#[derive(Debug, Clone, Copy)]
enum Held {
    Key(u32),
    Shift,
}

impl Held {
    fn in_log(self, log: &str) -> Result<bool> {
        let seen = records::trace(log)?;
        Ok(match self {
            Self::Key(code) => seen
                .keys
                .iter()
                .rev()
                .find(|key| key.code == code)
                .is_some_and(|key| key.pressed),
            Self::Shift => seen.modifiers.is_some_and(|mods| mods.depressed & 1 != 0),
        })
    }
}

fn killed(session: &mut Session<'_>, process: Process, label: &str) -> Result<()> {
    let output = process.stop()?;
    if output.status.signal() != Some(9) {
        return Err(Failure::new(format!(
            "{label}: expected SIGKILL, got {}",
            output.status
        )));
    }
    session.log(&format!(
        "{label}: observed press, then SIGKILL; {}",
        output.status
    ))
}

fn observe(session: &mut Session<'_>) -> Result<()> {
    let start = Instant::now();
    session.wait_until(
        "sitting-observe",
        "500 ms observation interval",
        WAIT,
        |_| Ok((start.elapsed() >= OBSERVE).then_some(())),
    )
}

fn keyboard_hold(session: &mut Session<'_>, log: &Path, shift: bool) -> Result<(usize, Held)> {
    let start = keyboard::offset(log)?;
    let args = if shift {
        &["-M", "shift", "-s", "2000", "-m", "shift"][..]
    } else {
        &["-P", "a", "-s", "2000", "-p", "a"][..]
    };
    session.log(&format!("COMMAND C7: wtype {}", args.join(" ")))?;
    let mut process = session.start_with_stdin("wtype", &keyboard::args(args), WTYPE_DEADLINE)?;
    process.feed(Vec::new())?;
    let held = session.wait_until(
        "c7-press",
        "the virtual key press or Shift modifier",
        Duration::from_secs(1),
        |_| {
            let log = keyboard::since(log, start)?;
            let seen = records::trace(&log)?;
            Ok(if shift {
                seen.modifiers
                    .filter(|mods| mods.depressed & 1 != 0)
                    .map(|_| Held::Shift)
            } else {
                seen.keys
                    .iter()
                    .find(|key| key.pressed && key.symbol == "a" && key.time == 0)
                    .map(|key| Held::Key(key.code))
            })
        },
    )?;
    process.ensure_running()?;
    killed(session, process, "C7")?;
    observe(session)?;
    session.log(&format!(
        "C7 {held:?}: stuck after SIGKILL = {}",
        held.in_log(&keyboard::since(log, start)?)?
    ))?;
    Ok((start, held))
}

fn c7(session: &mut Session<'_>, log: &Path) -> Result<()> {
    for (shift, number, symbols) in [
        (false, 3, &["a"][..]),
        (true, 4, &["Shift_L", "Shift_R"][..]),
    ] {
        let release = if shift {
            &["-m", "shift"][..]
        } else {
            &["-p", "a"][..]
        };
        let (start, held) = keyboard_hold(session, log, shift)?;
        session.log(&format!(
            "COMMAND C7 fresh recovery: wtype {}",
            release.join(" ")
        ))?;
        keyboard::send(session, release, None)?;
        observe(session)?;
        session.log(&format!(
            "C7 {held:?}: cleared by fresh wtype = {}",
            !held.in_log(&keyboard::since(log, start)?)?
        ))?;
        // A new observed press is required: fresh recovery must not clear the physical trial.
        let (physical_start, physical_held) = keyboard_hold(session, log, shift)?;
        let before = keyboard::offset(log)?;
        let key = if shift { "Shift" } else { "a" };
        session.log(&format!("INSTRUCTION {number}: Click the nested checkerboard. Press and release physical {key} once. Return here and confirm what you did and saw."))?;
        session.wait_until("c7-physical", "the physical key pair", HUMAN_WAIT, |_| {
            Ok(physical_key(&keyboard::since(log, before)?, symbols)?.then_some(()))
        })?;
        let after = keyboard::since(log, physical_start)?;
        session.log(&format!(
            "C7 {physical_held:?}: cleared by physical key = {}; trace {:?}",
            !physical_held.in_log(&after)?,
            records::trace(&after)?
        ))?;
        confirm(
            session,
            number,
            &format!(
                "Confirm that you physically pressed and released {key} in the nested checkerboard and describe what you saw."
            ),
        )?;
        // Leave a released virtual key/modifier before the next experiment.
        keyboard::send(session, release, None)?;
    }
    session.log("C7: both recovery methods recorded for independently interrupted a and Shift; human confirmed")
}

fn c9(session: &mut Session<'_>, log: &Path) -> Result<()> {
    let marker = session.bind_marker();
    if marker.try_exists().context("check bind-fired")? {
        return Err(Failure::new("C9: bind-fired already exists"));
    }
    session.log("INSTRUCTION 5: Click the nested checkerboard. Hold Ctrl and Shift, press and release F12 once, then release Shift and Ctrl. Return here and confirm what you did and saw.")?;
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
        5,
        "Confirm that you physically pressed Ctrl+Shift+F12 inside the nested checkerboard, released all keys, and describe what you saw.",
    )?;
    fs::remove_file(marker).context("remove nested bind-fired")?;
    keyboard::c9(session, log)?;
    session.screenshot("success-c9.png")?;
    session.log(
        "C9: physical positive control confirmed; virtual half did not fire bind for 1 s: pass",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const PRESS: &str =
        "[ 1: wl_keyboard] key: time: 0; key: 9; state: 1 (pressed)\n sym: a (97), utf8: 'a'\n";
    const RELEASE: &str =
        "[ 1: wl_keyboard] key: time: 0; key: 9; state: 0 (released)\n sym: a (97), utf8: ''\n";

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

    #[test]
    fn physical_pairs_cannot_be_virtual_or_another_key() {
        let virtual_pair = format!("{PRESS}{RELEASE}");
        assert!(!physical_key(&virtual_pair, &["a"]).unwrap());
        let physical = virtual_pair.replace("time: 0", "time: 123");
        assert!(physical_key(&physical, &["a"]).unwrap());
        let upper_half = virtual_pair.replace("time: 0", "time: -2147483648");
        assert!(physical_key(&upper_half, &["a"]).unwrap());
        let reversed = format!("{RELEASE}{PRESS}").replace("time: 0", "time: 123");
        assert!(!physical_key(&reversed, &["a"]).unwrap());
        assert!(!physical_key(&physical, &["x"]).unwrap());
        assert!(!physical_key(&PRESS.replace("time: 0", "time: 123"), &["a"]).unwrap());
    }

    #[test]
    fn a_held_key_requires_a_release_of_the_same_code() {
        assert!(Held::Key(9).in_log(PRESS).unwrap());
        assert!(!Held::Key(9).in_log(&format!("{PRESS}{RELEASE}")).unwrap());
        assert!(
            Held::Key(9)
                .in_log(&format!("{PRESS}{}", RELEASE.replace("key: 9", "key: 38")))
                .unwrap()
        );
    }

    #[test]
    fn shift_is_held_until_the_last_modifiers_record_clears_it() {
        let mods = |mask: &str| {
            format!(
                "[ 1: wl_keyboard] modifiers: serial: 1; group: 0\n                      depressed: {mask}\n                      latched: 00000000\n                      locked: 00000000\n"
            )
        };
        let shift = mods("00000001: Shift ");
        assert!(Held::Shift.in_log(&shift).unwrap());
        assert!(
            !Held::Shift
                .in_log(&format!("{shift}{}", mods("00000000")))
                .unwrap()
        );
        assert!(!Held::Shift.in_log(&mods("00000004: Control ")).unwrap());
        assert!(!Held::Shift.in_log("").unwrap());
        // wev starts a fresh xkb state for each keymap (`wev.c:335–339`).
        let keymap = "[ 1: wl_keyboard] keymap: format: 1 (xkb v1), size: 35572\n";
        assert!(!Held::Shift.in_log(&format!("{shift}{keymap}")).unwrap());
    }
}
