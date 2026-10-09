//! PARENT: the environment for `dbus-run-session` and the nested niri, built from scratch.

use std::collections::BTreeMap;
use std::env;
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::FileTypeExt as _;
use std::path::{Component, Path, PathBuf};

use crate::failure::{Context as _, Failure, Result};
use crate::nested::nothing_at;
use crate::test_dir::TestDir;

pub(crate) type Env = BTreeMap<&'static str, OsString>;

/// Kept from the user's session as they are.
const KEPT: [&str; 3] = ["HOME", "PATH", "LANG"];

/// The one path that may point outside `TEST_DIR`: the host Wayland socket nested niri
/// draws its window on.
const HOST_SOCKET: &str = "WAYLAND_DISPLAY";

/// Must be exactly `TestDir::system_bus_address`, with nothing at its path, so nested
/// clients can't reach the host's system bus.
const SYSTEM_BUS: &str = "DBUS_SYSTEM_BUS_ADDRESS";

/// Every other path variable, all of which must resolve under `TEST_DIR`.
const CONTAINED: [&str; 8] = [
    "XDG_RUNTIME_DIR",
    "XDG_STATE_HOME",
    "XDG_CACHE_HOME",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "NOCTALIA_CONFIG_HOME",
    "NOCTALIA_STATE_HOME",
    "NOCTALIA_DATA_HOME",
];

/// What the harness reads from the user's session.
#[derive(Debug)]
pub(crate) struct Host {
    pub(crate) runtime_dir: PathBuf,
    home: OsString,
    path: OsString,
    lang: Option<OsString>,
}

impl Host {
    pub(crate) fn from_env() -> Result<Self> {
        let runtime_dir = PathBuf::from(required("XDG_RUNTIME_DIR")?);
        Ok(Self {
            runtime_dir,
            home: required("HOME")?,
            path: required("PATH")?,
            lang: env::var_os("LANG"),
        })
    }

    pub(crate) fn visible_socket(&self) -> Result<PathBuf> {
        let socket = self.runtime_dir.join(required("WAYLAND_DISPLAY")?);
        if !fs::metadata(&socket)
            .context(format!("host Wayland socket {}", socket.display()))?
            .file_type()
            .is_socket()
        {
            return Err(Failure::new("host Wayland display is not a socket"));
        }
        Ok(socket)
    }

    pub(crate) fn home(&self) -> &Path {
        Path::new(&self.home)
    }
}

fn required(name: &str) -> Result<OsString> {
    env::var_os(name).ok_or_else(|| Failure::new(format!("{name} is not set")))
}

/// Builds PARENT. `NIRI_SOCKET`, `WAYLAND_SOCKET`, `DISPLAY`, `XDG_SESSION_ID` and
/// `DBUS_SESSION_BUS_ADDRESS` are left out, as is everything else not listed here.
pub(crate) fn parent(test_dir: &TestDir, host: &Host, display: &Path) -> Env {
    let mut env = Env::from([
        ("XDG_RUNTIME_DIR", test_dir.run().into_os_string()),
        ("XDG_STATE_HOME", test_dir.state().into_os_string()),
        ("XDG_CACHE_HOME", test_dir.cache().into_os_string()),
        ("XDG_CONFIG_HOME", test_dir.config().into_os_string()),
        ("XDG_DATA_HOME", test_dir.data().into_os_string()),
        (
            "NOCTALIA_CONFIG_HOME",
            test_dir.noctalia_config_home().into_os_string(),
        ),
        (
            "NOCTALIA_STATE_HOME",
            test_dir.state().join("noctalia").into_os_string(),
        ),
        (
            "NOCTALIA_DATA_HOME",
            test_dir.data().join("noctalia").into_os_string(),
        ),
        (HOST_SOCKET, display.as_os_str().to_owned()),
        (SYSTEM_BUS, test_dir.system_bus_address()),
        ("HOME", host.home.clone()),
        ("PATH", host.path.clone()),
    ]);
    if let Some(lang) = &host.lang {
        env.insert("LANG", lang.clone());
    }
    env
}

/// Checks PARENT before anything is started: only known variables, every path variable
/// and the system bus address under `TEST_DIR`, and an absolute host Wayland socket as the
/// one exemption.
pub(crate) fn check_containment(env: &Env, test_dir: &TestDir) -> Result<()> {
    let root = test_dir.root();
    let canonical = fs::canonicalize(root).context(format!("resolve {}", root.display()))?;
    if canonical != root {
        return Err(Failure::new(format!(
            "TEST_DIR {} resolves to {}",
            root.display(),
            canonical.display()
        )));
    }
    for (&name, value) in env {
        check_variable(name, Path::new(value), test_dir)?;
    }
    Ok(())
}

