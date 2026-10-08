//! Step 6 only. Physical events and the human's confirmation are separate requirements.

use std::fs;
use std::io::ErrorKind;
use std::os::unix::process::ExitStatusExt as _;
use std::path::Path;
use std::time::{Duration, Instant};

use niri_ipc::{Action, LogicalOutput, Request, Response, Window};

use crate::failure::{Context as _, Failure, Result};
use crate::keyboard::{self, WTYPE_DEADLINE};
use crate::pointer::Probe;
use crate::runner::Process;
use crate::session::Session;
use crate::wev::{self, keyboard as records};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    Full,
    FromC8,
}

impl Mode {
    pub(crate) const fn flag(self) -> &'static str {
        match self {
            Self::Full => "--sitting",
            Self::FromC8 => "--sitting-from-c8",
        }
    }
}

pub(crate) const RUN_DEADLINE: Duration = Duration::from_mins(30);
const WEV_DEADLINE: Duration = Duration::from_mins(29);
const HUMAN_WAIT: Duration = Duration::from_secs(120);
const WAIT: Duration = Duration::from_secs(5);
const OBSERVE: Duration = Duration::from_millis(500);
const LEFT: u32 = 272;

pub(crate) fn run(
    session: &mut Session<'_>,
    output: &LogicalOutput,
    path: &str,
    mode: Mode,
) -> Result<()> {
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
    if mode == Mode::Full {
        c6(session, &focused, &observer)?;
    } else {
        session.log(
            "C6 and C7: skipped, explicitly resuming from C8; no result claimed in this run",
        )?;
    }
    second.stop()?;
    if mode == Mode::Full {
        c7(session, &focused)?;
    }
    let probe = Probe { path, output };
    crate::pointer::enter_window(session, &probe, &focused, &window.layout)?;
    c8(session, &focused, &probe)?;
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

fn pointer_held(log: &str) -> Result<bool> {
    Ok(wev::button_trace(log)?
        .iter()
        .rev()
        .find(|button| button.code == LEFT)
        .is_some_and(|button| button.pressed))
}

fn physical_click(buttons: &[wev::Button]) -> bool {
    buttons.iter().enumerate().any(|(index, press)| {
        press.code == LEFT
            && !matches!(press.time, 16000..=16003)
            && press.pressed
            && buttons.iter().skip(index + 1).any(|release| {
                release.code == LEFT && !matches!(release.time, 16000..=16003) && !release.pressed
            })
    })
}

fn pointer_hold(
    session: &mut Session<'_>,
    log: &Path,
    probe: &Probe<'_>,
    time: u32,
) -> Result<usize> {
    let start = keyboard::offset(log)?;
    let args = probe.args(time, &["hold".to_owned(), LEFT.to_string()])?;
    session.log(&format!("COMMAND C8: {} {args:?}", probe.path))?;
    let mut process = session.start(
        probe.path,
        &args,
        session.artifact(&format!("vpointer-{time}.log")),
        WTYPE_DEADLINE,
    )?;
    session.wait_until(
        "c8-press",
        "the probe's left-button press in wev",
        Duration::from_secs(1),
        |_| {
            let log = keyboard::since(log, start)?;
            Ok(wev::button_trace(&log)?
                .iter()
                .any(|button| button.time == time && button.code == LEFT && button.pressed)
                .then_some(()))
        },
    )?;
    process.ensure_running()?;
    killed(session, process, "C8")?;
    observe(session)?;
    session.log(&format!(
        "C8 time {time}: stuck after SIGKILL = {}",
        pointer_held(&keyboard::since(log, start)?)?
    ))?;
    Ok(start)
}

fn c8(session: &mut Session<'_>, log: &Path, probe: &Probe<'_>) -> Result<()> {
    let start = pointer_hold(session, log, probe, 16001)?;
    probe.send(session, 16002, &["release".to_owned(), LEFT.to_string()])?;
    observe(session)?;
    session.log(&format!(
        "C8: cleared by fresh pointer release = {}",
        !pointer_held(&keyboard::since(log, start)?)?
    ))?;
    let physical_start = pointer_hold(session, log, probe, 16003)?;
    let before = keyboard::offset(log)?;
    session.log("INSTRUCTION 5: Physically left-click once inside the nested checkerboard and release the button. Return here and confirm what you did and saw.")?;
    session.wait_until(
        "c8-physical-click",
        "a physical left-button press and release",
        HUMAN_WAIT,
        |_| {
            let log = keyboard::since(log, before)?;
            let seen = wev::button_trace(&log)?;
            Ok(physical_click(&seen).then_some(()))
        },
    )?;
    session.log(&format!(
        "C8: cleared by physical click = {}",
        !pointer_held(&keyboard::since(log, physical_start)?)?
    ))?;
    confirm(
        session,
        5,
        "Confirm that you physically left-clicked and released inside the nested checkerboard and describe what you saw.",
    )?;
    session.screenshot("success-c8.png")?;
    session.log("C8: fresh-device and physical recovery recorded independently; human confirmed")
}

fn c9(session: &mut Session<'_>, log: &Path) -> Result<()> {
    let marker = session.bind_marker();
    if marker.try_exists().context("check bind-fired")? {
        return Err(Failure::new("C9: bind-fired already exists"));
    }
    session.log("INSTRUCTION 6: Click the nested checkerboard. Hold Ctrl and Shift, press and release F12 once, then release Shift and Ctrl. Return here and confirm what you did and saw.")?;
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
        6,
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
    fn physical_click_requires_an_ordered_non_probe_pair() {
        let event = |time, pressed| wev::Button {
            time,
            code: LEFT,
            pressed,
        };
        assert!(physical_click(&[event(123, true), event(124, false)]));
        assert!(!physical_click(&[event(124, false), event(123, true)]));
        assert!(!physical_click(&[event(16003, true), event(124, false)]));
        assert!(!physical_click(&[event(123, true)]));
        let signed_pair = "[ 1: wl_pointer] button: time: -2147483648; button: 272, state: 1 (pressed)\n[ 1: wl_pointer] button: time: -1; button: 272, state: 0 (released)\n";
        assert!(physical_click(&wev::button_trace(signed_pair).unwrap()));
    }

    #[test]
    fn pointer_release_clears_the_held_button() {
        let press =
            "[ 1: wl_pointer] button: time: 16001; button: 272 (left), state: 1 (pressed)\n";
        let release = press.replace("state: 1 (pressed)", "state: 0 (released)");
        assert!(pointer_held(press).unwrap());
        assert!(!pointer_held(&format!("{press}{release}")).unwrap());
        assert!(pointer_held(&format!("{press}{}", release.replace("272", "273"))).unwrap());
    }
}
