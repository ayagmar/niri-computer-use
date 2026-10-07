//! `harness supervise`: niri's `--` child, so its environment is NESTED. niri starts it
//! with stdin, stdout and stderr closed, so it reports through files in the artifacts
//! directory, and it quits niri itself when it is done.

use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::path::Path;
use std::time::Duration;

use niri_ipc::{LogicalOutput, Request, Response, Transform, WindowLayout};

use crate::failure::{Context as _, Failure, Result};
use crate::log::Log;
use crate::nested::Nested;
use crate::niri::Connection;
use crate::pointer::{self, Probe};
use crate::scale::Scale;
use crate::session::Session;
use crate::test_dir::TestDir;
use crate::{capture, keyboard};

pub(crate) const STATUS_FILE: &str = "supervise.status";
pub(crate) const LOG_FILE: &str = "supervise.log";
/// Below the 60 s deadline `harness run` gives the whole nested run.
const WEV_DEADLINE: Duration = Duration::from_secs(45);
const WAIT: Duration = Duration::from_secs(5);

pub(crate) fn supervise(
    test_dir: &TestDir,
    artifacts: &Path,
    scale: Scale,
    probe: &str,
) -> Result<()> {
    let mut log = Log::create(&artifacts.join(LOG_FILE), false)?;
    // Until `identify` passes, this may not be the nested niri, so nothing more is sent to
    // it. The deadline in `harness run` kills the process group instead.
    let identified = Nested::from_env(test_dir).and_then(|nested| {
        let mut niri = Connection::open(&nested.niri)?;
        let output = identify(&nested, &mut niri, &mut log)?;
        Ok((niri, output))
    });
    let (niri, output) = match identified {
        Ok(identified) => identified,
        Err(failure) => {
            write_status(artifacts, Err(&failure))?;
            return Err(failure);
        }
    };
    let c2 = check_output(&output, scale, &mut log);
    let mut session = Session::new(test_dir, artifacts, log, niri);
    let outcome = c2.and_then(|()| steps(&mut session, &output, probe));
    write_status(artifacts, outcome.as_ref())?;
    // Same connection as `identify`, so this reaches the niri that was identified.
    let quit = session.quit();
    outcome?;
    quit
}

/// Stage 4: the M0 checks against `wev`.
fn steps(session: &mut Session<'_>, output: &LogicalOutput, probe: &str) -> Result<()> {
    session.screenshot("success-verify-niri.png")?;
    session.log("saved success-verify-niri.png")?;
    let wev_log = session.artifact("wev.log");
    // wev doesn't flush stdout, which is block-buffered into a file, so `stdbuf` makes it
    // line-buffered for the reads below.
    let args = ["-oL", "wev"].map(OsString::from);
    let wev = session.start("stdbuf", &args, wev_log.clone(), WEV_DEADLINE)?;
    let window = wait_for_wev(session)?;
    capture::c3(session, output, &window)?;
    pointer::run(
        session,
        &Probe {
            path: probe,
            output,
        },
        &wev_log,
        &window,
    )?;
    keyboard::run(session, &wev_log)?;
    capture::nested_c15(session, output)?;
    wev.stop().map(drop)
}

/// The window rule makes `wev` a 400x300 floating window at the top-left.
fn wait_for_wev(session: &mut Session<'_>) -> Result<WindowLayout> {
    let layout = session.wait_until("wev-window", "a 400x300 floating wev", WAIT, |session| {
        let Response::Windows(windows) = session.request(&Request::Windows)? else {
            return Err(Failure::new("niri answered Windows with another response"));
        };
        Ok(windows.into_iter().find_map(|window| {
            let layout = window.layout;
            let placed = window.app_id.as_deref() == Some("wev")
                && window.is_floating
                && layout.window_size == (400, 300)
                && layout.tile_pos_in_workspace_view.is_some();
            placed.then_some(layout)
        }))
    })?;
    session.log(&format!("wev window: {layout:?}"))?;
    Ok(layout)
}

