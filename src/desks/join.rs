//! Joining a desk: what `serve --desk` and every other subcommand that names a desk read
//! before they use it. A desk is joined only once its supervisor says it is ready and still
//! runs, and its sockets are there; nothing is started and nothing falls back to the host.
//! The joined desk keeps what its server reads from the host: the host's niri for the lock
//! state and the stop key, its Noctalia for the lock state, its runtime directory and
//! session bus for `systemd-run` and notifications, and the host-wide stop flag.

use std::ffi::OsString;
use std::os::unix::fs::FileTypeExt as _;
use std::path::{Path, PathBuf};

use super::layout::{Desks, Layout};
use super::name::DeskName;
use super::record::{self, Endpoints, Host, Phase, Record, Supervisor};

/// Where a server works: on the host session, or on one of its desks.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) enum Place {
    #[default]
    Host,
    Desk(Box<Joined>),
}

/// A ready desk, and what its server keeps of the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Joined {
    pub(crate) name: DeskName,
    pub(crate) endpoints: Endpoints,
    pub(crate) root: PathBuf,
    pub(crate) host: Host,
    /// The host-wide stop flag.
    pub(crate) stop_flag: PathBuf,
}

impl Joined {
    /// The variables this desk's server reads in place of its own environment's.
    pub(crate) fn overlay(&self) -> super::environment::Vars {
        super::environment::overlay(&super::environment::Endpoints {
            root: &self.root,
            niri_socket: &self.endpoints.niri_socket,
            wayland_display: &self.endpoints.wayland_display,
            session_bus: &self.endpoints.session_bus,
        })
    }

    /// The variables the server's children get: the desk's runtime directory, niri and
    /// display, as for the server itself, and the host's session bus, which notifications
    /// and `systemd-run` use.
    pub(crate) fn children(&self) -> Vec<(&'static str, OsString)> {
        let mut vars: Vec<(&'static str, OsString)> = vec![
            ("XDG_RUNTIME_DIR", self.root.clone().into_os_string()),
            (
                "NIRI_SOCKET",
                self.endpoints.niri_socket.clone().into_os_string(),
            ),
            (
                "WAYLAND_DISPLAY",
                self.endpoints.wayland_display.clone().into(),
            ),
        ];
        if let Some(bus) = &self.host.session_bus {
            vars.push(("DBUS_SESSION_BUS_ADDRESS", bus.into()));
        }
        vars
    }
}

/// Joins the desk `name` of the host session whose runtime directory is `host_runtime`:
/// only when it is ready, its supervisor still the process it recorded as `alive` says,
/// and its sockets there. Reads the desk's files and nothing else: the host's lock
/// sources are those `host.json` recorded at `desk start`, whatever the joining server's
/// own environment would find.
pub(crate) fn join(
    host_runtime: &Path,
    name: &DeskName,
    alive: impl Fn(Supervisor) -> bool,
) -> Result<Joined, String> {
    let desks = Desks::of(host_runtime);
    let layout = desks.desk(name)?;
    let endpoints = ready(&layout, name, alive)?;
    let host: Host = record::read(&layout.host()).map_err(|error| {
        format!("desk {name} has no readable record of its host session: {error}")
    })?;
    Ok(Joined {
        name: name.clone(),
        endpoints,
        root: layout.root().to_path_buf(),
        host,
        stop_flag: desks.stop_flag(),
    })
}

