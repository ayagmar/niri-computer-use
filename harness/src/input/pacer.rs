//! Native typing sends 1000 keys in tens of milliseconds, too fast for a check to act
//! while a call is still typing. A floating window that redraws every frame keeps the
//! nested niri rendering; measured in M7 round 3, a native key's round trip then waits
//! for a nested frame, about 15 ms. Focus takeover, stop, cancel and SIGKILL checks run
//! while it is open.

use std::ffi::OsString;
use std::time::Duration;

use niri_ipc::{Action, Request, Response};

use super::WAIT;
use crate::failure::{Context as _, Failure, Result};
use crate::runner::Process;
use crate::session::Session;

const APP_ID: &str = "org.ncu.Pacer";
const DEADLINE: Duration = Duration::from_secs(120);

/// Opens the animating window and gives focus back to the window that had it.
pub(super) fn start(session: &mut Session<'_>) -> Result<Process> {
    let focused = focused(session)?.ok_or_else(|| Failure::new("pacer: nothing focused"))?;
    let harness = std::env::current_exe().context("find the harness binary")?;
    let program = harness
        .to_str()
        .ok_or_else(|| Failure::new("the harness path isn't UTF-8"))?;
    let args: Vec<OsString> = vec![
        "window".into(),
        session.test_dir().root().into(),
        APP_ID.into(),
        "--animate".into(),
        "--deadline".into(),
        DEADLINE.as_millis().to_string().into(),
    ];
    let process = session.start(
        program,
        &args,
        session.artifact("pacer.log"),
        DEADLINE + WAIT,
    )?;
    session.wait_until("m7-pacer", "the pacing window", WAIT, |session| {
        Ok(windows(session)?
            .iter()
            .any(|window| window.app_id.as_deref() == Some(APP_ID))
            .then_some(()))
    })?;
    session.request(&Request::Action(Action::FocusWindow { id: focused }))?;
    session.wait_until("m7-pacer-focus", "focus given back", WAIT, |session| {
        Ok((self::focused(session)? == Some(focused)).then_some(()))
    })?;
    session.log("M7 pacer: a window redrawing every frame is open")?;
    Ok(process)
}

fn windows(session: &mut Session<'_>) -> Result<Vec<niri_ipc::Window>> {
    let Response::Windows(windows) = session.request(&Request::Windows)? else {
        return Err(Failure::new("no nested windows reply"));
    };
    Ok(windows)
}

fn focused(session: &mut Session<'_>) -> Result<Option<u64>> {
    Ok(windows(session)?
        .iter()
        .find(|window| window.is_focused)
        .map(|window| window.id))
}
