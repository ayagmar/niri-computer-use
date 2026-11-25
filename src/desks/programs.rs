//! The programs `desk start` needs, found before anything starts, so a missing one is
//! named at once rather than as a failed step. They are runtime dependencies of desks
//! alone: cage for a headless desk, dbus-daemon, niri, and at-spi2-core's bus launcher and
//! registry daemon, which distributions install outside `PATH`.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use super::record::Mode;

/// Where distributions put at-spi2-core's daemons: Arch in `/usr/lib`, Debian and Fedora
/// in `/usr/libexec`, some in a directory of the package's own.
pub(crate) const ACCESSIBILITY_DIRS: [&str; 3] =
    ["/usr/lib", "/usr/libexec", "/usr/lib/at-spi2-core"];
const LAUNCHER: &str = "at-spi-bus-launcher";
const REGISTRY: &str = "at-spi2-registryd";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Programs {
    /// `None` for a windowed desk, which needs no cage.
    pub(crate) cage: Option<PathBuf>,
    pub(crate) dbus_daemon: PathBuf,
    pub(crate) niri: PathBuf,
    pub(crate) launcher: PathBuf,
    pub(crate) registry: PathBuf,
}

impl Programs {
    /// Finds every program a desk in `mode` needs: on `path`, and the accessibility daemons
    /// in `accessibility_dirs`. Names every one missing.
    pub(crate) fn find(
        path: &OsStr,
        accessibility_dirs: &[&Path],
        mode: Mode,
    ) -> Result<Self, String> {
        let on_path = |name: &str| {
            std::env::split_paths(path)
                .filter(|dir| dir.is_absolute())
                .map(|dir| dir.join(name))
                .find(|file| executable(file))
        };
        let in_dirs = |name: &str| {
            accessibility_dirs
                .iter()
                .map(|dir| dir.join(name))
                .find(|file| executable(file))
        };
        let dirs = accessibility_dirs
            .iter()
            .map(|dir| dir.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        let mut missing = Vec::new();
        let mut need = |found: Option<PathBuf>, what: String| {
            if found.is_none() {
                missing.push(what);
            }
            found
        };
        let cage = (mode == Mode::Headless).then(|| {
            need(
                on_path("cage"),
                "cage (on PATH; a headless desk runs inside it)".to_owned(),
            )
        });
        let dbus_daemon = need(on_path("dbus-daemon"), "dbus-daemon (on PATH)".to_owned());
        let niri = need(on_path("niri"), "niri (on PATH)".to_owned());
        let launcher = need(
            in_dirs(LAUNCHER),
            format!("{LAUNCHER} (in {dirs}; from at-spi2-core)"),
        );
        let registry = need(
            in_dirs(REGISTRY),
            format!("{REGISTRY} (in {dirs}; from at-spi2-core)"),
        );
        match (dbus_daemon, niri, launcher, registry) {
            (Some(dbus_daemon), Some(niri), Some(launcher), Some(registry))
                if missing.is_empty() =>
            {
                Ok(Self {
                    cage: cage.flatten(),
                    dbus_daemon,
                    niri,
                    launcher,
                    registry,
                })
            }
            _ => Err(format!(
                "a desk needs programs this system lacks: {}",
                missing.join("; ")
            )),
        }
    }
}

fn executable(file: &Path) -> bool {
    file.is_file() && rustix::fs::access(file, rustix::fs::Access::EXEC_OK).is_ok()
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;

    use super::*;

    fn program(dir: &Path, name: &str) {
        std::fs::create_dir_all(dir).unwrap();
        let file = dir.join(name);
        std::fs::write(&file, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o700)).unwrap();
    }

    #[test]
    fn the_accessibility_daemons_are_found_where_any_distribution_puts_them() {
        let root = crate::test_support::fresh_dir("desk-programs");
        let (bin, lib, libexec) = (root.join("bin"), root.join("lib"), root.join("libexec"));
        for name in ["cage", "dbus-daemon", "niri"] {
            program(&bin, name);
        }
        program(&libexec, LAUNCHER);
        program(&lib, REGISTRY);
        let dirs = [lib.as_path(), libexec.as_path()];
        let found = Programs::find(bin.as_os_str(), &dirs, Mode::Headless).unwrap();
        assert_eq!(found.launcher, libexec.join(LAUNCHER));
        assert_eq!(found.registry, lib.join(REGISTRY));
        assert_eq!(found.cage, Some(bin.join("cage")));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn every_missing_program_is_named_and_cage_only_for_a_headless_desk() {
        let root = crate::test_support::fresh_dir("desk-programs-missing");
        let bin = root.join("bin");
        program(&bin, "niri");
        let dirs = [root.join("lib")];
        let dirs: Vec<&Path> = dirs.iter().map(PathBuf::as_path).collect();
        let error = Programs::find(bin.as_os_str(), &dirs, Mode::Headless).unwrap_err();
        for name in ["cage", "dbus-daemon", LAUNCHER, REGISTRY] {
            assert!(error.contains(name), "{name}: {error}");
        }
        assert!(!error.contains("niri (on PATH)"), "{error}");
        let windowed = Programs::find(bin.as_os_str(), &dirs, Mode::Windowed).unwrap_err();
        assert!(!windowed.contains("cage"), "{windowed}");
        std::fs::remove_dir_all(root).unwrap();
    }
}