fn check_variable(name: &str, value: &Path, test_dir: &TestDir) -> Result<()> {
    let root = test_dir.root();
    if KEPT.contains(&name) {
        return Ok(());
    }
    if name == HOST_SOCKET {
        return if value.is_absolute() {
            Ok(())
        } else {
            Err(Failure::new(format!("{name} must be an absolute path")))
        };
    }
    if name == SYSTEM_BUS {
        let expected = test_dir.system_bus_address();
        return if value.as_os_str() == expected {
            nothing_at(&test_dir.system_bus())
        } else {
            Err(Failure::new(format!(
                "{name} must be {}",
                expected.display()
            )))
        };
    }
    if !CONTAINED.contains(&name) {
        return Err(Failure::new(format!(
            "unexpected variable {name} in PARENT"
        )));
    }
    if is_under(value, root) {
        Ok(())
    } else {
        Err(Failure::new(format!(
            "{name}={} is outside TEST_DIR {}",
            value.display(),
            root.display()
        )))
    }
}

/// Lexical containment. Sound because `TEST_DIR` is fresh, private and has no symlinks in
/// its own path, so nothing below it can redirect out of it.
pub(crate) fn is_under(path: &Path, root: &Path) -> bool {
    path.starts_with(root)
        && path
            .components()
            .all(|part| matches!(part, Component::RootDir | Component::Normal(_)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host() -> Host {
        Host {
            runtime_dir: PathBuf::from("/run/user/1000"),
            home: OsString::from("/home/u"),
            path: OsString::from("/usr/bin"),
            lang: Some(OsString::from("C.UTF-8")),
        }
    }

    fn test_dir(name: &str) -> (TestDir, PathBuf) {
        let base = fs::canonicalize(env::temp_dir())
            .unwrap()
            .join(format!("harness-env-{name}-{}", std::process::id()));
        (TestDir::create(&base, "t").unwrap(), base)
    }

    #[test]
    fn parent_leaves_out_host_endpoints() {
        let (test_dir, base) = test_dir("parent");
        let env = parent(&test_dir, &host(), Path::new("/fake/wayland-1"));
        for name in [
            "NIRI_SOCKET",
            "WAYLAND_SOCKET",
            "DISPLAY",
            "XDG_SESSION_ID",
            "DBUS_SESSION_BUS_ADDRESS",
        ] {
            assert!(!env.contains_key(name), "{name}");
        }
        check_containment(&env, &test_dir).unwrap();
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn containment_rejects_unknown_and_escaping_variables() {
        let (test_dir, base) = test_dir("containment");
        let mut stray = parent(&test_dir, &host(), Path::new("/fake/wayland-1"));
        stray.insert(
            "NIRI_SOCKET",
            test_dir.run().join("niri.sock").into_os_string(),
        );
        assert!(check_containment(&stray, &test_dir).is_err());

        let mut escaping = parent(&test_dir, &host(), Path::new("/fake/wayland-1"));
        escaping.insert("XDG_CONFIG_HOME", OsString::from("/home/u/.config"));
        assert!(check_containment(&escaping, &test_dir).is_err());

        let mut system_bus = parent(&test_dir, &host(), Path::new("/fake/wayland-1"));
        system_bus.insert(
            SYSTEM_BUS,
            OsString::from("unix:path=/run/dbus/system_bus_socket"),
        );
        assert!(check_containment(&system_bus, &test_dir).is_err());
        system_bus.insert(
            SYSTEM_BUS,
            OsString::from(format!("unix:path={}/other", test_dir.run().display())),
        );
        assert!(check_containment(&system_bus, &test_dir).is_err());

        let listening = parent(&test_dir, &host(), Path::new("/fake/wayland-1"));
        fs::write(test_dir.system_bus(), "").unwrap();
        assert!(check_containment(&listening, &test_dir).is_err());
        fs::remove_file(test_dir.system_bus()).unwrap();

        let mut relative = parent(&test_dir, &host(), Path::new("/fake/wayland-1"));
        relative.insert(HOST_SOCKET, OsString::from("wayland-1"));
        assert!(check_containment(&relative, &test_dir).is_err());
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn is_under_rejects_parent_components() {
        let root = Path::new("/r/t");
        assert!(is_under(Path::new("/r/t/run/x"), root));
        assert!(!is_under(Path::new("/r/t/../escape"), root));
        assert!(!is_under(Path::new("/r/other"), root));
    }
}
