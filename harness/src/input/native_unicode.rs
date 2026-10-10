//! Native C10: the UTF-8 corpus, with the symbols the nested layout lacks on spare keys
//! of a call-long extended keymap. An unfocused client saves every map niri sends, so
//! the check compares contents: the compositor's map must come back byte for byte.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use niri_ipc::{Action, LayoutSwitchTarget, Request, Response};
use rustix::process::{Pid, Signal, kill_process};
use serde_json::{Value, json};

use super::{WAIT, Wev, stop};
use crate::failure::{Context as _, Failure, Result};
use crate::keyboard::{self, CORPUS};
use crate::keymaps;
use crate::mcp::{Client, field, structured};
use crate::runner::Process;
use crate::session::{Session, pause};
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
/// clients; its guardian's fresh keyboard must send the compositor's back byte for byte,
/// before `recover`.
pub(super) fn crash(session: &mut Session<'_>, wev: &Wev<'_>, server: &str) -> Result<()> {
    let (observer, directory, original) = observe(session, "crash")?;
    let mut sent = 0;
    super::native_gestures::typing_crash(session, wev, server, ("é", "extended"), |session| {
        let maps = session.wait_until(
            "m7-crash-keymap",
            "the compositor's keymap from the guardian",
            WAIT,
            |_| {
                let maps = keymaps::saved(&directory)?;
                Ok((maps.len() >= 3 && maps.last() == Some(&original)).then_some(maps))
            },
        )?;
        if maps.get(1) == Some(&original) {
            return Err(Failure::new(
                "M7 native crash: the killed call's extended keymap never reached clients",
            ));
        }
        sent = maps.len() - 1;
        Ok(())
    })?;
    observer.stop()?;
    session.log(&format!(
        "M7 SIGKILL during an extended keymap: {sent} maps sent; the guardian sent the compositor's byte for byte before recover"
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

/// A layout change while a call's extended keymap is what clients hold: first niri's
/// keymap, given a second layout, then the active layout. The serving process is stopped
/// as soon as the extension reaches clients and continued after the change, so the change
/// lands mid-call. The call must end early and leave the latest compositor keymap with
/// clients, and the focused client in the layout niri switched to; neither the keymap nor
/// the layout the call began with may come back. niri's config is put back at the end.
pub(super) fn layout_change(
    session: &mut Session<'_>,
    client: &mut Client,
    wev: &Wev<'_>,
) -> Result<()> {
    let config = session.test_dir().niri_config();
    let original_config =
        fs::read_to_string(&config).context(format!("read {}", config.display()))?;
    let (observer, directory, original) = observe(session, "layout")?;
    let serving = serving_pid(session, client)?;
    let changed = mid_call(session, client, (serving, &directory), |session| {
        write_config(&config, &format!("{original_config}{TWO_LAYOUTS}"))?;
        session.wait_until("m7-layout-keymap", "niri's two-layout keymap", WAIT, |_| {
            Ok(keymaps::saved(&directory)?
                .into_iter()
                .find(|map| two_layouts(map)))
        })
    })?;
    let (outcome, base) = changed;
    expect_outcome(&outcome, "uncertain", "the compositor keymap changed")?;
    clients_hold(session, &directory, &base, "the new keymap")?;
    let offset = wev.offset()?;
    let switched = mid_call(session, client, (serving, &directory), |session| {
        session.request(&Request::Action(Action::SwitchLayout {
            layout: LayoutSwitchTarget::Index(1),
        }))?;
        session.wait_until(
            "m7-layout-switch",
            "the second layout active",
            WAIT,
            |session| Ok((current_layout(session)? == 1).then_some(())),
        )
    })?;
    expect_outcome(&switched.0, "interrupted", "")?;
    clients_hold(
        session,
        &directory,
        &base,
        "the new keymap after the switch",
    )?;
    // The release and restore after the call must not send the focused client back to
    // the layout the call began in. niri's own layout index doesn't follow a virtual
    // keyboard's group, so only the client shows it.
    pause(QUIET);
    let log = wev.read()?;
    let group = trace(log.get(offset..).unwrap_or_default())?
        .modifiers
        .map(|modifiers| modifiers.group);
    if group != Some(1) {
        return Err(Failure::new(format!(
            "M7 layout change: wev was left in group {group:?} after the call, not 1"
        )));
    }
    write_config(&config, &original_config)?;
    clients_hold(session, &directory, &original, "the original keymap")?;
    observer.stop()?;
    session.log(
        "M7 native layout change: a new compositor keymap and a layout switch mid-extension each ended the call, and clients kept the new keymap, wev in the layout switched to",
    )
}

const TWO_LAYOUTS: &str =
    "\ninput {\n    keyboard {\n        xkb {\n            layout \"us,de\"\n        }\n    }\n}\n";

fn write_config(config: &Path, text: &str) -> Result<()> {
    fs::write(config, text).context(format!("write {}", config.display()))
}

/// Whether `map` names a second group, written `name[2]` or `name[Group2]`.
fn two_layouts(map: &[u8]) -> bool {
    String::from_utf8_lossy(map).lines().any(|line| {
        let line = line.trim_start();
        line.starts_with("name[2]") || line.starts_with("name[Group2]")
    })
}

/// The process that types for `client`: its server, or in shared mode the engine.
fn serving_pid(session: &mut Session<'_>, client: &mut Client) -> Result<i32> {
    let status = structured(&client.call(session, "status", json!({}))?)?;
    field(&status, "/engine/pid")
        .as_i64()
        .and_then(|pid| i32::try_from(pid).ok())
        .ok_or_else(|| Failure::new(format!("status names no serving PID: {status}")))
}

fn current_layout(session: &mut Session<'_>) -> Result<u8> {
    let Response::KeyboardLayouts(layouts) = session.request(&Request::KeyboardLayouts)? else {
        return Err(Failure::new(
            "niri answered KeyboardLayouts with something else",
        ));
    };
    Ok(layouts.current_idx)
}

/// Types a long text that needs an extended keymap, stops `serving` once niri has sent
/// the extension to clients, runs `change`, continues `serving`, and returns the call's
/// structured result with what `change` returned.
fn mid_call<T>(
    session: &mut Session<'_>,
    client: &mut Client,
    (serving, directory): (i32, &Path),
    change: impl FnOnce(&mut Session<'_>) -> Result<T>,
) -> Result<(Value, T)> {
    let before = keymaps::saved(directory)?.len();
    let id = client.start_call(
        "type_text",
        json!({"text": "é".repeat(1000), "expect": {"app_id": "wev"}}),
    )?;
    let until = Instant::now() + WAIT;
    while keymaps::saved(directory)?.len() <= before {
        if Instant::now() > until {
            return Err(Failure::new(
                "M7 layout change: no extended keymap reached clients",
            ));
        }
        pause(Duration::from_millis(1));
    }
    signal(serving, Signal::STOP)?;
    let changed = change(session);
    signal(serving, Signal::CONT)?;
    let changed = changed?;
    let outcome = structured(&client.result(session, id)?)?;
    stop::marker_gone(session)?;
    Ok((outcome, changed))
}

fn expect_outcome(outcome: &Value, observed: &str, detail: &str) -> Result<()> {
    let typed = field(outcome, "/typed").as_u64();
    let matches = field(outcome, "/observed") == observed
        && field(outcome, "/detail")
            .as_str()
            .unwrap_or_default()
            .contains(detail)
        && typed.is_some_and(|typed| typed < 1000);
    if matches {
        return Ok(());
    }
    Err(Failure::new(format!(
        "M7 layout change: expected {observed} after part of the text, got {outcome}"
    )))
}

/// Waits until the last map niri sent clients is `map`.
fn clients_hold(session: &mut Session<'_>, directory: &Path, map: &[u8], what: &str) -> Result<()> {
    session.wait_until("m7-layout-restored", what, WAIT, |_| {
        Ok((keymaps::saved(directory)?.last().map(Vec::as_slice) == Some(map)).then_some(()))
    })
}

fn signal(pid: i32, signal: Signal) -> Result<()> {
    let pid = Pid::from_raw(pid).ok_or_else(|| Failure::new("no serving PID"))?;
    kill_process(pid, signal).context(format!("send {signal:?} to the serving process"))
}
