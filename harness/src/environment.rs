//! PARENT: the environment for `dbus-run-session` and the nested niri, built from scratch.

use std::collections::BTreeMap;
use std::env;
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::FileTypeExt as _;
use std::path::{Component, Path, PathBuf};

use crate::failure::{Context as _, Failure, Result};
use crate::test_dir::TestDir;

pub(crate) type Env = BTreeMap<&'static str, OsString>;

/// Kept from the user's session as they are.
const KEPT: [&str; 3] = ["HOME", "PATH", "LANG"];

/// The one path that may point outside `TEST_DIR`: the host Wayland socket nested niri
/// draws its window on.
const HOST_SOCKET: &str = "WAYLAND_DISPLAY";

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
    pub(crate) wayland_socket: PathBuf,
    home: OsString,
    path: OsString,
    lang: Option<OsString>,
}

impl Host {
    pub(crate) fn from_env() -> Result<Self> {
        let runtime_dir = PathBuf::from(required("XDG_RUNTIME_DIR")?);
        let wayland_socket = runtime_dir.join(required("WAYLAND_DISPLAY")?);
        let file_type = fs::metadata(&wayland_socket)
            .context(format!("host Wayland socket {}", wayland_socket.display()))?
            .file_type();
        if !file_type.is_socket() {
            return Err(Failure::new(format!(
                "host Wayland socket {} is not a socket",
                wayland_socket.display()
            )));
        }
        Ok(Self {
            runtime_dir,
            wayland_socket,
            home: required("HOME")?,
            path: required("PATH")?,
            lang: env::var_os("LANG"),
        })
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
pub(crate) fn parent(test_dir: &TestDir, host: &Host) -> Env {
    let mut env = Env::from([
        ("XDG_RUNTIME_DIR", test_dir.run().into_os_string()),
        ("XDG_STATE_HOME", test_dir.state().into_os_string()),
        ("XDG_CACHE_HOME", test_dir.cache().into_os_string()),
        ("XDG_CONFIG_HOME", test_dir.config().into_os_string()),
        ("XDG_DATA_HOME", test_dir.data().into_os_string()),
        (
            "NOCTALIA_CONFIG_HOME",
            test_dir.config().join("noctalia").into_os_string(),
        ),
        (
            "NOCTALIA_STATE_HOME",
            test_dir.state().join("noctalia").into_os_string(),
        ),
        (
            "NOCTALIA_DATA_HOME",
            test_dir.data().join("noctalia").into_os_string(),
        ),
        (HOST_SOCKET, host.wayland_socket.clone().into_os_string()),
        ("HOME", host.home.clone()),
        ("PATH", host.path.clone()),
    ]);
    if let Some(lang) = &host.lang {
        env.insert("LANG", lang.clone());
    }
    env
}

/// Checks PARENT before anything is started: only known variables, every path variable
/// under `TEST_DIR`, and an absolute host Wayland socket as the one exemption.
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
        check_variable(name, Path::new(value), root)?;
    }
    Ok(())
}

fn check_variable(name: &str, value: &Path, root: &Path) -> Result<()> {
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
            wayland_socket: PathBuf::from("/run/user/1000/wayland-1"),
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
        let env = parent(&test_dir, &host());
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
        let mut stray = parent(&test_dir, &host());
        stray.insert(
            "NIRI_SOCKET",
            test_dir.run().join("niri.sock").into_os_string(),
        );
        assert!(check_containment(&stray, &test_dir).is_err());

        let mut escaping = parent(&test_dir, &host());
        escaping.insert("XDG_CONFIG_HOME", OsString::from("/home/u/.config"));
        assert!(check_containment(&escaping, &test_dir).is_err());

        let mut relative = parent(&test_dir, &host());
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
