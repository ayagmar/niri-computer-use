//! `harness supervise`: niri's `--` child, so its environment is NESTED. niri starts it
//! with stdin, stdout and stderr closed, so it reports through files in the artifacts
//! directory, and it quits niri itself when it is done.

use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::path::Path;
use std::process::Output;
use std::time::Duration;

use niri_ipc::{Action, LogicalOutput, Request, Response, Transform};

use crate::failure::{Context as _, Failure, Result};
use crate::log::Log;
use crate::nested::Nested;
use crate::niri::Connection;
use crate::runner::{self, ChildEnv, Group, Invocation, Sink};
use crate::scale::Scale;
use crate::test_dir::TestDir;

pub(crate) const STATUS_FILE: &str = "supervise.status";
pub(crate) const LOG_FILE: &str = "supervise.log";
const STEP_DEADLINE: Duration = Duration::from_secs(10);

pub(crate) fn supervise(test_dir: &TestDir, artifacts: &Path, scale: Scale) -> Result<()> {
    let mut log = Log::create(&artifacts.join(LOG_FILE), false)?;
    // Until `identify` passes, this may not be the nested niri, so nothing more is sent to
    // it. The deadline in `harness run` kills the process group instead.
    let identified = Nested::from_env(test_dir).and_then(|nested| {
        let mut niri = Connection::open(&nested.niri)?;
        let output = identify(&nested, &mut niri, &mut log)?;
        Ok((niri, output))
    });
    let (mut niri, output) = match identified {
        Ok(identified) => identified,
        Err(failure) => {
            write_status(artifacts, Err(&failure))?;
            return Err(failure);
        }
    };
    let outcome = check_output(&output, scale, &mut log)
        .and_then(|()| screenshot(test_dir, artifacts, &mut log));
    write_status(artifacts, outcome.as_ref())?;
    // Same connection as `identify`, so this reaches the niri that was identified.
    let quit = niri.request(&Request::Action(Action::Quit {
        skip_confirmation: true,
    }));
    outcome?;
    quit?;
    Ok(())
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

fn screenshot(test_dir: &TestDir, artifacts: &Path, log: &mut Log) -> Result<()> {
    let path = artifacts.join("success-verify-niri.png");
    nested_run(
        test_dir,
        "grim",
        &["-o".into(), "winit".into(), path.into()],
    )?;
    log.line("saved success-verify-niri.png")
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

/// Every process a test step starts goes through here. It re-checks the nested endpoints
/// before each spawn, and keeps the process in the supervisor's group, so the deadline in
/// `harness run` kills it along with niri.
fn nested_run(test_dir: &TestDir, program: &'static str, args: &[OsString]) -> Result<Output> {
    Nested::from_env(test_dir)?;
    runner::run(&Invocation {
        program,
        args: args.to_vec(),
        env: ChildEnv::Inherit,
        output: Sink::Capture,
        group: Group::Caller,
        deadline: STEP_DEADLINE,
    })
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
