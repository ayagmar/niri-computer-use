//! `harness run`: preflight (stage 0), start the nested niri under a private bus
//! (stage 1), wait for the supervisor's verdict (stage 2), and compare host snapshots (C1).

use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process;
use std::time::{Duration, SystemTime};

use crate::config::{self, Decorations};
use crate::environment::{self, Env, Host};
use crate::eval;
use crate::failure::{Context as _, Failure, Result};
use crate::journal;
use crate::log::Log;
use crate::runner::{self, ChildEnv, Group, Invocation, Sink};
use crate::scale::Scale;
use crate::shared;
use crate::snapshot;
use crate::supervise;
use crate::test_dir::TestDir;

const VALIDATE_DEADLINE: Duration = Duration::from_secs(10);
const NESTED_DEADLINE: Duration = Duration::from_secs(60);
const SERVER: &str = "target/debug/niri-computer-use";
const CONTROL_DEADLINE: Duration = Duration::from_secs(90);
const ACTIONS_DEADLINE: Duration = Duration::from_secs(130);
const INPUT_DEADLINE: Duration = Duration::from_secs(180);
const SHELL_DEADLINE: Duration = Duration::from_secs(130);
const A11Y_DEADLINE: Duration = Duration::from_secs(240);
const ENGINE_DEADLINE: Duration = Duration::from_secs(150);
const MEASURE_DEADLINE: Duration = Duration::from_secs(300);
/// The agent's twelve minutes, with the fixtures and Noctalia around it.
const EVAL_DEADLINE: Duration = Duration::from_mins(15);
/// All `noctalia config validate` prints for a config without warnings. It exits 0 even
/// when it warns, for example about an unknown key.
const NOCTALIA_VALID: &str = "\u{2713} Config is valid\n";

/// What `harness run` was asked to do.
#[derive(Debug, Clone)]
pub(crate) struct Options {
    pub(crate) scale: Scale,
    pub(crate) decorations: Decorations,
    /// Draw on the host only for an explicitly requested human sitting.
    pub(crate) visible: bool,
    /// Start Noctalia in the nested session and run C13 through `niri-computer-use`.
    pub(crate) noctalia: bool,
    /// Run the supervised sitting (C6, C7, C9) at the human's pace.
    pub(crate) sitting: bool,
    /// Run checks with `niri-computer-use` and the nested Noctalia.
    pub(crate) server: Option<ServerChecks>,
    /// Run one skill eval: an agent doing one task through `niri-computer-use`.
    pub(crate) eval: Option<eval::Options>,
    pub(crate) mode: ServerMode,
    /// The server under test, instead of this checkout's debug build.
    pub(crate) binary: Option<PathBuf>,
}

/// How the servers under test serve their clients.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ServerMode {
    /// Each server serves its one client.
    Standalone,
    /// Every server bridges its client to the nested niri's shared engine.
    Shared,
}

/// The checks that run `niri-computer-use` servers against the nested niri.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ServerChecks {
    /// M2: the lease, stop, resume and recover.
    Control,
    /// M3: launch, focus and close.
    Actions,
    /// M4: the pointer tools.
    Input,
    /// M5: the shell tools and the Noctalia lock source.
    Shell,
    /// M9: the nested accessibility bus.
    A11y,
    /// The shared engine with ten clients; always in shared mode.
    Engine,
    /// What the servers cost, for docs/results/engine.md.
    Measure,
}

impl ServerChecks {
    pub(crate) const fn flag(self) -> &'static str {
        match self {
            Self::Control => "--control",
            Self::Actions => "--actions",
            Self::Input => "--input",
            Self::Shell => "--shell",
            Self::A11y => "--a11y",
            Self::Engine => "--engine",
            Self::Measure => "--measure",
        }
    }

    pub(crate) fn from_flag(flag: &str) -> Option<Self> {
        [
            Self::Control,
            Self::Actions,
            Self::Input,
            Self::Shell,
            Self::A11y,
            Self::Engine,
            Self::Measure,
        ]
        .into_iter()
        .find(|checks| checks.flag() == flag)
    }

    const fn deadline(self) -> Duration {
        match self {
            Self::Control => CONTROL_DEADLINE,
            Self::Actions => ACTIONS_DEADLINE,
            Self::Input => INPUT_DEADLINE,
            Self::Shell => SHELL_DEADLINE,
            Self::A11y => A11Y_DEADLINE,
            Self::Engine => ENGINE_DEADLINE,
            Self::Measure => MEASURE_DEADLINE,
        }
    }
}

pub(crate) fn run(options: &Options) -> Result<()> {
    let host = Host::from_env()?;
    let stamp = stamp()?;
    let artifacts = create_artifacts(&stamp)?;
    let mut log = Log::create(&artifacts.join("harness.log"), true)?;
    // Short, so that the shared engine's socket fits a Unix socket's 108 bytes.
    let test_dir = TestDir::create(&host.runtime_dir.join("ncu-test"), &stamp)?;
    log.line(&format!("TEST_DIR {}", test_dir.root().display()))?;
    log.line(&format!("artifacts {}", artifacts.display()))?;

    let mut outcome = run_nested(&host, &test_dir, &artifacts, options, &mut log);
    if options.mode == ServerMode::Shared {
        let checked = shared::check(&test_dir, &artifacts, &mut log);
        outcome = outcome.and(checked);
    }
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
    options: &Options,
    log: &mut Log,
) -> Result<()> {
    let display = if options.visible {
        host.visible_socket()?
    } else {
        test_dir.root().join("cage/wayland-0")
    };
    let env = preflight(host, test_dir, artifacts, options, &display)?;
    log.line("stage 0, preflight: pass")?;
    if !options.visible {
        log.line("headless cage: isolated; C1 host snapshots enabled")?;
    }
    let journal = journal::mark()?;
    let nested = snapshot::checked(
        log,
        || snapshot::take(host),
        || run_session(&env, test_dir, artifacts, options),
    );
    let journal = journal::check(&journal, log);
    for line in fs::read_to_string(artifacts.join(supervise::LOG_FILE))
        .unwrap_or_default()
        .lines()
    {
        log.line(&format!("  {line}"))?;
    }
    nested?;
    journal?;
    log.line("stages 1-2, nested niri and private bus: pass")
}

