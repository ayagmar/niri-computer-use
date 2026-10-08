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
const VPOINTER: &str = "probes/vpointer/target/debug/vpointer";
const NOCTALIA_SOCKET: &str = "probes/noctalia-socket/target/debug/noctalia-socket";
/// All `noctalia config validate` prints for a config without warnings. It exits 0 even
/// when it warns, for example about an unknown key.
const NOCTALIA_VALID: &str = "\u{2713} Config is valid\n";

/// What `harness run` was asked to do.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Options {
    pub(crate) scale: Scale,
    /// Start Noctalia in the nested session and run C13.
    pub(crate) noctalia: bool,
    pub(crate) sitting: Option<crate::sitting::Mode>,
}

pub(crate) fn run(options: Options) -> Result<()> {
    let host = Host::from_env()?;
    let stamp = stamp()?;
    let artifacts = create_artifacts(&stamp)?;
    let mut log = Log::create(&artifacts.join("harness.log"), true)?;
    let test_dir = TestDir::create(&host.runtime_dir.join("niri-desktop-mcp-test"), &stamp)?;
    log.line(&format!("TEST_DIR {}", test_dir.root().display()))?;
    log.line(&format!("artifacts {}", artifacts.display()))?;

    let outcome = run_nested(&host, &test_dir, &artifacts, options, &mut log);
    // Checked even when the run failed: that is when a stray process is most likely.
    let leftovers = no_leftovers(test_dir.root(), &mut log);
    let outcome = outcome.and(leftovers);
    let removed = fs::remove_dir_all(test_dir.root())
        .context(format!("remove {}", test_dir.root().display()));
    let outcome = outcome.and(removed);
    log.line(match &outcome {
        Ok(()) => "result: pass",
        Err(_) => "result: fail",
    })?;
    outcome
}

/// Fails if a process still running names `TEST_DIR` in its command line. Everything the
/// run started should be gone once `dbus-run-session`'s group is killed, but a program can
/// move a child into a group of its own. Such processes are reported, not killed.
fn no_leftovers(root: &Path, log: &mut Log) -> Result<()> {
    let found = leftovers(root)?;
    for process in &found {
        log.line(&format!("  still running: {process}"))?;
    }
    if found.is_empty() {
        log.line("leftover processes: none")
    } else {
        Err(Failure::new(format!(
            "{} processes naming TEST_DIR outlived the run",
            found.len()
        )))
    }
}

fn leftovers(root: &Path) -> Result<Vec<String>> {
    let mut found = Vec::new();
    for entry in fs::read_dir("/proc").context("list /proc")? {
        let entry = entry.context("list /proc")?;
        let name = entry.file_name();
        let Some(pid) = name
            .to_str()
            .filter(|name| name.bytes().all(|b| b.is_ascii_digit()))
        else {
            continue;
        };
        // A process can exit between the listing and this read.
        let Ok(cmdline) = fs::read(entry.path().join("cmdline")) else {
            continue;
        };
        if let Some(command) = naming(&cmdline, root) {
            found.push(format!("{pid} {command}"));
        }
    }
    Ok(found)
}

/// The command line, NUL-separated as in `/proc/<pid>/cmdline`, if it names `root`.
fn naming(cmdline: &[u8], root: &Path) -> Option<String> {
    let command = String::from_utf8_lossy(cmdline).replace('\0', " ");
    let command = command.trim_end();
    command
        .contains(root.to_string_lossy().as_ref())
        .then(|| command.to_owned())
}

/// The name of this run's directories.
pub(crate) fn stamp() -> Result<String> {
    let seconds = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .context("read the clock")?
        .as_secs();
    Ok(run_name(seconds, process::id()))
}

/// Unix time plus the harness process ID, so runs started in the same second don't collide.
fn run_name(seconds: u64, pid: u32) -> String {
    format!("{seconds}-{pid}")
}

/// `target/e2e/<stamp>` under the working directory.
pub(crate) fn create_artifacts(stamp: &str) -> Result<PathBuf> {
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
    options: Options,
    log: &mut Log,
) -> Result<()> {
    let env = preflight(host, test_dir, artifacts, options)?;
    log.line("stage 0, preflight: pass")?;

    let before = snapshot::take(host)?;
    let nested = start_nested(&env, test_dir, artifacts, options);
    let after = snapshot::take(host);
    for line in fs::read_to_string(artifacts.join(supervise::LOG_FILE))
        .unwrap_or_default()
        .lines()
    {
        log.line(&format!("  {line}"))?;
    }
    // Compared before the nested result is checked, so a failed run still reports what
    // changed on the host.
    let c1 = snapshot::report(log, &before, after);
    nested?;
    log.line("stages 1-2, nested niri and private bus: pass")?;
    c1
}

