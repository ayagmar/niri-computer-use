//! A desk's systemd scope. `desk start` runs the supervisor through `systemd-run --user
//! --scope`, as the unit `ncu-desk-<name>-<generation>.scope`, so the whole desk, every
//! process the supervisor starts and every process those start, is in the scope's cgroup
//! however it forks. Stopping a desk stops that unit and then checks its cgroup is empty
//! before anything is reported stopped or removed. Without a user manager there is no
//! desk: no unscoped fallback.

use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::time::Instant;

use super::name::DeskName;
use crate::runner::{self, Search};

/// Where the unified cgroup hierarchy is mounted.
pub(crate) const CGROUP_ROOT: &str = "/sys/fs/cgroup";
const SYSTEMCTL: &str = "systemctl";
/// How long systemd gives what is left in a stopping scope after `SIGTERM`, before it
/// sends `SIGKILL`.
const STOP_TIMEOUT: &str = "5s";
/// The most `systemctl show` may print.
const MAX_SHOW: u64 = 16 * 1024;
/// How often a stop looks whether the scope's cgroup is empty yet.
const POLL: Duration = Duration::from_millis(100);
/// How many of the processes left a failed stop names.
const NAMED: usize = 10;

/// One start of a desk: the unit's name and the record's tie to it. A directory is removed
/// only by whoever stopped the unit of the generation its record names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct Generation(String);

impl Generation {
    const MAX: usize = 20;

    /// A new generation: the nanoseconds since the epoch. Starts of one desk are
    /// serialized by its lock, so no two of one desk share one.
    pub(crate) fn new() -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos());
        Self(nanos.to_string())
    }

    /// 1 to 20 ASCII digits.
    pub(crate) fn parse(text: &str) -> Result<Self, String> {
        let digits = !text.is_empty()
            && text.len() <= Self::MAX
            && text.bytes().all(|byte| byte.is_ascii_digit());
        if digits {
            Ok(Self(text.to_owned()))
        } else {
            Err(format!(
                "a desk's generation is 1 to {} digits, not {text:?}",
                Self::MAX
            ))
        }
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

/// The scope unit of the start `generation` of the desk `name`.
pub(crate) fn unit(name: &DeskName, generation: &Generation) -> String {
    format!("ncu-desk-{name}-{}.scope", generation.as_str())
}

/// `systemd-run`'s arguments to run `exe` with `supervise` in the scope `unit`. systemd
/// collects the unit once it is inactive, failed or not, and stops what a stop's
/// `SIGTERM` left with `SIGKILL` after `STOP_TIMEOUT`.
pub(crate) fn run_args(
    unit: &str,
    name: &DeskName,
    exe: &str,
    supervise: &[String],
) -> Vec<String> {
    let mut args = vec![
        "--user".to_owned(),
        "--scope".to_owned(),
        "--collect".to_owned(),
        "--quiet".to_owned(),
        format!("--unit={unit}"),
        format!("--property=TimeoutStopSec={STOP_TIMEOUT}"),
        format!("--description=niri-computer-use desk {name}"),
        "--".to_owned(),
        exe.to_owned(),
    ];
    args.extend_from_slice(supervise);
    args
}

/// The cgroup a process is in, from its `/proc/<pid>/cgroup`: the unified hierarchy's
/// `0::<path>` line.
pub(crate) fn cgroup_of(text: &str) -> Option<String> {
    text.lines()
        .find_map(|line| line.strip_prefix("0::"))
        .filter(|path| path.starts_with('/'))
        .map(str::to_owned)
}

/// Whether `cgroup` is the cgroup of the unit `unit`.
pub(crate) fn is_unit(cgroup: &str, unit: &str) -> bool {
    cgroup.rsplit('/').next() == Some(unit)
}

/// What a scope's cgroup holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Contents {
    Empty,
    /// What is left, as `PID (name)`, the first `NAMED` of them.
    Left(Vec<String>),
}

/// The processes in `cgroup` itself under `root`, by `cgroup.procs`.
pub(crate) fn members(root: &Path, cgroup: &str) -> Result<Vec<u32>, String> {
    let file = cgroup_dir(root, cgroup)?.join("cgroup.procs");
    let text = std::fs::read_to_string(&file)
        .map_err(|error| format!("read {}: {error}", file.display()))?;
    Ok(text
        .lines()
        .filter_map(|pid| pid.trim().parse().ok())
        .collect())
}

fn cgroup_dir(root: &Path, cgroup: &str) -> Result<std::path::PathBuf, String> {
    if cgroup.split('/').any(|part| part == "..") || !cgroup.starts_with('/') {
        return Err(format!("{cgroup:?} isn't a cgroup path"));
    }
    Ok(root.join(cgroup.trim_start_matches('/')))
}

