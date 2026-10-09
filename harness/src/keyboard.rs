//! C5, C9's virtual-input half and C10. Every wtype call goes through the nested session.

use std::ffi::OsString;
use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::failure::{Context as _, Failure, Result};
use crate::session::Session;
use crate::wev::keyboard::{self, Modifiers, Trace};

const WAIT: Duration = Duration::from_secs(5);
pub(crate) const WTYPE_DEADLINE: Duration = Duration::from_secs(3);
const CORPUS: &str = include_str!("../corpus.txt");
const CTRL_A: &[&str] = &["-M", "ctrl", "-k", "a", "-m", "ctrl"];
const ROUTING: &[&str] = &[
    "-M", "ctrl", "-M", "shift", "-k", "F12", "-m", "shift", "-m", "ctrl",
];

pub(crate) fn run(session: &mut Session<'_>, log: &Path) -> Result<()> {
    session.wait_until("keyboard-enter", "wl_keyboard.enter in wev", WAIT, |_| {
        Ok(keyboard::trace(&read(log)?)?.focused.then_some(()))
    })?;
    session.log("keyboard: wev logged wl_keyboard.enter")?;
    // A failure in normal use rejects wtype. Nothing after C5(a) runs in that case.
    c5a(session, log)?;
    c5b(session, log)?;
    c9(session, log)?;
    session.log("C9 physical positive control: unverified in automatic mode")?;
    c10(session, log)
}

pub(crate) fn read(path: &Path) -> Result<String> {
    fs::read_to_string(path).context(format!("read {}", path.display()))
}

pub(crate) fn offset(path: &Path) -> Result<usize> {
    Ok(read(path)?.len())
}

pub(crate) fn since(path: &Path, offset: usize) -> Result<String> {
    let log = read(path)?;
    log.get(offset..)
        .map(str::to_owned)
        .ok_or_else(|| Failure::new("wev log shrank or offset split UTF-8"))
}

pub(crate) fn args(args: &[&str]) -> Vec<OsString> {
    args.iter().map(OsString::from).collect()
}

