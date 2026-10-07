use std::ffi::OsString;
use std::fs::DirBuilder;
use std::os::unix::fs::DirBuilderExt as _;
use std::path::{Component, Path, PathBuf};

use crate::failure::{Context as _, Failure, Result};

/// `TEST_DIR`: everything a nested run creates lives under this directory.
#[derive(Debug, Clone)]
pub(crate) struct TestDir {
    root: PathBuf,
}

impl TestDir {
    /// Creates a fresh `<base>/<name>` with its subdirectories, all mode 0700.
    pub(crate) fn create(base: &Path, name: &str) -> Result<Self> {
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(base)
            .context(format!("create {}", base.display()))?;
        let test_dir = Self::open(base.join(name))?;
        create_private(&test_dir.root)?;
        for dir in [
            test_dir.run(),
            test_dir.state(),
            test_dir.cache(),
            test_dir.config(),
            test_dir.data(),
        ] {
            create_private(&dir)?;
        }
        Ok(test_dir)
    }

    /// Uses an existing `TEST_DIR`. The path is restricted to characters that need no
    /// quoting in the generated niri and D-Bus configs.
    pub(crate) fn open(root: PathBuf) -> Result<Self> {
        let plain = root
            .to_str()
            .is_some_and(|text| text.bytes().all(is_plain_path_byte));
        let normal = root
            .components()
            .all(|part| matches!(part, Component::RootDir | Component::Normal(_)));
        if !(root.is_absolute() && plain && normal) {
            return Err(Failure::new(format!(
                "TEST_DIR {} must be absolute, without `.` or `..`, and use only [A-Za-z0-9/._-]",
                root.display()
            )));
        }
        let test_dir = Self { root };
        let run_len = test_dir.run().as_os_str().len();
        if run_len > MAX_RUN_DIR_LEN {
            return Err(Failure::new(format!(
                "{} is {run_len} bytes; socket paths in it would pass the 108-byte limit",
                test_dir.run().display()
            )));
        }
        Ok(test_dir)
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn run(&self) -> PathBuf {
        self.root.join("run")
    }

    pub(crate) fn state(&self) -> PathBuf {
        self.root.join("state")
    }

    pub(crate) fn cache(&self) -> PathBuf {
        self.root.join("cache")
    }

    pub(crate) fn config(&self) -> PathBuf {
        self.root.join("config")
    }

    pub(crate) fn data(&self) -> PathBuf {
        self.root.join("data")
    }

    /// `DBUS_SYSTEM_BUS_ADDRESS` in PARENT. Nothing may exist at its path, so a nested
    /// client that looks for the system bus fails instead of reaching the host's.
    pub(crate) fn system_bus_address(&self) -> OsString {
        let mut address = OsString::from("unix:path=");
        address.push(self.system_bus());
        address
    }

    pub(crate) fn system_bus(&self) -> PathBuf {
        self.run().join("no-system-bus")
    }

    pub(crate) fn niri_config(&self) -> PathBuf {
        self.root.join("niri.kdl")
    }

    pub(crate) fn dbus_config(&self) -> PathBuf {
        self.root.join("dbus.conf")
    }

    /// Created by the test bind, so a test can tell whether a niri bind fired.
    pub(crate) fn bind_marker(&self) -> PathBuf {
        self.root.join("bind-fired")
    }
}

/// Unix socket paths are limited to 108 bytes including the NUL. niri's IPC socket,
/// `niri.wayland-N.<pid>.sock`, is up to 28 bytes, plus the `/` before it.
const MAX_RUN_DIR_LEN: usize = 108 - 1 - 1 - 28;

const fn is_plain_path_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'.' | b'_' | b'-')
}

fn create_private(dir: &Path) -> Result<()> {
    DirBuilder::new()
        .mode(0o700)
        .create(dir)
        .context(format!("create {}", dir.display()))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt as _;

    use super::*;

    #[test]
    fn create_makes_private_subdirectories() {
        let base = std::env::temp_dir().join(format!("harness-test-dir-{}", std::process::id()));
        let test_dir = TestDir::create(&base, "run1").unwrap();
        for dir in [
            test_dir.root().to_path_buf(),
            test_dir.run(),
            test_dir.data(),
        ] {
            let mode = fs::metadata(&dir).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o700, "{}", dir.display());
        }
        assert!(TestDir::create(&base, "run1").is_err(), "must be fresh");
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn open_rejects_paths_that_would_need_quoting() {
        for root in [
            "relative/dir",
            "/tmp/a b",
            "/tmp/a\"b",
            "/tmp/a<b",
            "/tmp/../run",
        ] {
            assert!(TestDir::open(PathBuf::from(root)).is_err(), "{root}");
        }
        assert!(TestDir::open(PathBuf::from("/run/user/1000/x-1/2")).is_ok());
        let long = format!("/{}", "a".repeat(80));
        assert!(TestDir::open(PathBuf::from(long)).is_err());
    }
}