/// What `cgroup` holds under `root`: nothing once its directory is gone or its
/// `cgroup.events` says `populated 0`, which counts its sub-cgroups too. `proc_root`
/// names what is left.
pub(crate) fn contents(root: &Path, cgroup: &str, proc_root: &Path) -> Result<Contents, String> {
    let dir = cgroup_dir(root, cgroup)?;
    let events = dir.join("cgroup.events");
    let text = match std::fs::read_to_string(&events) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && !dir.exists() => {
            return Ok(Contents::Empty);
        }
        read => read.map_err(|error| format!("read {}: {error}", events.display()))?,
    };
    let populated = text
        .lines()
        .find_map(|line| line.strip_prefix("populated "))
        .ok_or_else(|| format!("{} has no populated line", events.display()))?;
    if populated.trim() == "0" {
        return Ok(Contents::Empty);
    }
    let mut left: Vec<String> = members(root, cgroup)
        .unwrap_or_default()
        .into_iter()
        .take(NAMED)
        .map(|pid| {
            let name = crate::control::procs::comm(proc_root, pid).unwrap_or_default();
            format!("{pid} ({name})")
        })
        .collect();
    if left.is_empty() {
        left.push(format!("processes in cgroups under {cgroup}"));
    }
    Ok(Contents::Left(left))
}

/// What `systemctl show` says about a unit.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Shown {
    pub(crate) active_state: String,
    /// `None` once the unit has no cgroup, as when it is gone.
    pub(crate) control_group: Option<String>,
}

impl Shown {
    pub(crate) fn parse(text: &str) -> Self {
        let mut shown = Self::default();
        for line in text.lines() {
            if let Some(state) = line.strip_prefix("ActiveState=") {
                state.clone_into(&mut shown.active_state);
            } else if let Some(group) = line.strip_prefix("ControlGroup=") {
                shown.control_group = Some(group.to_owned()).filter(|group| !group.is_empty());
            }
        }
        shown
    }

    /// Whether the unit runs nothing: inactive or failed, as a collected unit reads.
    fn ended(&self) -> bool {
        matches!(self.active_state.as_str(), "inactive" | "failed")
    }
}

/// Stops the scope `unit` and waits, until `until`, for its cgroup to be empty: the one
/// `recorded` when the supervisor recorded it, else the one systemd names. `Ok` only once
/// it is checked empty; otherwise what is left, and what `systemctl` said.
pub(crate) async fn stop(unit: &str, recorded: Option<&str>, until: Instant) -> Result<(), String> {
    let stopped = systemctl(&["stop", unit], until).await.err();
    emptied(unit, stopped, until, async || {
        check(unit, recorded, until).await
    })
    .await
}

/// Checks with `check` until the scope is empty or `until` has passed; then what is left,
/// and `stopped`, the stop's own error, if it had one.
async fn emptied(
    unit: &str,
    stopped: Option<String>,
    until: Instant,
    mut check: impl AsyncFnMut() -> Result<Contents, String>,
) -> Result<(), String> {
    loop {
        let checked = check().await;
        if matches!(checked, Ok(Contents::Empty)) {
            return Ok(());
        }
        if Instant::now() >= until {
            let left = match checked {
                Ok(Contents::Left(left)) => format!("{unit} still holds {}", left.join(", ")),
                Ok(Contents::Empty) => String::new(),
                Err(error) => format!("couldn't check {unit} is empty: {error}"),
            };
            return Err(match stopped {
                Some(stop) => format!("{left}; {stop}"),
                None => left,
            });
        }
        tokio::time::sleep(POLL).await;
    }
}

/// What the scope's cgroup holds now.
async fn check(unit: &str, recorded: Option<&str>, until: Instant) -> Result<Contents, String> {
    let root = Path::new(CGROUP_ROOT);
    let proc_root = Path::new("/proc");
    if let Some(cgroup) = recorded {
        return contents(root, cgroup, proc_root);
    }
    let shown = systemctl(
        &["show", "-p", "ActiveState", "-p", "ControlGroup", unit],
        until,
    )
    .await
    .map(|text| Shown::parse(&text))?;
    match &shown.control_group {
        Some(cgroup) => contents(root, cgroup, proc_root),
        None if shown.ended() => Ok(Contents::Empty),
        None => Err(format!(
            "systemd says {unit} is {} without a cgroup",
            shown.active_state
        )),
    }
}

