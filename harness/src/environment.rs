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
/// The distribution's Python, which has the `PyGObject` the fixtures import. A `python3`
/// found on `PATH` may be a version manager's shim, which can refuse to run or lack `gi`.
pub(crate) const PYTHON: &str = "/usr/bin/python3";

const KEPT: [&str; 4] = ["HOME", "PATH", "LANG", SHARED];

/// Set to `0` without `--shared`, so that every server runs standalone; `--shared` leaves
/// it unset, so that servers run in shared mode by default.
const SHARED: &str = "NIRI_COMPUTER_USE_SHARED";

/// The one path that may point outside `TEST_DIR`: the host Wayland socket nested niri
/// draws its window on.
const HOST_SOCKET: &str = "WAYLAND_DISPLAY";

/// Must be exactly `TestDir::system_bus_address`, with nothing at its path, so nested
/// clients can't reach the host's system bus.
const SYSTEM_BUS: &str = "DBUS_SYSTEM_BUS_ADDRESS";

/// For `--multi`: the outputs a niri built with `harness/niri-test/headless-outputs.patch`
/// gives itself, `WxH` sizes separated by commas.
pub(crate) const HEADLESS_OUTPUTS: &str = "NIRI_TEST_HEADLESS_OUTPUTS";

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
/// Without `shared`, `NIRI_COMPUTER_USE_SHARED=0` turns shared mode off.
pub(crate) fn parent(test_dir: &TestDir, host: &Host, display: &Path, shared: bool) -> Env {
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
    if !shared {
        env.insert(SHARED, "0".into());
    }
    env
}

/// The host's `PATH` with `dir` first, for a nested session whose niri is in `dir`.
pub(crate) fn first_on_path(dir: &Path, host: &Host) -> Result<OsString> {
    let rest = env::split_paths(&host.path);
    env::join_paths(std::iter::once(dir.to_path_buf()).chain(rest))
        .context(format!("put {} first on PATH", dir.display()))
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
    if name == HEADLESS_OUTPUTS {
        return if sizes(value.as_os_str()) {
            Ok(())
        } else {
            Err(Failure::new(format!(
                "{name} must be WxH sizes separated by commas"
            )))
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

/// Whether `value` is `WxH` sizes in pixels, separated by commas.
fn sizes(value: &std::ffi::OsStr) -> bool {
    let number = |text: &str| !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit());
    value.to_str().is_some_and(|value| {
        value.split(',').all(|size| {
            size.split_once('x')
                .is_some_and(|(width, height)| number(width) && number(height))
        })
    })
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
        let env = parent(&test_dir, &host(), Path::new("/fake/wayland-1"), false);
        assert_eq!(env.get(SHARED), Some(&OsString::from("0")));
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
    fn a_chosen_niri_comes_first_on_path() {
        let path = first_on_path(Path::new("/c/bin/abc"), &host()).unwrap();
        assert_eq!(
            env::split_paths(&path).collect::<Vec<_>>(),
            [PathBuf::from("/c/bin/abc"), PathBuf::from("/usr/bin")]
        );
    }

    #[test]
    fn containment_rejects_unknown_and_escaping_variables() {
        let (test_dir, base) = test_dir("containment");
        let mut stray = parent(&test_dir, &host(), Path::new("/fake/wayland-1"), false);
        stray.insert(
            "NIRI_SOCKET",
            test_dir.run().join("niri.sock").into_os_string(),
        );
        assert!(check_containment(&stray, &test_dir).is_err());

        let mut escaping = parent(&test_dir, &host(), Path::new("/fake/wayland-1"), false);
        escaping.insert("XDG_CONFIG_HOME", OsString::from("/home/u/.config"));
        assert!(check_containment(&escaping, &test_dir).is_err());

        let mut system_bus = parent(&test_dir, &host(), Path::new("/fake/wayland-1"), false);
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

        let listening = parent(&test_dir, &host(), Path::new("/fake/wayland-1"), false);
        fs::write(test_dir.system_bus(), "").unwrap();
        assert!(check_containment(&listening, &test_dir).is_err());
        fs::remove_file(test_dir.system_bus()).unwrap();

        let mut relative = parent(&test_dir, &host(), Path::new("/fake/wayland-1"), false);
        relative.insert(HOST_SOCKET, OsString::from("wayland-1"));
        assert!(check_containment(&relative, &test_dir).is_err());

        let mut outputs = parent(&test_dir, &host(), Path::new("/fake/wayland-1"), false);
        outputs.insert(HEADLESS_OUTPUTS, OsString::from("1280x720,1920x1080"));
        assert!(check_containment(&outputs, &test_dir).is_ok());
        for spec in ["", "1280x720,", "1280x", "/home/u/x", "1280x720;1x1"] {
            outputs.insert(HEADLESS_OUTPUTS, OsString::from(spec));
            assert!(check_containment(&outputs, &test_dir).is_err(), "{spec}");
        }
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
