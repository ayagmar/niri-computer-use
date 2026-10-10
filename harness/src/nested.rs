//! The nested endpoints: the niri, Wayland and D-Bus sockets the supervisor inherits, and
//! the socket a nested Noctalia creates. Each must resolve to a path under `TEST_DIR/run`
//! before anything is sent to it.

use std::env;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use crate::environment::is_under;
use crate::failure::{Context as _, Failure, Result};
use crate::test_dir::TestDir;

/// The socket paths of the nested endpoints, each checked to be under `TEST_DIR/run`.
#[derive(Debug)]
pub(crate) struct Nested {
    pub(crate) niri: PathBuf,
    pub(crate) wayland: PathBuf,
    pub(crate) dbus: PathBuf,
    /// Where a Noctalia started in NESTED puts its IPC socket. It exists only while one runs.
    noctalia: PathBuf,
    /// `DBUS_SYSTEM_BUS_ADDRESS`, `TEST_DIR/run/no-system-bus`. Nothing may exist there,
    /// so there is no system bus.
    pub(crate) system_bus: PathBuf,
}

impl Nested {
    pub(crate) fn from_env(test_dir: &TestDir) -> Result<Self> {
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
        let display = variable("WAYLAND_DISPLAY")?;
        let nested = Self {
            niri: PathBuf::from(variable("NIRI_SOCKET")?),
            wayland: runtime_dir.join(&display),
            dbus: dbus_socket(&address.to_string_lossy())?,
            noctalia: runtime_dir.join(noctalia_socket_name(&display)),
            system_bus: dbus_socket(&variable("DBUS_SYSTEM_BUS_ADDRESS")?.to_string_lossy())?,
        };
        if nested.system_bus != test_dir.system_bus() {
            return Err(Failure::new(format!(
                "DBUS_SYSTEM_BUS_ADDRESS points to {}, not {}",
                nested.system_bus.display(),
                test_dir.system_bus().display()
            )));
        }
        for (name, path) in [
            ("NIRI_SOCKET", &nested.niri),
            ("WAYLAND_DISPLAY", &nested.wayland),
            ("DBUS_SESSION_BUS_ADDRESS", &nested.dbus),
            ("the Noctalia socket", &nested.noctalia),
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
    /// resolve to a path under it. Nothing may exist at the system bus path.
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
            resolve_under(path, run)?;
        }
        nothing_at(&self.system_bus)
    }

    /// The Noctalia socket once it exists, resolved like the other endpoints. `None` while
    /// it doesn't exist.
    pub(crate) fn noctalia_socket(&self, run: &Path) -> Result<Option<PathBuf>> {
        let path = &self.noctalia;
        let exists = path
            .try_exists()
            .context(format!("check {}", path.display()))?;
        if !exists {
            return Ok(None);
        }
        resolve_under(path, run)?;
        Ok(Some(path.clone()))
    }
}

/// Requires that nothing, not even a dangling symlink, exists at `path`, so no bus can
/// listen there.
pub(crate) fn nothing_at(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Ok(_) => Err(Failure::new(format!(
            "{} exists, but nothing may be there",
            path.display()
        ))),
        Err(error) => Err(Failure::new(format!("check {}: {error}", path.display()))),
    }
}

pub(crate) fn resolve_under(path: &Path, run: &Path) -> Result<()> {
    let resolved = fs::canonicalize(path).context(format!("resolve {}", path.display()))?;
    if is_under(&resolved, run) {
        Ok(())
    } else {
        Err(Failure::new(format!(
            "{} resolves to {}, outside {}",
            path.display(),
            resolved.display(),
            run.display()
        )))
    }
}

/// `noctalia-$WAYLAND_DISPLAY.sock`, as Noctalia 5.2.1 builds it (`resolveSocketPath` in
/// `src/ipc/ipc_service.cpp`).
fn noctalia_socket_name(display: &OsStr) -> OsString {
    let mut name = OsString::from("noctalia-");
    name.push(display);
    name.push(".sock");
    name
}