/// The desk's endpoints, once its record says it is ready, its supervisor runs and its
/// sockets exist.
fn ready(
    layout: &Layout,
    name: &DeskName,
    alive: impl Fn(Supervisor) -> bool,
) -> Result<Endpoints, String> {
    let start = format!("start it with `niri-computer-use desk start {name}`");
    if !layout.root().exists() {
        return Err(format!("there is no desk {name}; {start}"));
    }
    let record: Record = record::read(&layout.record())
        .map_err(|error| format!("desk {name} has no readable record ({error}); {start}"))?;
    let endpoints = match record.phase {
        Phase::Ready { endpoints, .. } => endpoints,
        Phase::Starting { step } => {
            return Err(format!(
                "desk {name} is still starting ({}); wait for `desk start` to finish",
                step_name(step)
            ));
        }
        Phase::Stopping => return Err(format!("desk {name} is stopping")),
        Phase::Stopped { reason } => {
            return Err(format!(
                "desk {name} stopped ({reason}); clean up with `niri-computer-use desk stop {name}`, then {start}"
            ));
        }
        Phase::Failed { step, reason } => {
            return Err(format!(
                "desk {name} failed to start at {}: {reason}; {start}",
                step_name(step)
            ));
        }
    };
    if !alive(record.supervisor) {
        return Err(format!(
            "desk {name}'s supervisor (PID {}) isn't running, so the desk is gone; clean up with `niri-computer-use desk stop {name}`, then {start}",
            record.supervisor.pid
        ));
    }
    let display = layout.root().join(&endpoints.wayland_display);
    for socket in [&endpoints.niri_socket, &display] {
        if !std::fs::symlink_metadata(socket).is_ok_and(|meta| meta.file_type().is_socket()) {
            return Err(format!(
                "desk {name} is recorded ready, but {} isn't a socket",
                socket.display()
            ));
        }
    }
    Ok(endpoints)
}

