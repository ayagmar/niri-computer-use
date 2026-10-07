//! `harness supervise`: niri's `--` child, so its environment is NESTED. niri starts it
//! with stdin, stdout and stderr closed, so it reports through files in the artifacts
//! directory, and it quits niri itself when it is done.

use std::collections::HashMap;
use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;
use std::time::Duration;

use niri_ipc::{Action, LogicalOutput, Request, Response, Transform};

use crate::environment::is_under;
use crate::failure::{Context as _, Failure, Result};
use crate::log::Log;
use crate::niri::Connection;
use crate::runner::{self, ChildEnv, Group, Invocation, Sink};
use crate::scale::Scale;
use crate::test_dir::TestDir;

pub(crate) const STATUS_FILE: &str = "supervise.status";
pub(crate) const LOG_FILE: &str = "supervise.log";
const STEP_DEADLINE: Duration = Duration::from_secs(10);

/// The socket paths of the nested endpoints, each checked to be under `TEST_DIR/run`.
#[derive(Debug)]
struct Nested {
    niri: PathBuf,
    wayland: PathBuf,
    dbus: PathBuf,
}

impl Nested {
    fn from_env(test_dir: &TestDir) -> Result<Self> {
        let nested = Self::check(test_dir, |name| env::var_os(name))?;
        nested.resolve(&test_dir.run())?;
        Ok(nested)
    }

    /// Reads the endpoints through `lookup` and refuses any that is not under
    /// `TEST_DIR/run`. Nothing is ever sent to a niri that fails this check.
    fn check(test_dir: &TestDir, lookup: impl Fn(&str) -> Option<OsString>) -> Result<Self> {
        let variable =
            |name: &str| lookup(name).ok_or_else(|| Failure::new(format!("{name} is not set")));
        let run = test_dir.run();
        let runtime_dir = PathBuf::from(variable("XDG_RUNTIME_DIR")?);
        if runtime_dir != run {
            return Err(Failure::new(format!(
                "XDG_RUNTIME_DIR={} is not {}",
                runtime_dir.display(),
                run.display()
            )));
        }
        let address = variable("DBUS_SESSION_BUS_ADDRESS")?;
        let nested = Self {
            niri: PathBuf::from(variable("NIRI_SOCKET")?),
            wayland: runtime_dir.join(variable("WAYLAND_DISPLAY")?),
            dbus: dbus_socket(&address.to_string_lossy())?,
        };
        for (name, path) in [
            ("NIRI_SOCKET", &nested.niri),
            ("WAYLAND_DISPLAY", &nested.wayland),
            ("DBUS_SESSION_BUS_ADDRESS", &nested.dbus),
        ] {
            if !is_under(path, &run) {
                return Err(Failure::new(format!(
                    "{name} points to {}, outside {}",
                    path.display(),
                    run.display()
                )));
            }
        }
        Ok(nested)
    }

    /// Follows symlinks: `run` must be its own canonical path, and each endpoint must
    /// resolve to a path under it.
    fn resolve(&self, run: &Path) -> Result<()> {
        let canonical = fs::canonicalize(run).context(format!("resolve {}", run.display()))?;
        if canonical != run {
            return Err(Failure::new(format!(
                "{} resolves to {}",
                run.display(),
                canonical.display()
            )));
        }
        for path in [&self.niri, &self.wayland, &self.dbus] {
            let resolved = fs::canonicalize(path).context(format!("resolve {}", path.display()))?;
            if !is_under(&resolved, run) {
                return Err(Failure::new(format!(
                    "{} resolves to {}, outside {}",
                    path.display(),
                    resolved.display(),
                    run.display()
                )));
            }
        }
        Ok(())
    }
}