/// The socket path of a single `unix:path=…` D-Bus address.
pub(crate) fn dbus_socket(address: &str) -> Result<PathBuf> {
    let path = address
        .strip_prefix("unix:")
        .filter(|rest| !rest.contains([';', '%']))
        .and_then(|rest| rest.split(',').find_map(|pair| pair.strip_prefix("path=")));
    path.map(PathBuf::from)
        .ok_or_else(|| Failure::new(format!("unsupported D-Bus address {address:?}")))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

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
            (
                "DBUS_SYSTEM_BUS_ADDRESS",
                OsString::from(format!("unix:path={}/no-system-bus", run.display())),
            ),
        ])
    }

    fn check_with(test_dir: &TestDir, env: &HashMap<&'static str, OsString>) -> Result<Nested> {
        Nested::check(test_dir, |name| env.get(name).cloned())
    }

    #[test]
    fn accepts_endpoints_under_test_dir_run() {
        let test_dir = TestDir::open(PathBuf::from("/r/t")).unwrap();
        let nested = check_with(&test_dir, &nested_env(&test_dir)).unwrap();
        assert_eq!(
            nested.noctalia,
            Path::new("/r/t/run/noctalia-wayland-1.sock")
        );
    }

    #[test]
    fn refuses_any_host_endpoint() {
        let test_dir = TestDir::open(PathBuf::from("/r/t")).unwrap();
        let host = [
            ("XDG_RUNTIME_DIR", "/run/user/1000"),
            ("NIRI_SOCKET", "/run/user/1000/niri.wayland-1.1725.sock"),
            ("WAYLAND_DISPLAY", "/run/user/1000/wayland-1"),
            ("DBUS_SESSION_BUS_ADDRESS", "unix:path=/run/user/1000/bus"),
            (
                "DBUS_SYSTEM_BUS_ADDRESS",
                "unix:path=/run/dbus/system_bus_socket",
            ),
        ];
        for (name, value) in host {
            let mut env = nested_env(&test_dir);
            env.insert(name, OsString::from(value));
            assert!(check_with(&test_dir, &env).is_err(), "{name}");
        }
        for name in ["NIRI_SOCKET", "DBUS_SYSTEM_BUS_ADDRESS"] {
            let mut missing = nested_env(&test_dir);
            missing.remove(name);
            assert!(check_with(&test_dir, &missing).is_err(), "{name}");
        }
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
            noctalia: run.join("noctalia-wayland-1.sock"),
            system_bus: run.join("no-system-bus"),
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
    fn the_system_bus_must_be_the_generated_path_with_nothing_there() {
        let lexical = TestDir::open(PathBuf::from("/r/t")).unwrap();
        let mut elsewhere = nested_env(&lexical);
        elsewhere.insert(
            "DBUS_SYSTEM_BUS_ADDRESS",
            OsString::from("unix:path=/r/t/run/other"),
        );
        assert!(check_with(&lexical, &elsewhere).is_err());

        let base = fs::canonicalize(env::temp_dir())
            .unwrap()
            .join(format!("harness-system-bus-{}", std::process::id()));
        let test_dir = TestDir::create(&base, "t").unwrap();
        let run = test_dir.run();
        for name in ["niri.sock", "wayland-1", "dbus-x"] {
            fs::write(run.join(name), "").unwrap();
        }
        let nested = Nested {
            niri: run.join("niri.sock"),
            wayland: run.join("wayland-1"),
            dbus: run.join("dbus-x"),
            noctalia: run.join("noctalia-wayland-1.sock"),
            system_bus: test_dir.system_bus(),
        };
        nested.resolve(&run).unwrap();
        fs::write(&nested.system_bus, "").unwrap();
        assert!(nested.resolve(&run).is_err());
        fs::remove_file(&nested.system_bus).unwrap();
        std::os::unix::fs::symlink("/run/dbus/system_bus_socket", &nested.system_bus).unwrap();
        assert!(nested.resolve(&run).is_err());
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn the_noctalia_socket_is_absent_until_created_and_must_resolve_under_run() {
        let base = fs::canonicalize(env::temp_dir())
            .unwrap()
            .join(format!("harness-noctalia-{}", std::process::id()));
        let test_dir = TestDir::create(&base, "t").unwrap();
        let run = test_dir.run();
        let mut nested = Nested {
            niri: run.join("niri.sock"),
            wayland: run.join("wayland-1"),
            dbus: run.join("dbus-x"),
            noctalia: run.join("noctalia-wayland-1.sock"),
            system_bus: run.join("no-system-bus"),
        };
        assert_eq!(nested.noctalia_socket(&run).unwrap(), None);
        fs::write(&nested.noctalia, "").unwrap();
        assert_eq!(
            nested.noctalia_socket(&run).unwrap(),
            Some(run.join("noctalia-wayland-1.sock"))
        );

        fs::write(base.join("host.sock"), "").unwrap();
        nested.noctalia = run.join("noctalia-alias.sock");
        std::os::unix::fs::symlink(base.join("host.sock"), &nested.noctalia).unwrap();
        assert!(nested.noctalia_socket(&run).is_err());
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