fn write_status(artifacts: &Path, outcome: std::result::Result<&(), &Failure>) -> Result<()> {
    let status = match outcome {
        Ok(()) => "pass".to_owned(),
        Err(failure) => format!("fail: {failure}"),
    };
    let path = artifacts.join(STATUS_FILE);
    fs::write(&path, status).context(format!("write {}", path.display()))
}

/// Stage 2: logs the endpoints and the niri version, and requires that niri's only output
/// is `winit`. That is what makes it safe to send `Quit` on this connection: the host niri
/// drives real outputs.
fn identify(nested: &Nested, niri: &mut Connection, log: &mut Log) -> Result<LogicalOutput> {
    log.line(&format!("NIRI_SOCKET={}", nested.niri.display()))?;
    log.line(&format!("WAYLAND_DISPLAY={}", nested.wayland.display()))?;
    log.line(&format!(
        "DBUS_SESSION_BUS_ADDRESS socket={}",
        nested.dbus.display()
    ))?;

    let Response::Version(version) = niri.request(&Request::Version)? else {
        return Err(Failure::new("niri answered Version with another response"));
    };
    log.line(&format!("niri version: {version}"))?;

    let Response::Outputs(outputs) = niri.request(&Request::Outputs)? else {
        return Err(Failure::new("niri answered Outputs with another response"));
    };
    only_winit(outputs)
}

/// C2: the configured scale, and the transform niri forces on `winit`.
fn check_output(output: &LogicalOutput, scale: Scale, log: &mut Log) -> Result<()> {
    if !scale.matches(output.scale) || output.transform != Transform::Flipped180 {
        return Err(Failure::new(format!(
            "winit has scale {} and {:?}, expected {scale} and Flipped180",
            output.scale, output.transform
        )));
    }
    log.line(&format!("outputs: winit only, scale {scale}, Flipped180"))
}

fn only_winit(mut outputs: HashMap<String, niri_ipc::Output>) -> Result<LogicalOutput> {
    let names: Vec<String> = outputs.keys().cloned().collect();
    let winit = outputs.remove("winit").filter(|_| outputs.is_empty());
    let Some(winit) = winit else {
        return Err(Failure::new(format!(
            "expected only the winit output, got {names:?}"
        )));
    };
    winit
        .logical
        .ok_or_else(|| Failure::new("the winit output has no logical geometry"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outputs(name: &str, scale: f64, transform: &str) -> HashMap<String, niri_ipc::Output> {
        let json = format!(
            r#"{{"{name}":{{"name":"{name}","make":"m","model":"m","serial":null,
            "physical_size":null,"modes":[{{"width":1280,"height":800,"refresh_rate":60000,
            "is_preferred":true}}],"current_mode":0,"is_custom_mode":false,
            "vrr_supported":false,"vrr_enabled":false,"logical":{{"x":0,"y":0,
            "width":1280,"height":800,"scale":{scale},"transform":"{transform}"}}}}}}"#
        );
        serde_json::from_str(&json).unwrap()
    }

    #[test]
    fn identifies_only_a_lone_winit_output() {
        only_winit(outputs("winit", 1.0, "Flipped180")).unwrap();
        assert!(only_winit(outputs("DP-1", 1.0, "Normal")).is_err());
        let mut two = outputs("winit", 1.0, "Flipped180");
        two.extend(outputs("DP-1", 1.0, "Normal"));
        assert!(only_winit(two).is_err());
    }

    #[test]
    fn c2_checks_scale_and_transform() {
        let log_path = std::env::temp_dir().join(format!("harness-c2-{}", std::process::id()));
        let mut log = Log::create(&log_path, false).unwrap();
        let scale = "1.5".parse().unwrap();
        let output =
            |reported, transform| only_winit(outputs("winit", reported, transform)).unwrap();
        check_output(&output(1.5, "Flipped180"), scale, &mut log).unwrap();
        assert!(check_output(&output(1.0, "Flipped180"), scale, &mut log).is_err());
        assert!(check_output(&output(1.5, "Normal"), scale, &mut log).is_err());
        fs::remove_file(log_path).unwrap();
    }
}
