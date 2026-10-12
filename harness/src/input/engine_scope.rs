//! E6: the shared engine's own systemd scope, against the user's real systemd manager.
//! The nested session can't reach it otherwise: `systemd-run` and `systemctl --user` look
//! for `$XDG_RUNTIME_DIR/systemd/private`, and the nested `XDG_RUNTIME_DIR` is
//! `TEST_DIR/run`. For this check only, that path links to the manager's socket in
//! `/run/user/<uid>`, where systemd puts it. Every scope the check creates is transient
//! and collected once empty.

use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use serde_json::json;

use super::SERVER_DEADLINE;
use super::engine::{engine_pid, guardian_pids};
use crate::failure::{Context as _, Failure, Result};
use crate::mcp::{Client, field, structured};
use crate::session::Session;
use crate::shared::no_engine_left;

/// The description the bridge gives the engine's scope.
const DESCRIPTION: &str = "niri-computer-use shared engine";

/// E6: client X, in a scope of its own, starts the engine, and client Y joins it. The
/// engine and its guardian run in a third scope, so stopping X's scope leaves Y served by
/// the same engine; once Y leaves, the engine exits and its scope is collected. Skipped
/// when the host has no user systemd socket.
pub(super) fn run(session: &mut Session<'_>, server: &str) -> Result<()> {
    let uid = rustix::process::getuid().as_raw();
    let host = PathBuf::from(format!("/run/user/{uid}/systemd/private"));
    if !host.exists() {
        return session.log(&format!(
            "E6: skipped, no user systemd at {}",
            host.display()
        ));
    }
    no_engine_left(session, "e6-idle")?;
    let dir = session.test_dir().run().join("systemd");
    fs::create_dir_all(&dir).context(format!("create {}", dir.display()))?;
    let link = dir.join("private");
    symlink(&host, &link).context(format!("link {}", link.display()))?;
    let result = scoped(session, server);
    fs::remove_file(&link).context(format!("remove {}", link.display()))?;
    result
}

fn scoped(session: &mut Session<'_>, server: &str) -> Result<()> {
    let unit = format!("ncu-e6-x-{}.scope", std::process::id());
    let args: Vec<OsString> = [
        "--user",
        "--scope",
        "--collect",
        "--quiet",
        &format!("--unit={unit}"),
        "--",
        server,
        "serve",
    ]
    .map(OsString::from)
    .into();
    let x = Client::start_command(
        session,
        "systemd-run",
        &args,
        "harness-e6-x",
        SERVER_DEADLINE,
    )?;
    let mut y = Client::start(session, server, "harness-e6-y", SERVER_DEADLINE)?;
    let status = structured(&y.call(session, "status", json!({}))?)?;
    let engine = engine_pid(&status)?;
    let scope = engine_scope(engine, &unit)?;
    if description(session, &scope)? != DESCRIPTION {
        return Err(Failure::new(format!(
            "E6: {scope} isn't the engine's scope"
        )));
    }
    session.run(
        "systemctl",
        &["--user".into(), "stop".into(), unit.clone().into()],
    )?;
    if x.wait().is_ok() {
        return Err(Failure::new("E6: X's server outlived its scope"));
    }
    let after = structured(&y.call(session, "status", json!({}))?)?;
    if engine_pid(&after)? != engine {
        return Err(Failure::new(format!(
            "E6: Y is served by {} after X's scope stopped, not engine {engine}",
            field(&after, "/engine/pid")
        )));
    }
    y.stop()?;
    no_engine_left(session, "e6-exit")?;
    session.wait_until(
        "e6-collected",
        "the engine's scope collected",
        super::WAIT,
        |session| Ok((load_state(session, &scope)? == "not-found").then_some(())),
    )?;
    session.log(&format!(
        "E6: X in {unit} started engine {engine}, which runs with its guardian in {scope}; stopping X's scope left Y served by it, and once Y left the engine exited and {scope} was collected"
    ))
}

/// The scope the engine runs in, after checking that its guardian shares it and that it
/// is neither X's scope nor the harness's cgroup.
fn engine_scope(engine: u64, x: &str) -> Result<String> {
    let cgroup = cgroup_of(&engine.to_string())?;
    let guardians = guardian_pids(engine)?;
    let [guardian] = guardians.as_slice() else {
        return Err(Failure::new(format!(
            "E6: engine {engine} has guardians {guardians:?}"
        )));
    };
    let theirs = cgroup_of(guardian)?;
    if theirs != cgroup {
        return Err(Failure::new(format!(
            "E6: the guardian runs in {theirs}, the engine in {cgroup}"
        )));
    }
    let unit = Path::new(&cgroup);
    let scope = unit
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let is_scope = unit.extension().is_some_and(|suffix| suffix == "scope");
    if !is_scope || scope == x || cgroup == cgroup_of("self")? {
        return Err(Failure::new(format!(
            "E6: engine {engine} runs in {cgroup}, not a scope of its own"
        )));
    }
    Ok(scope)
}

/// The cgroup v2 path of `/proc/<pid>`.
fn cgroup_of(pid: &str) -> Result<String> {
    let path = Path::new("/proc").join(pid).join("cgroup");
    let text = fs::read_to_string(&path).context(format!("read {}", path.display()))?;
    text.lines()
        .find_map(|line| line.strip_prefix("0::"))
        .map(str::to_owned)
        .ok_or_else(|| Failure::new(format!("no cgroup v2 line in {}", path.display())))
}

fn description(session: &Session<'_>, unit: &str) -> Result<String> {
    show(session, unit, "Description")
}

fn load_state(session: &Session<'_>, unit: &str) -> Result<String> {
    show(session, unit, "LoadState")
}

fn show(session: &Session<'_>, unit: &str, property: &str) -> Result<String> {
    let args = ["--user", "show", "--value", "-p", property, unit].map(OsString::from);
    let output = session.run("systemctl", &args)?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}