/// The headless parent is part of the lifecycle C1 observes, including cleanup.
fn run_session(env: &Env, test_dir: &TestDir, artifacts: &Path, options: &Options) -> Result<()> {
    let cage = if options.visible {
        None
    } else {
        Some(crate::headless::Headless::start(
            test_dir.root(),
            env,
            artifacts,
            deadline(options) + VALIDATE_DEADLINE,
        )?)
    };
    let nested = start_nested(env, test_dir, artifacts, options);
    let cleanup = cage.map_or(Ok(()), crate::headless::Headless::stop);
    cleanup.and(nested)
}

/// Stage 0: generated configs, PARENT, containment, `niri validate`, and with Noctalia,
/// `noctalia config validate`.
fn preflight(
    host: &Host,
    test_dir: &TestDir,
    artifacts: &Path,
    options: &Options,
    display: &Path,
) -> Result<Env> {
    let niri_config = config::niri(options.scale, &test_dir.bind_marker(), options.decorations);
    write(&test_dir.niri_config(), &niri_config)?;
    write(&artifacts.join("niri.kdl"), &niri_config)?;
    write(&test_dir.dbus_config(), &config::dbus(&test_dir.run()))?;

    let env = environment::parent(test_dir, host, display, options.mode == ServerMode::Shared);
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
    if options.noctalia || options.server.is_some() || options.eval.is_some() {
        write_noctalia_config(&env, test_dir, artifacts)?;
    }
    Ok(env)
}

fn write_noctalia_config(env: &Env, test_dir: &TestDir, artifacts: &Path) -> Result<()> {
    let path = test_dir.noctalia_config();
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).context(format!("create {}", dir.display()))?;
    }
    let wallpapers = test_dir.data().join("wallpapers");
    fs::create_dir_all(&wallpapers).context(format!("create {}", wallpapers.display()))?;
    let config = config::noctalia(&wallpapers);
    write(&path, &config)?;
    write(&artifacts.join("noctalia.toml"), &config)?;
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
fn start_nested(env: &Env, test_dir: &TestDir, artifacts: &Path, options: &Options) -> Result<()> {
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
    ];
    if options.decorations == Decorations::Server {
        args.push(Decorations::SERVER_FLAG.into());
    }
    if options.sitting {
        args.push("--sitting".into());
    } else if options.noctalia {
        args.push("--noctalia".into());
        args.push(server(options)?.into());
    } else if let Some(checks) = options.server {
        args.push(checks.flag().into());
        args.push(server(options)?.into());
    } else if let Some(eval) = &options.eval {
        args.extend(eval_args(eval, options)?);
    }
    runner::run(&Invocation {
        program: "dbus-run-session",
        args,
        env: ChildEnv::Exact(env),
        output: Sink::File(artifacts.join("niri.log")),
        group: Group::Own,
        deadline: deadline(options),
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

const fn deadline(options: &Options) -> Duration {
    match (options.sitting, options.server, &options.eval) {
        (true, _, _) => crate::sitting::RUN_DEADLINE,
        (false, Some(checks), _) => checks.deadline(),
        (false, None, Some(_)) => EVAL_DEADLINE,
        (false, None, None) => NESTED_DEADLINE,
    }
}

/// `--eval <server> <scenario> <skill directory | none> <model>` for the supervisor.
fn eval_args(eval: &eval::Options, options: &Options) -> Result<Vec<OsString>> {
    let skill = eval.skill.as_ref().map_or_else(
        || OsString::from("none"),
        |skill| skill.clone().into_os_string(),
    );
    Ok(vec![
        "--eval".into(),
        server(options)?.into(),
        eval.scenario.name().into(),
        skill,
        eval.model.clone().into(),
    ])
}

/// The `niri-computer-use` binary the make targets build.
/// The server under test: `--server`'s, or this checkout's debug build.
fn server(options: &Options) -> Result<PathBuf> {
    let path = match &options.binary {
        Some(path) => path.clone(),
        None => env::current_dir()
            .context("read the working directory")?
            .join(SERVER),
    };
    if path.is_file() {
        Ok(path)
    } else {
        Err(Failure::new(format!(
            "{} is missing; build it with `cargo build -p niri-computer-use`",
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
        let root = Path::new("/run/user/1000/ncu-test/1-2");
        let clone = b"git\0clone\0https://github.com/x\0/run/user/1000/ncu-test/1-2/state/x\0";
        assert_eq!(
            naming(clone, root).as_deref(),
            Some("git clone https://github.com/x /run/user/1000/ncu-test/1-2/state/x")
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