pub(crate) fn step_name(step: record::Step) -> String {
    serde_json::to_value(step)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::os::unix::net::UnixListener;

    use super::*;
    use crate::desks::check::RoundTrips;
    use crate::desks::record::{Mode, Size, Step};

    fn record(phase: Phase) -> Record {
        Record {
            name: DeskName::parse("web").unwrap(),
            mode: Mode::Headless,
            size: Some(Size::DEFAULT),
            supervisor: Supervisor {
                pid: 4242,
                start_time: 7,
            },
            generation: crate::desks::scope::Generation::parse("1").unwrap(),
            cgroup: "/app.slice/ncu-desk-web-1.scope".to_owned(),
            started: "2026-11-25T00:00:00.000Z".to_owned(),
            phase,
        }
    }

    fn ready_phase(root: &Path) -> Phase {
        Phase::Ready {
            endpoints: Endpoints {
                niri_socket: root.join("niri.wayland-1.99.sock"),
                wayland_display: "wayland-1".to_owned(),
                session_bus: "unix:path=/nowhere".to_owned(),
                accessibility_bus: "unix:path=/nowhere/at-spi".to_owned(),
            },
            round_trips: RoundTrips {
                count: 40,
                p50_us: 300,
                p95_us: 600,
                max_us: 700,
            },
        }
    }

    /// Every file under `dir`, to show joining wrote nothing.
    fn listing(dir: &Path) -> Vec<PathBuf> {
        let mut found: Vec<PathBuf> = walk(dir);
        found.sort();
        found
    }

    fn walk(dir: &Path) -> Vec<PathBuf> {
        std::fs::read_dir(dir).map_or_else(
            |_| Vec::new(),
            |entries| {
                entries
                    .filter_map(Result::ok)
                    .flat_map(|entry| {
                        let path = entry.path();
                        let mut below = walk(&path);
                        below.push(path);
                        below
                    })
                    .collect()
            },
        )
    }

    /// Catches a `serve --desk` that serves a desk that isn't there, isn't ready, or whose
    /// supervisor is gone, which would leave an agent acting on nothing or, through a
    /// fallback, on the owner's desktop; and one that makes anything while it looks.
    #[test]
    fn only_a_ready_running_desk_with_its_sockets_is_joined_and_nothing_is_written() {
        let host = crate::test_support::short_dir("join");
        let name = DeskName::parse("web").unwrap();
        let alive = |_| true;
        let missing = join(&host, &name, alive).unwrap_err();
        assert!(missing.contains("there is no desk web"), "{missing}");
        assert!(!host.join("ncu-d").exists());

        let layout = Desks::of(&host).desk(&name).unwrap();
        std::fs::create_dir_all(layout.root()).unwrap();
        let unrecorded = join(&host, &name, alive).unwrap_err();
        assert!(unrecorded.contains("no readable record"), "{unrecorded}");

        let host_record = Host {
            runtime_dir: host.clone(),
            niri_socket: Some(host.join("niri.wayland-9.1.sock")),
            wayland_display: Some("wayland-9".to_owned()),
            session_bus: Some("unix:path=/host/bus".to_owned()),
            lock: record::LockSources {
                logind_session: Some("4".to_owned()),
                noctalia: Some(host.join("noctalia-wayland-9.sock")),
            },
        };
        record::write(&layout.host(), &host_record).unwrap();
        for (phase, said) in [
            (
                Phase::Starting { step: Step::Niri },
                "still starting (niri)",
            ),
            (Phase::Stopping, "is stopping"),
            (
                Phase::Stopped {
                    reason: "niri exited".to_owned(),
                },
                "stopped (niri exited)",
            ),
            (
                Phase::Failed {
                    step: Step::RoundTrips,
                    reason: "p95 31 ms".to_owned(),
                },
                "failed to start at round_trips: p95 31 ms",
            ),
        ] {
            record::write(&layout.record(), &record(phase)).unwrap();
            let refused = join(&host, &name, alive).unwrap_err();
            assert!(refused.contains(said), "{refused}");
        }

        record::write(&layout.record(), &record(ready_phase(layout.root()))).unwrap();
        let gone = join(&host, &name, |_| false).unwrap_err();
        assert!(
            gone.contains("supervisor (PID 4242) isn't running"),
            "{gone}"
        );
        let socketless = join(&host, &name, alive).unwrap_err();
        assert!(socketless.contains("isn't a socket"), "{socketless}");

        let _niri = UnixListener::bind(layout.root().join("niri.wayland-1.99.sock")).unwrap();
        let _display = UnixListener::bind(layout.root().join("wayland-1")).unwrap();
        let before = listing(&host);
        let joined = join(&host, &name, alive).unwrap();
        assert_eq!(listing(&host), before);
        assert_eq!(joined.host, host_record);
        assert_eq!(joined.stop_flag, host.join("ncu-d/stop.flag"));
        std::fs::remove_dir_all(host).unwrap();
    }

    /// Catches a desk server whose children lose the host's session bus, which notifies the
    /// owner and reaches `systemd-run`, or keep the host's niri or display.
    #[test]
    fn the_servers_children_get_the_desk_and_the_hosts_bus() {
        let root = PathBuf::from("/run/user/1000/ncu-d/web");
        let Phase::Ready { endpoints, .. } = ready_phase(&root) else {
            panic!("ready_phase gives a ready phase");
        };
        let joined = Joined {
            name: DeskName::parse("web").unwrap(),
            endpoints,
            root: root.clone(),
            host: Host {
                runtime_dir: "/run/user/1000".into(),
                niri_socket: Some("/run/user/1000/niri.wayland-1.5.sock".into()),
                wayland_display: Some("wayland-1".to_owned()),
                session_bus: Some("unix:path=/run/user/1000/bus".to_owned()),
                lock: record::LockSources::default(),
            },
            stop_flag: "/run/user/1000/ncu-d/stop.flag".into(),
        };
        let vars: std::collections::BTreeMap<_, _> = joined.children().into_iter().collect();
        assert_eq!(vars["XDG_RUNTIME_DIR"], root.as_os_str());
        assert_eq!(
            vars["NIRI_SOCKET"],
            root.join("niri.wayland-1.99.sock").as_os_str()
        );
        assert_eq!(
            vars["DBUS_SESSION_BUS_ADDRESS"],
            "unix:path=/run/user/1000/bus"
        );
    }
}
