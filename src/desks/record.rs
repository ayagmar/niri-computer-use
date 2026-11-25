//! What a desk's directory says about it: `desk.json`, the record its supervisor keeps of
//! its lifecycle and endpoints, and `host.json`, what its server takes from the host
//! session. Both are replaced whole, so a reader never sees half of one.

use std::io::Write as _;
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::check::RoundTrips;
use super::name::DeskName;
use super::scope::Generation;

/// The biggest record read back; both are a few hundred bytes.
const MAX_RECORD: u64 = 64 * 1024;

/// How the desk's niri is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Mode {
    /// Inside cage's headless backend: nothing on screen.
    Headless,
    /// As a window on the host's desktop.
    Windowed,
}

/// The headless output's size in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Size {
    pub(crate) width: u32,
    pub(crate) height: u32,
}

impl Size {
    pub(crate) const DEFAULT: Self = Self {
        width: 1920,
        height: 1080,
    };
    const MIN: u32 = 320;
    const MAX: u32 = 7680;

    /// `WxH`, each side 320 to 7680 pixels.
    pub(crate) fn parse(text: &str) -> Result<Self, String> {
        let side = |side: &str| {
            side.parse::<u32>()
                .ok()
                .filter(|value| (Self::MIN..=Self::MAX).contains(value) && !side.starts_with('0'))
        };
        text.split_once('x')
            .and_then(|(width, height)| {
                Some(Self {
                    width: side(width)?,
                    height: side(height)?,
                })
            })
            .ok_or_else(|| {
                format!(
                    "a desk's size is WIDTHxHEIGHT, each {} to {} pixels, such as 1920x1080, not {text:?}",
                    Self::MIN,
                    Self::MAX
                )
            })
    }
}

impl std::fmt::Display for Size {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}x{}", self.width, self.height)
    }
}

/// The supervisor process: with its start time, its PID names it for good.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Supervisor {
    pub(crate) pid: u32,
    pub(crate) start_time: u64,
}

/// The step a desk's start is at, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Step {
    /// The configs, and `niri validate`.
    Config,
    /// cage, and its output's size, for a headless desk.
    Parent,
    /// The session bus.
    Bus,
    Niri,
    /// The accessibility bus and registry, and `IsEnabled`.
    Accessibility,
    /// The round-trip check.
    RoundTrips,
}

/// Where a desk's server reaches it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Endpoints {
    pub(crate) niri_socket: PathBuf,
    /// A name in the desk's directory, such as `wayland-1`.
    pub(crate) wayland_display: String,
    pub(crate) session_bus: String,
    pub(crate) accessibility_bus: String,
}

/// A desk's lifecycle. A desk is `Starting` until every step passed, then `Ready` until
/// `desk stop` or a failure makes it `Stopping`, then `Stopped` once the supervisor has
/// stopped every process; a start that fails is `Failed`, once the supervisor has stopped
/// what it started. Only the supervisor writes it. The directory goes only when `desk
/// stop` or the next `desk start` has stopped the desk's scope and found it empty.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub(crate) enum Phase {
    Starting {
        step: Step,
    },
    Ready {
        endpoints: Endpoints,
        round_trips: RoundTrips,
    },
    Stopping,
    /// Why the desk stopped, and what its supervisor found left, if anything.
    Stopped {
        reason: String,
    },
    Failed {
        step: Step,
        reason: String,
    },
}

/// `desk.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Record {
    pub(crate) name: DeskName,
    pub(crate) mode: Mode,
    /// The headless output's size; a windowed desk's is its host window's.
    pub(crate) size: Option<Size>,
    pub(crate) supervisor: Supervisor,
    /// The start this record belongs to, which names its scope.
    pub(crate) generation: Generation,
    /// The scope's cgroup, as the supervisor found itself in it.
    pub(crate) cgroup: String,
    pub(crate) started: String,
    #[serde(flatten)]
    pub(crate) phase: Phase,
}

/// `host.json`: what a desk's server needs from the host session, written at
/// `desk start` from the host's resolved environment. The host's lock state is read from
/// the sources recorded in `lock`, notifications go to its session bus, `systemd-run`
/// finds the user's manager through its runtime directory, and the stop key is read
/// from its niri's config.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Host {
    pub(crate) runtime_dir: PathBuf,
    pub(crate) niri_socket: Option<PathBuf>,
    pub(crate) wayland_display: Option<String>,
    pub(crate) session_bus: Option<String>,
    pub(crate) lock: LockSources,
}

/// Where the host's lock state is read, as `desk start` found it: logind's locked hint
/// for the session the host's niri runs in, and the host's Noctalia. A desk's server reads
/// exactly these, whatever its own environment finds.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct LockSources {
    /// The logind session whose hint the host's niri sets.
    pub(crate) logind_session: Option<String>,
    /// The host's Noctalia's socket, when the host has Noctalia.
    pub(crate) noctalia: Option<PathBuf>,
}

/// Replaces the file at `path` with `value`, readable by the user alone.
pub(crate) fn write(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let text = serde_json::to_vec_pretty(value)
        .map_err(|error| format!("write {}: {error}", path.display()))?;
    let partial = path.with_extension("json.partial");
    let written = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&partial)
        .and_then(|mut file| file.write_all(&text))
        .and_then(|()| std::fs::rename(&partial, path));
    written.map_err(|error| format!("write {}: {error}", path.display()))
}

/// The record at `path`.
pub(crate) fn read<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, String> {
    use std::io::Read as _;
    let mut text = Vec::new();
    std::fs::File::open(path)
        .and_then(|file| file.take(MAX_RECORD).read_to_end(&mut text))
        .map_err(|error| format!("read {}: {error}", path.display()))?;
    serde_json::from_slice(&text).map_err(|error| format!("read {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_size_is_two_sides_in_bounds() {
        assert_eq!(
            Size::parse("1920x1080").unwrap(),
            Size {
                width: 1920,
                height: 1080
            }
        );
        assert_eq!(Size::parse("320x7680").unwrap().to_string(), "320x7680");
        for bad in [
            "",
            "1920",
            "1920x",
            "x1080",
            "1920X1080",
            "100x100",
            "9000x1080",
            "01920x1080",
            "-1x5",
            "1920x1080x2",
        ] {
            assert!(Size::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn a_record_reads_back_as_written() {
        let dir = crate::test_support::fresh_dir("desk-record");
        let path = dir.join("desk.json");
        let record = Record {
            name: DeskName::parse("web").unwrap(),
            mode: Mode::Headless,
            size: Some(Size::DEFAULT),
            supervisor: Supervisor {
                pid: 42,
                start_time: 7,
            },
            generation: Generation::parse("9").unwrap(),
            cgroup: "/app.slice/ncu-desk-web-9.scope".to_owned(),
            started: "2026-11-25T00:00:00Z".to_owned(),
            phase: Phase::Failed {
                step: Step::RoundTrips,
                reason: "slow".to_owned(),
            },
        };
        write(&path, &record).unwrap();
        assert_eq!(read::<Record>(&path).unwrap(), record);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"phase\": \"failed\""), "{text}");
        assert!(!dir.join("desk.json.partial").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
