//! Native C10: the UTF-8 corpus, with the symbols the nested layout lacks on spare keys
//! of a call-long extended keymap. An unfocused client saves every map niri sends, so
//! the check compares contents: the compositor's map must come back byte for byte.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::json;

use super::{WAIT, Wev, stop};
use crate::failure::{Context as _, Failure, Result};
use crate::keyboard::{self, CORPUS};
use crate::keymaps;
use crate::mcp::{Client, structured};
use crate::runner::Process;
use crate::session::Session;
use crate::wev::keyboard::trace;

const OBSERVER_DEADLINE: Duration = Duration::from_secs(60);
/// How long a later call is watched for a keymap it should not cause.
const QUIET: Duration = Duration::from_millis(300);

pub(super) fn run(session: &mut Session<'_>, client: &mut Client, wev: &Wev<'_>) -> Result<()> {
    let (observer, directory, original) = observe(session, "native")?;
    let mut times = Vec::new();
    for run in 1..=5 {
        let before = keymaps::saved(&directory)?.len();
        let offset = wev.offset()?;
        let start = Instant::now();
        structured(&client.call(
            session,
            "type_text",
            json!({"text": CORPUS, "expect": {"app_id": "wev"}}),
        )?)?;
        let seen = keyboard::observed(session, wev.log, offset, 100, true)?;
        keyboard::text(&trace(&seen)?, CORPUS)?;
        let elapsed = start.elapsed();
        times.push(elapsed);
        restored(session, &directory, &original, before)?;
        stop::marker_gone(session)?;
        session.log(&format!("M7 native C10 run {run}: 100 decoded pairs in {:.3} ms, extended keymap sent, then the compositor's keymap byte for byte", elapsed.as_secs_f64() * 1000.0))?;
    }
    quiet_after(session, client, wev, &directory)?;
    observer.stop()?;
    times.sort_unstable();
    let [min, _, median, _, max] = times.as_slice() else {
        return Err(Failure::new("M7 native C10 needs five durations"));
    };
    session.log(&format!(
        "M7 native C10 tool-to-wev ms min/median/max {:.3}/{:.3}/{:.3}; 3 s >= 2x max: {}",
        min.as_secs_f64() * 1000.0,
        median.as_secs_f64() * 1000.0,
        max.as_secs_f64() * 1000.0,
        Duration::from_secs(3) >= *max * 2
    ))
}

/// A server killed while its extended map is niri's active one leaves that map in
/// clients; recover's fresh keyboard must send the compositor's back byte for byte.
pub(super) fn crash(session: &mut Session<'_>, wev: &Wev<'_>, server: &str) -> Result<()> {
    let (observer, directory, original) = observe(session, "crash")?;
    super::native_gestures::typing_crash(session, wev, server, "é", "extended")?;
    let maps = session.wait_until(
        "m7-crash-keymap",
        "the compositor's keymap after recover",
        WAIT,
        |_| {
            let maps = keymaps::saved(&directory)?;
            Ok((maps.len() >= 3 && maps.last() == Some(&original)).then_some(maps))
        },
    )?;
    observer.stop()?;
    if maps.get(1) == Some(&original) {
        return Err(Failure::new(
            "M7 native crash: the killed call's extended keymap never reached clients",
        ));
    }
    session.log(&format!(
        "M7 SIGKILL during an extended keymap: {} maps sent; recover sent the compositor's byte for byte",
        maps.len() - 1
    ))
}

/// What wtype's own keymap leaves behind, for comparison only: no physical key follows.
pub(super) fn wtype_comparison(
    session: &mut Session<'_>,
    client: &mut Client,
    wev: &Wev<'_>,
) -> Result<()> {
    let (observer, directory, original) = observe(session, "wtype")?;
    let offset = wev.offset()?;
    structured(&client.call(
        session,
        "type_text",
        json!({"text": CORPUS, "expect": {"app_id": "wev"}}),
    )?)?;
    keyboard::text(
        &trace(&keyboard::observed(session, wev.log, offset, 100, false)?)?,
        CORPUS,
    )?;
    // A pause: wtype leaves its map until a physical key, which never comes here.
    session.still_absent("m7-wtype-keymaps", QUIET, || Ok(false))?;
    let maps = keymaps::saved(&directory)?;
    observer.stop()?;
    session.log(&format!(
        "M7 wtype C10 keymaps sent after the observer's first: {}; compositor's keymap last: {}",
        maps.len().saturating_sub(1),
        maps.last() == Some(&original)
    ))
}

fn observe(session: &mut Session<'_>, name: &str) -> Result<(Process, PathBuf, Vec<u8>)> {
    let directory = session.artifact(&format!("keymaps-{name}"));
    fs::create_dir_all(&directory).context(format!("create {}", directory.display()))?;
    let harness = std::env::current_exe().context("find the harness binary")?;
    let args: Vec<OsString> = vec![
        "keymaps".into(),
        session.test_dir().root().into(),
        directory.clone().into(),
        OBSERVER_DEADLINE.as_millis().to_string().into(),
    ];
    let program = harness
        .to_str()
        .ok_or_else(|| Failure::new("the harness path isn't UTF-8"))?;
    let observer = session.start(
        program,
        &args,
        session.artifact(&format!("keymaps-{name}.log")),
        OBSERVER_DEADLINE + WAIT,
    )?;
    let original = session.wait_until(
        "m7-keymap-observer",
        "the observer's first keymap",
        WAIT,
        |_| Ok(keymaps::saved(&directory)?.into_iter().next()),
    )?;
    Ok((observer, directory, original))
}

/// The call sent exactly two maps to every `wl_keyboard`: the extended one, then the
/// compositor's.
fn restored(
    session: &mut Session<'_>,
    directory: &Path,
    original: &[u8],
    before: usize,
) -> Result<()> {
    let sent = session.wait_until(
        "m7-keymap-restored",
        "the compositor's keymap sent back",
        WAIT,
        |_| {
            let maps = keymaps::saved(directory)?;
            let sent = maps.get(before..).unwrap_or_default().to_vec();
            Ok((sent.last().map(Vec::as_slice) == Some(original)).then_some(sent))
        },
    )?;
    match sent.as_slice() {
        [extended, _] if extended.as_slice() != original => Ok(()),
        _ => Err(Failure::new(format!(
            "M7 native C10: expected the extended keymap then the compositor's, got {} maps",
            sent.len()
        ))),
    }
}

/// After restoration niri's active map is the compositor's again, so a call that needs
/// no extension sends no map at all.
fn quiet_after(
    session: &mut Session<'_>,
    client: &mut Client,
    wev: &Wev<'_>,
    directory: &Path,
) -> Result<()> {
    let before = keymaps::saved(directory)?.len();
    let offset = wev.offset()?;
    structured(&client.call(
        session,
        "type_text",
        json!({"text": "Hello", "expect": {"app_id": "wev"}}),
    )?)?;
    keyboard::text(
        &trace(&keyboard::observed(session, wev.log, offset, 5, true)?)?,
        "Hello",
    )?;
    session.still_absent("m7-keymap-quiet", QUIET, || {
        Ok(keymaps::saved(directory)?.len() != before)
    })?;
    session.log("M7 native: a later layout-only call sent no keymap")
}