/// Stage 0: generated configs, PARENT, containment, `niri validate`, and with Noctalia,
/// `noctalia config validate`.
fn preflight(host: &Host, test_dir: &TestDir, artifacts: &Path, options: Options) -> Result<Env> {
    let niri_config = config::niri(options.scale, &test_dir.bind_marker());
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
    if options.noctalia {
        write_noctalia_config(&env, test_dir, artifacts)?;
    }
    Ok(env)
}

fn write_noctalia_config(env: &Env, test_dir: &TestDir, artifacts: &Path) -> Result<()> {
    let path = test_dir.noctalia_config();
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).context(format!("create {}", dir.display()))?;
    }
    write(&path, config::NOCTALIA)?;
    write(&artifacts.join("noctalia.toml"), config::NOCTALIA)?;
    let output = runner::run(&Invocation {
        program: "noctalia",
        args: vec!["config".into(), "validate".into(), path.into()],
        env: ChildEnv::Exact(env),
        output: Sink::Capture,
        group: Group::Own,
        deadline: VALIDATE_DEADLINE,
    })?;
    noctalia_valid(&output.stdout, &output.stderr)
}

fn noctalia_valid(stdout: &[u8], stderr: &[u8]) -> Result<()> {
    if stdout == NOCTALIA_VALID.as_bytes() && stderr.is_empty() {
        return Ok(());
    }
    Err(Failure::new(format!(
        "noctalia config validate: {}{}",
        String::from_utf8_lossy(stdout).trim_end(),
        String::from_utf8_lossy(stderr).trim_end()
    )))
}

/// Stages 1 and 2: `dbus-run-session -- niri -c … -- harness supervise …`. The supervisor
/// quits niri when it is done, which ends the bus session.
fn start_nested(env: &Env, test_dir: &TestDir, artifacts: &Path, options: Options) -> Result<()> {
    let harness = env::current_exe().context("find the harness binary")?;
    let mut bus_config = OsString::from("--config-file=");
    bus_config.push(test_dir.dbus_config());
    let mut args = vec![
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
        options.scale.to_string().into(),
        probe(VPOINTER)?.into(),
    ];
    if let Some(mode) = options.sitting {
        args.push(mode.flag().into());
    } else if options.noctalia {
        args.push(probe(NOCTALIA_SOCKET)?.into());
    }
    runner::run(&Invocation {
        program: "dbus-run-session",
        args,
        env: ChildEnv::Exact(env),
        output: Sink::File(artifacts.join("niri.log")),
        group: Group::Own,
        deadline: if options.sitting.is_some() {
            crate::sitting::RUN_DEADLINE
        } else {
            NESTED_DEADLINE
        },
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

/// A probe built by `make nested`.
fn probe(relative: &str) -> Result<PathBuf> {
    let path = env::current_dir()
        .context("read the working directory")?
        .join(relative);
    if path.is_file() {
        Ok(path)
    } else {
        Err(Failure::new(format!(
            "{} is missing; build it with `make nested`",
            path.display()
        )))
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

    #[test]
    fn a_command_line_naming_test_dir_is_a_leftover() {
        let root = Path::new("/run/user/1000/niri-desktop-mcp-test/1-2");
        let clone =
            b"git\0clone\0https://github.com/x\0/run/user/1000/niri-desktop-mcp-test/1-2/state/x\0";
        assert_eq!(
            naming(clone, root).as_deref(),
            Some("git clone https://github.com/x /run/user/1000/niri-desktop-mcp-test/1-2/state/x")
        );
        assert_eq!(naming(b"harness\0run\0--noctalia\0", root), None);
        assert_eq!(naming(b"", root), None);
    }

    #[test]
    fn noctalia_config_must_validate_without_warnings() {
        noctalia_valid("\u{2713} Config is valid\n".as_bytes(), b"").unwrap();
        let warned = "WARN  /t/config.toml:2:23: shell.setup_wizard_enable: unknown setting\n\n\
                      \u{2713} Config is valid (1 warning(s))\n";
        let error = noctalia_valid(warned.as_bytes(), b"").unwrap_err();
        assert!(error.to_string().contains("unknown setting"), "{error}");
        assert!(noctalia_valid(NOCTALIA_VALID.as_bytes(), b"note\n").is_err());
    }
}