pub(crate) fn send(
    session: &Session<'_>,
    arguments: &[&str],
    text: Option<&str>,
) -> Result<Duration> {
    let started = Instant::now();
    let mut process = session.start_with_stdin("wtype", &args(arguments), WTYPE_DEADLINE)?;
    let fed = process.feed(text.unwrap_or_default().as_bytes().to_vec());
    let output = process.wait()?;
    fed?;
    if !output.stderr.is_empty() {
        return Err(Failure::new(format!(
            "wtype stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(started.elapsed())
}

/// Wait for the final release and, for chords, the final all-zero modifiers record.
pub(crate) fn observed(
    session: &mut Session<'_>,
    log: &Path,
    start: usize,
    count: usize,
    chord: bool,
) -> Result<String> {
    session.wait_until(
        "keyboard-events",
        "the expected keys and final modifiers",
        WAIT,
        |_| {
            let log = since(log, start)?;
            let trace = keyboard::trace(&log)?;
            let ready = trace.keys.len() >= count * 2
                && (!chord || trace.modifiers == Some(Modifiers::default()));
            Ok(ready.then_some(log))
        },
    )
}

pub(crate) fn chord(trace: &Trace<'_>, symbol: &str, mask: u32, count: usize) -> Result<()> {
    pairs(trace, count)?;
    let expected = Modifiers {
        depressed: mask,
        ..Modifiers::default()
    };
    if trace
        .keys
        .iter()
        .any(|key| key.symbol != symbol || key.modifiers != Some(expected))
        || trace.modifiers != Some(Modifiers::default())
    {
        return Err(Failure::new(format!(
            "expected {count} {symbol} pairs with mask {mask} and final modifiers 0; got {trace:?}"
        )));
    }
    Ok(())
}

fn pairs(trace: &Trace<'_>, count: usize) -> Result<()> {
    if trace.keys.len() != count * 2
        || trace
            .keys
            .as_chunks::<2>()
            .0
            .iter()
            .any(|[press, release]| {
                !press.pressed
                    || release.pressed
                    || press.code != release.code
                    || press.symbol != release.symbol
            })
    {
        return Err(Failure::new(format!(
            "expected exactly {count} ordered press/release pairs; got {trace:?}"
        )));
    }
    Ok(())
}

pub(crate) fn text(trace: &Trace<'_>, expected: &str) -> Result<()> {
    pairs(trace, expected.chars().count())?;
    let decoded: String = trace
        .keys
        .iter()
        .filter(|key| key.pressed)
        .map(|key| key.text)
        .collect();
    if decoded != expected {
        return Err(Failure::new(format!(
            "decoded text differs: expected {expected:?}, got {decoded:?}"
        )));
    }
    Ok(())
}

fn c5a(session: &mut Session<'_>, log: &Path) -> Result<()> {
    let start = offset(log)?;
    for run in 1..=20 {
        let before = offset(log)?;
        send(session, CTRL_A, None)?;
        let seen = observed(session, log, before, 1, true)?;
        chord(&keyboard::trace(&seen)?, "a", 4, 1)?;
        session.log(&format!(
            "C5(a) Ctrl+a run {run}: one press/release with Control, final modifiers 0: pass"
        ))?;
    }
    chord(&keyboard::trace(&since(log, start)?)?, "a", 4, 20)?;
    let text_start = offset(log)?;
    for run in 1..=20 {
        let before = offset(log)?;
        send(session, &["-"], Some("Hello"))?;
        let seen = observed(session, log, before, 5, false)?;
        text(&keyboard::trace(&seen)?, "Hello")?;
        session.log(&format!(
            "C5(a) Hello run {run}: exactly five decoded pairs: pass"
        ))?;
    }
    text(
        &keyboard::trace(&since(log, text_start)?)?,
        &"Hello".repeat(20),
    )?;
    session.log("C5(a): exactly 20 Ctrl+a pairs and 100 Hello pairs: pass")
}

fn c5b(session: &mut Session<'_>, log: &Path) -> Result<()> {
    let start = offset(log)?;
    let mut gate_args = vec!["-"];
    gate_args.extend(CTRL_A);
    let mut process = session.start_with_stdin("wtype", &args(&gate_args), WTYPE_DEADLINE)?;
    let held = Instant::now();
    session.still_absent("c5b", Duration::from_secs(2), || {
        process.ensure_running()?;
        Ok(keyboard::has_input(&since(log, start)?))
    })?;
    session.log(&format!(
        "C5(b): stdin held open {:.3} s, child alive, no keys or modifiers",
        held.elapsed().as_secs_f64()
    ))?;
    process.feed(Vec::new())?;
    process.wait()?;
    let seen = observed(session, log, start, 1, true)?;
    chord(&keyboard::trace(&seen)?, "a", 4, 1)?;
    session.log("C5(b): after EOF exactly one Ctrl+a pair, final modifiers 0: pass")
}

pub(crate) fn c9(session: &mut Session<'_>, log: &Path) -> Result<()> {
    let marker = session.bind_marker();
    if marker.try_exists().context("check bind-fired")? {
        return Err(Failure::new("C9: bind-fired existed before virtual input"));
    }
    let start = offset(log)?;
    send(session, ROUTING, None)?;
    let seen = observed(session, log, start, 1, true)?;
    chord(&keyboard::trace(&seen)?, "F12", 5, 1)?;
    session.still_absent("c9", Duration::from_secs(1), || {
        marker.try_exists().context("check bind-fired")
    })?;
    chord(&keyboard::trace(&since(log, start)?)?, "F12", 5, 1)?;
    session.log("C9 virtual half: exactly one F12 pair with Control+Shift, final modifiers 0; bind-fired absent for 1 s: pass")
}

fn c10(session: &mut Session<'_>, log: &Path) -> Result<()> {
    validate_corpus(CORPUS)?;
    let mut times = Vec::new();
    for run in 1..=5 {
        let start = offset(log)?;
        let elapsed = send(session, &["-"], Some(CORPUS))?;
        let seen = observed(session, log, start, 100, false)?;
        text(&keyboard::trace(&seen)?, CORPUS)?;
        session.log(&format!(
            "C10 run {run}: {} bytes, 100 decoded pairs, {:.3} ms: pass",
            CORPUS.len(),
            elapsed.as_secs_f64() * 1000.0
        ))?;
        times.push(elapsed);
    }
    times.sort_unstable();
    let [min, _, median, _, max] = times.as_slice() else {
        return Err(Failure::new("C10 needs five durations"));
    };
    session.log(&format!(
        "C10 ms min/median/max {:.3}/{:.3}/{:.3}; 3 s >= 2x max: {}",
        min.as_secs_f64() * 1000.0,
        median.as_secs_f64() * 1000.0,
        max.as_secs_f64() * 1000.0,
        WTYPE_DEADLINE >= *max * 2
    ))
}

fn validate_corpus(corpus: &str) -> Result<()> {
    if corpus.chars().count() != 100
        || !['é', 'ß', '→'].into_iter().all(|ch| corpus.contains(ch))
        || !corpus.chars().any(|ch| ch.is_ascii_alphabetic())
    {
        return Err(Failure::new(
            "C10 corpus must have 100 characters, ASCII, é, ß and →",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wev::keyboard::Key;

    fn fixture<'a>(symbol: &'a str, text: &'a str, mask: u32) -> Trace<'a> {
        let modifiers = Modifiers {
            depressed: mask,
            ..Modifiers::default()
        };
        Trace {
            keymaps: Vec::new(),
            entered: false,
            keys: vec![
                Key {
                    time: 0,
                    code: 9,
                    pressed: true,
                    symbol,
                    text,
                    modifiers: Some(modifiers),
                },
                Key {
                    time: 0,
                    code: 9,
                    pressed: false,
                    symbol,
                    text: "",
                    modifiers: Some(modifiers),
                },
            ],
            modifiers: Some(Modifiers::default()),
            focused: true,
        }
    }

    #[test]
    fn exact_pairs_require_order_matching_keys_and_counts() {
        let mut seen = fixture("a", "a", 4);
        chord(&seen, "a", 4, 1).unwrap();
        assert!(chord(&seen, "a", 5, 1).is_err());
        assert!(chord(&seen, "F12", 4, 1).is_err());
        assert!(chord(&seen, "a", 4, 0).is_err());
        seen.keys[1].code = 10;
        assert!(pairs(&seen, 1).is_err());
        seen.keys[1].code = 9;
        seen.keys.swap(0, 1);
        assert!(pairs(&seen, 1).is_err());
        assert!(pairs(&Trace::default(), 1).is_err());
    }

    #[test]
    fn chords_require_final_released_modifiers() {
        let mut seen = fixture("F12", "", 5);
        chord(&seen, "F12", 5, 1).unwrap();
        seen.modifiers = seen.keys[0].modifiers;
        assert!(chord(&seen, "F12", 5, 1).is_err());
    }

    #[test]
    fn text_compares_decoded_unicode_not_just_event_counts() {
        let seen = fixture("eacute", "é", 0);
        text(&seen, "é").unwrap();
        assert!(text(&seen, "e").is_err());
        assert!(text(&seen, "éé").is_err());
    }

    #[test]
    fn committed_corpus_counts_characters_and_has_all_required_scripts() {
        validate_corpus(CORPUS).unwrap();
        assert_eq!(CORPUS.chars().count(), 100);
        assert!(CORPUS.len() > 100);
        assert!(validate_corpus(&format!("{CORPUS}\n")).is_err());
        assert!(validate_corpus(&CORPUS.replace('→', "x")).is_err());
    }
}
