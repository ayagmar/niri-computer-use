//! Starts whose `desk start` is killed before the supervisor has run. A stand-in for
//! `systemd-run` first on the start's `PATH` (`fixtures/systemd-run.sh`) holds the lock it
//! was handed for a while:
//!
//! - Then it becomes the real `systemd-run`. Meanwhile a `desk stop` is refused, the start
//!   being in progress, and leaves the start alone; the desk becomes ready, and `desk stop`
//!   stops it.
//! - Or it leaves a stray process in the start's scope and exits, with no directory made.
//!   The next `desk start`, and in a second round the next `desk stop`, stop that scope.

use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;

use super::{
    DESK_DEADLINE, NAME, SIZE, WAIT, alive, copy_logs, desk, desk_root, desk_said, expect,
    scope_known,
};
use crate::failure::{Context as _, Failure, Result};
use crate::session::Session;

/// How long the stand-in holds the lock before the real `systemd-run`: past `desk stop`'s
/// 5 s wait for it.
const HELD: &str = "10";
/// How long the stand-in waits before it leaves its stray.
const STRAY_DELAY: &str = "1";
/// How long a start the stand-in delayed may take to be ready.
const LATE_READY: Duration = Duration::from_secs(40);

pub(super) fn run(session: &mut Session<'_>, server: &str) -> Result<()> {
    let fake = StandIn::write(session)?;
    let checked = delayed(session, server, &fake)
        .and_then(|()| stray(session, server, &fake, "start"))
        .and_then(|()| stray(session, server, &fake, "stop"));
    copy_logs(session);
    if checked.is_err() {
        // Whatever the failed check left: the late desk, or a stray's scope.
        desk_said(session, server, &["stop".into(), NAME.into()]).ok();
    }
    checked
}

/// The stand-in, in a directory of its own put first on `PATH`.
struct StandIn {
    bin: PathBuf,
    dir: PathBuf,
    real: PathBuf,
}

impl StandIn {
    fn write(session: &Session<'_>) -> Result<Self> {
        let dir = session.test_dir().root().join("stand-in");
        let bin = dir.join("bin");
        fs::create_dir_all(&bin).context(format!("create {}", bin.display()))?;
        let script = bin.join("systemd-run");
        fs::write(&script, include_str!("../../fixtures/systemd-run.sh"))
            .context(format!("write {}", script.display()))?;
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755))
            .context(format!("make {} executable", script.display()))?;
        let path = std::env::var_os("PATH").unwrap_or_default();
        let real = std::env::split_paths(&path)
            .map(|entry| entry.join("systemd-run"))
            .find(|candidate| candidate.is_file())
            .ok_or_else(|| Failure::new("no systemd-run on PATH"))?;
        Ok(Self { bin, dir, real })
    }

    /// Starts `desk start` with the stand-in first on its `PATH`, in `mode` after `delay`,
    /// and returns once the stand-in has started.
    fn start(
        &self,
        session: &mut Session<'_>,
        server: &str,
        mode: &str,
        delay: &str,
    ) -> Result<crate::runner::Process> {
        for file in ["started", "stray"] {
            fs::remove_file(self.dir.join(file)).ok();
        }
        let path = std::env::var_os("PATH").unwrap_or_default();
        let mut path_var = OsString::from("PATH=");
        path_var.push(&self.bin);
        path_var.push(":");
        path_var.push(&path);
        let mut args = vec![
            path_var,
            var("DESK_FAKE_DIR", &self.dir),
            var("DESK_FAKE_REAL", &self.real),
            format!("DESK_FAKE_MODE={mode}").into(),
            format!("DESK_FAKE_DELAY={delay}").into(),
            server.into(),
        ];
        args.extend(["desk", "start", "--size", SIZE, NAME].map(OsString::from));
        let log = session.artifact(&format!("desk-start-{mode}.log"));
        let start = session.start("env", &args, log, DESK_DEADLINE)?;
        let started = self.dir.join("started");
        session.wait_until("desk-stand-in", "the stand-in started", WAIT, |_| {
            Ok(started.exists().then_some(()))
        })?;
        Ok(start)
    }
}

fn var(name: &str, path: &Path) -> OsString {
    let mut var = OsString::from(format!("{name}="));
    var.push(path);
    var
}

/// A start killed while the stand-in holds its lock: `desk stop` is refused and leaves it,
/// and the late supervisor publishes the desk, which `desk stop` then stops.
fn delayed(session: &mut Session<'_>, server: &str, fake: &StandIn) -> Result<()> {
    let start = fake.start(session, server, "real", HELD)?;
    start.stop()?;
    let (stopped, said) = desk_said(session, server, &["stop".into(), NAME.into()])?;
    expect(
        !stopped && said.contains("is in progress"),
        "desk stop refused while the killed start's supervisor holds the lock",
        &said,
    )?;
    session.log(&format!(
        "desk: killed a start mid-way; desk stop said {:?}",
        said.trim()
    ))?;
    let record = desk_root(session).join("desk.json");
    session.wait_until("desk-late", "the late start ready", LATE_READY, |_| {
        let phase = fs::read_to_string(&record)
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .and_then(|record| record.get("phase").cloned());
        Ok((phase == Some(Value::from("ready"))).then_some(()))
    })?;
    desk(session, server, &["stop".into(), NAME.into()])?;
    expect(
        !desk_root(session).exists(),
        "desk stop to remove the late start's desk",
        &desk_root(session).display().to_string(),
    )?;
    session.log("desk: the killed start's supervisor published the desk, and desk stop stopped it")
}

/// A start killed while the stand-in leaves a stray in its scope and no directory: the
/// next `desk <next>` stops that scope.
fn stray(session: &mut Session<'_>, server: &str, fake: &StandIn, next: &str) -> Result<()> {
    let start = fake.start(session, server, "stray", STRAY_DELAY)?;
    start.stop()?;
    let file = fake.dir.join("stray");
    let stray: u32 = session.wait_until("desk-stray", "the stray's PID", WAIT, |_| {
        Ok(fs::read_to_string(&file)
            .ok()
            .and_then(|pid| pid.trim().parse().ok()))
    })?;
    let unit = latest_unit(session)?;
    expect(
        alive(stray) && scope_known(session, &unit)? && !desk_root(session).exists(),
        "a stray in the killed start's scope, with no directory",
        &format!("stray {stray} alive: {}, {unit}", alive(stray)),
    )?;
    let args: Vec<OsString> = match next {
        "start" => ["start", "--size", SIZE, NAME].map(OsString::from).to_vec(),
        _ => vec!["stop".into(), NAME.into()],
    };
    let (succeeded, said) = desk_said(session, server, &args)?;
    expect(
        succeeded,
        &format!("desk {next} after the stray's start"),
        &said,
    )?;
    expect(
        !alive(stray) && !scope_known(session, &unit)?,
        &format!("desk {next} to stop the scope {unit} its directory's absence hid"),
        &format!("stray {stray} alive: {}", alive(stray)),
    )?;
    if next == "start" {
        desk(session, server, &["stop".into(), NAME.into()])?;
    }
    session.log(&format!(
        "desk: desk {next} stopped {unit}, which a killed start left with a stray and no directory"
    ))
}

/// The scope unit of the desk's latest start, by the generation its lock file names.
fn latest_unit(session: &Session<'_>) -> Result<String> {
    let lock = session
        .test_dir()
        .run()
        .join("ncu-d")
        .join(format!(".{NAME}.lock"));
    let generation = fs::read_to_string(&lock).context(format!("read {}", lock.display()))?;
    Ok(format!("ncu-desk-{NAME}-{}.scope", generation.trim()))
}
