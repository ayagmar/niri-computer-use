//! `harness run`: preflight (stage 0), start the nested niri under a private bus
//! (stage 1), wait for the supervisor's verdict (stage 2), and compare host snapshots (C1).

use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process;
use std::time::{Duration, SystemTime};

use crate::config;
use crate::environment::{self, Env, Host};
use crate::failure::{Context as _, Failure, Result};
use crate::log::Log;
use crate::runner::{self, ChildEnv, Group, Invocation, Sink};
use crate::scale::Scale;
use crate::snapshot;
use crate::supervise;
use crate::test_dir::TestDir;

const VALIDATE_DEADLINE: Duration = Duration::from_secs(10);
const NESTED_DEADLINE: Duration = Duration::from_secs(60);

pub(crate) fn run(scale: Scale) -> Result<()> {
    let host = Host::from_env()?;
    let seconds = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .context("read the clock")?
        .as_secs();
    let stamp = run_name(seconds, process::id());
    let artifacts = create_artifacts(&stamp)?;
    let mut log = Log::create(&artifacts.join("harness.log"), true)?;
    let test_dir = TestDir::create(&host.runtime_dir.join("niri-desktop-mcp-test"), &stamp)?;
    log.line(&format!("TEST_DIR {}", test_dir.root().display()))?;
    log.line(&format!("artifacts {}", artifacts.display()))?;

    let outcome = run_nested(&host, &test_dir, &artifacts, scale, &mut log);
    let removed = fs::remove_dir_all(test_dir.root())
        .context(format!("remove {}", test_dir.root().display()));
    let outcome = outcome.and(removed);
    log.line(match &outcome {
        Ok(()) => "result: pass",
        Err(_) => "result: fail",
    })?;
    outcome
}

/// Unix time plus the harness process ID, so runs started in the same second don't collide.
fn run_name(seconds: u64, pid: u32) -> String {
    format!("{seconds}-{pid}")
}

fn create_artifacts(stamp: &str) -> Result<PathBuf> {
    let base = env::current_dir().context("read the working directory")?;
    let base = base.join("target/e2e");
    fs::create_dir_all(&base).context(format!("create {}", base.display()))?;
    let artifacts = base.join(stamp);
    fs::create_dir(&artifacts).context(format!("create {}", artifacts.display()))?;
    Ok(artifacts)
}

fn run_nested(
    host: &Host,
    test_dir: &TestDir,
    artifacts: &Path,
    scale: Scale,
    log: &mut Log,
) -> Result<()> {
    let env = preflight(host, test_dir, artifacts, scale)?;
    log.line("stage 0, preflight: pass")?;

    let before = snapshot::take(host)?;
    let nested = start_nested(&env, test_dir, artifacts, scale);
    let after = snapshot::take(host);
    for line in fs::read_to_string(artifacts.join(supervise::LOG_FILE))
        .unwrap_or_default()
        .lines()
    {
        log.line(&format!("  {line}"))?;
    }
    nested?;
    log.line("stages 1-2, nested niri and private bus: pass")?;

    let changes = snapshot::diff(&before, &after?);
    for change in &changes {
        log.line(&format!("  {change}"))?;
    }
    if changes.is_empty() {
        log.line("C1, host snapshot: unchanged")
    } else {
        Err(Failure::new("C1, host snapshot: changed"))
    }
}

/// Stage 0: generated configs, PARENT, containment, and `niri validate`.
fn preflight(host: &Host, test_dir: &TestDir, artifacts: &Path, scale: Scale) -> Result<Env> {
    let niri_config = config::niri(scale, &test_dir.bind_marker());
    write(&test_dir.niri_config(), &niri_config)?;
    write(&artifacts.join("niri.kdl"), &niri_config)?;
    write(&test_dir.dbus_config(), &config::dbus(&test_dir.run()))?;

    let env = environment::parent(test_dir, host);
    environment::check_containment(&env, test_dir)?;
    runner::run(&Invocation {
        program: "niri",
        args: vec![
            "validate".into(),
            "-c".into(),
            test_dir.niri_config().into(),
        ],
        env: ChildEnv::Exact(&env),
        output: Sink::Capture,
        group: Group::Own,
        deadline: VALIDATE_DEADLINE,
    })?;
    Ok(env)
}

/// Stages 1 and 2: `dbus-run-session -- niri -c … -- harness supervise …`. The supervisor
/// quits niri when it is done, which ends the bus session.
fn start_nested(env: &Env, test_dir: &TestDir, artifacts: &Path, scale: Scale) -> Result<()> {
    let harness = env::current_exe().context("find the harness binary")?;
    let mut bus_config = OsString::from("--config-file=");
    bus_config.push(test_dir.dbus_config());
    let args = vec![
        bus_config,
        "--".into(),
        "niri".into(),
        "-c".into(),
        test_dir.niri_config().into(),
        "--".into(),
        harness.into(),
        "supervise".into(),
        test_dir.root().into(),
        artifacts.into(),
        scale.to_string().into(),
    ];
    runner::run(&Invocation {
        program: "dbus-run-session",
        args,
        env: ChildEnv::Exact(env),
        output: Sink::File(artifacts.join("niri.log")),
        group: Group::Own,
        deadline: NESTED_DEADLINE,
    })?;
    let status_path = artifacts.join(supervise::STATUS_FILE);
    let status = fs::read_to_string(&status_path)
        .context(format!("read {}; see niri.log", status_path.display()))?;
    if status == "pass" {
        Ok(())
    } else {
        Err(Failure::new(format!("supervisor: {status}")))
    }
}

fn write(path: &Path, contents: &str) -> Result<()> {
    fs::write(path, contents).context(format!("write {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_in_the_same_second_get_different_names() {
        assert_ne!(run_name(1_791_334_343, 100), run_name(1_791_334_343, 101));
        assert_eq!(run_name(1_791_334_343, 100), "1791334343-100");
    }
}