/// `systemctl --user <args>`, until `until`: its stdout, or its exit and stderr.
async fn systemctl(args: &[&str], until: Instant) -> Result<String, String> {
    let mut all = vec!["--user".to_owned()];
    all.extend(args.iter().map(|arg| (*arg).to_owned()));
    let left = until.saturating_duration_since(Instant::now()).max(POLL);
    let done = runner::run(SYSTEMCTL, Search::Own, &all, left, MAX_SHOW)
        .await
        .map_err(|error| error.detail)?;
    if !done.status.success() {
        return Err(done.failure(SYSTEMCTL).detail);
    }
    Ok(String::from_utf8_lossy(&done.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name() -> DeskName {
        DeskName::parse("web").unwrap()
    }

    #[test]
    fn a_generation_names_one_scope_and_is_digits_alone() {
        let generation = Generation::parse("1764000000000000001").unwrap();
        assert_eq!(
            unit(&name(), &generation),
            "ncu-desk-web-1764000000000000001.scope"
        );
        for bad in ["", "12a", "../x", "123456789012345678901", "-1"] {
            assert!(Generation::parse(bad).is_err(), "{bad:?}");
        }
        assert!(Generation::parse(Generation::new().as_str()).is_ok());
        let args = run_args("u.scope", &name(), "/proc/7/exe", &["desk".to_owned()]);
        assert_eq!(
            args[..5],
            [
                "--user",
                "--scope",
                "--collect",
                "--quiet",
                "--unit=u.scope"
            ]
        );
        assert_eq!(args[args.len() - 3..], ["--", "/proc/7/exe", "desk"]);
    }

    #[test]
    fn the_unified_cgroup_names_the_unit() {
        let text =
            "0::/user.slice/user-1000.slice/user@1000.service/app.slice/ncu-desk-web-1.scope\n";
        let cgroup = cgroup_of(text).unwrap();
        assert!(is_unit(&cgroup, "ncu-desk-web-1.scope"));
        assert!(!is_unit(&cgroup, "web-1.scope"));
        assert_eq!(cgroup_of("1:name=systemd:/x\n"), None);
    }

    #[test]
    fn show_reads_the_state_and_a_cgroup_only_when_there_is_one() {
        let gone = Shown::parse("ActiveState=inactive\nControlGroup=\n");
        assert_eq!(gone.control_group, None);
        assert!(gone.ended());
        let running = Shown::parse("ControlGroup=/a/b.scope\nActiveState=active\n");
        assert_eq!(running.control_group.as_deref(), Some("/a/b.scope"));
        assert!(!running.ended());
    }

    /// Catches a stop that calls a scope empty while a process is left in it, or in a
    /// cgroup below it, or that can't tell a gone cgroup from an unreadable one.
    #[test]
    fn a_cgroup_is_empty_only_when_gone_or_unpopulated() {
        let dir = crate::test_support::fresh_dir("desk-cgroup");
        let proc_root = dir.join("proc");
        std::fs::create_dir_all(proc_root.join("42")).unwrap();
        std::fs::write(proc_root.join("42/comm"), "sleep\n").unwrap();
        let scope = dir.join("cg/app.slice/d.scope");
        assert_eq!(
            contents(&dir.join("cg"), "/app.slice/d.scope", &proc_root),
            Ok(Contents::Empty)
        );
        std::fs::create_dir_all(&scope).unwrap();
        let unreadable = contents(&dir.join("cg"), "/app.slice/d.scope", &proc_root);
        assert!(unreadable.is_err(), "{unreadable:?}");
        std::fs::write(scope.join("cgroup.events"), "populated 1\nfrozen 0\n").unwrap();
        std::fs::write(scope.join("cgroup.procs"), "42\n").unwrap();
        assert_eq!(
            contents(&dir.join("cg"), "/app.slice/d.scope", &proc_root),
            Ok(Contents::Left(vec!["42 (sleep)".to_owned()]))
        );
        std::fs::write(scope.join("cgroup.procs"), "").unwrap();
        assert!(matches!(
            contents(&dir.join("cg"), "/app.slice/d.scope", &proc_root),
            Ok(Contents::Left(_))
        ));
        std::fs::write(scope.join("cgroup.events"), "populated 0\nfrozen 0\n").unwrap();
        assert_eq!(
            contents(&dir.join("cg"), "/app.slice/d.scope", &proc_root),
            Ok(Contents::Empty)
        );
        assert!(contents(&dir.join("cg"), "/app.slice/../x", &proc_root).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Catches a stop that says stopped while something is still in the scope when the
    /// shared deadline ends, or that gives up before a slow process has gone.
    #[tokio::test]
    async fn a_stop_is_done_only_once_the_scope_is_empty_by_the_deadline() {
        let left_once = || Ok(Contents::Left(vec!["7 (sleep)".to_owned()]));
        let checks = std::cell::Cell::new(0);
        let later = Instant::now() + Duration::from_secs(5);
        let emptied_late = emptied("d.scope", None, later, async || {
            checks.set(checks.get() + 1);
            (checks.get() < 3)
                .then(left_once)
                .unwrap_or(Ok(Contents::Empty))
        })
        .await;
        assert_eq!(emptied_late, Ok(()));
        let soon = Instant::now() + Duration::from_millis(300);
        let left = emptied(
            "d.scope",
            Some("systemctl timed out".to_owned()),
            soon,
            async || Ok(Contents::Left(vec!["7 (sleep)".to_owned()])),
        )
        .await
        .unwrap_err();
        assert_eq!(left, "d.scope still holds 7 (sleep); systemctl timed out");
        let unchecked = emptied("d.scope", None, Instant::now(), async || {
            Err("gone".to_owned())
        })
        .await
        .unwrap_err();
        assert!(unchecked.contains("couldn't check"), "{unchecked}");
    }
}