/// The socket path of a single `unix:path=…` D-Bus address.
fn dbus_socket(address: &str) -> Result<PathBuf> {
    let path = address
        .strip_prefix("unix:")
        .filter(|rest| !rest.contains([';', '%']))
        .and_then(|rest| rest.split(',').find_map(|pair| pair.strip_prefix("path=")));
    path.map(PathBuf::from)
        .ok_or_else(|| Failure::new(format!("unsupported D-Bus address {address:?}")))
}

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
        let log_path = env::temp_dir().join(format!("harness-c2-{}", std::process::id()));
        let mut log = Log::create(&log_path, false).unwrap();
        let scale = "1.5".parse().unwrap();
        let output =
            |reported, transform| only_winit(outputs("winit", reported, transform)).unwrap();
        check_output(&output(1.5, "Flipped180"), scale, &mut log).unwrap();
        assert!(check_output(&output(1.0, "Flipped180"), scale, &mut log).is_err());
        assert!(check_output(&output(1.5, "Normal"), scale, &mut log).is_err());
        fs::remove_file(log_path).unwrap();
    }

    fn nested_env(test_dir: &TestDir) -> HashMap<&'static str, OsString> {
        let run = test_dir.run();
        HashMap::from([
            ("XDG_RUNTIME_DIR", run.clone().into_os_string()),
            ("WAYLAND_DISPLAY", OsString::from("wayland-1")),
            (
                "NIRI_SOCKET",
                run.join("niri.wayland-1.7.sock").into_os_string(),
            ),
            (
                "DBUS_SESSION_BUS_ADDRESS",
                OsString::from(format!("unix:path={}/dbus-x,guid=1", run.display())),
            ),
        ])
    }

    fn check_with(test_dir: &TestDir, env: &HashMap<&'static str, OsString>) -> Result<Nested> {
        Nested::check(test_dir, |name| env.get(name).cloned())
    }

    #[test]
    fn accepts_endpoints_under_test_dir_run() {
        let test_dir = TestDir::open(PathBuf::from("/r/t")).unwrap();
        check_with(&test_dir, &nested_env(&test_dir)).unwrap();
    }

    #[test]
    fn refuses_any_host_endpoint() {
        let test_dir = TestDir::open(PathBuf::from("/r/t")).unwrap();
        let host = [
            ("XDG_RUNTIME_DIR", "/run/user/1000"),
            ("NIRI_SOCKET", "/run/user/1000/niri.wayland-1.1725.sock"),
            ("WAYLAND_DISPLAY", "/run/user/1000/wayland-1"),
            ("DBUS_SESSION_BUS_ADDRESS", "unix:path=/run/user/1000/bus"),
        ];
        for (name, value) in host {
            let mut env = nested_env(&test_dir);
            env.insert(name, OsString::from(value));
            assert!(check_with(&test_dir, &env).is_err(), "{name}");
        }
        let mut missing = nested_env(&test_dir);
        missing.remove("NIRI_SOCKET");
        assert!(check_with(&test_dir, &missing).is_err());
    }

    #[test]
    fn resolve_refuses_an_endpoint_symlinked_outside_run() {
        let base = fs::canonicalize(env::temp_dir())
            .unwrap()
            .join(format!("harness-resolve-{}", std::process::id()));
        let test_dir = TestDir::create(&base, "t").unwrap();
        let run = test_dir.run();
        for name in ["niri.sock", "wayland-1", "dbus-x"] {
            fs::write(run.join(name), "").unwrap();
        }
        let nested = Nested {
            niri: run.join("niri.sock"),
            wayland: run.join("wayland-1"),
            dbus: run.join("dbus-x"),
        };
        nested.resolve(&run).unwrap();

        fs::write(base.join("host.sock"), "").unwrap();
        std::os::unix::fs::symlink(base.join("host.sock"), run.join("alias.sock")).unwrap();
        let aliased = Nested {
            niri: run.join("alias.sock"),
            ..nested
        };
        assert!(aliased.resolve(&run).is_err());
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn reads_the_socket_of_a_unix_path_address() {
        let path = dbus_socket("unix:path=/t/run/dbus-ab,guid=12").unwrap();
        assert_eq!(path, Path::new("/t/run/dbus-ab"));
    }

    #[test]
    fn rejects_other_and_multiple_addresses() {
        for address in [
            "unix:abstract=/tmp/dbus-x,guid=1",
            "tcp:host=localhost,port=1",
            "unix:path=/t/a;unix:path=/t/b",
            "unix:path=/t/%2e%2e/x",
        ] {
            assert!(dbus_socket(address).is_err(), "{address}");
        }
    }
}
