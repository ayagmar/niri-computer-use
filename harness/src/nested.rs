//! The nested endpoints: the niri, Wayland and D-Bus sockets the supervisor inherits. Each
//! must resolve to a path under `TEST_DIR/run` before anything is sent to it.

use std::env;
use std::ffi::OsString;
use std::fs;
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
