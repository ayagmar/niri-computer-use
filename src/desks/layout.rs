//! Where desks live. Every desk of the host session is a directory under one root,
//! `<host runtime dir>/ncu-d/`, which is `/run/user/<uid>/ncu-d/` in a logind session.
//! The root also holds the host-wide stop flag every desk's server checks. A desk's
//! directory is its nested niri's `XDG_RUNTIME_DIR`, so its sockets and the runtime
//! directory its server keeps beside niri's socket are inside it, and it is short enough
//! for the longest of them. Its logs are under the host's state directory instead, so a
//! failed start leaves them behind.

use std::path::{Path, PathBuf};

use super::name::DeskName;

/// The root's name in the host's runtime directory: short, so a desk's sockets fit.
const ROOT: &str = "ncu-d";
/// The host-wide stop flag in the root. The dot keeps it from ever being a desk's name.
pub(crate) const STOP: &str = "stop.flag";
/// The most bytes a Unix socket's path may have, without the terminating NUL.
pub(crate) const SOCKET_PATH_MAX: usize = 107;
/// The longest socket a desk holds, the server's engine socket, after the desk's
/// directory: `/niri-computer-use/niri.<display>.<pid>/engine.sock` with the longest
/// display name niri picks (`wayland-32`, smithay's last) and a 7-digit PID (Linux's
/// `pid_max` is at most 4194304).
const LONGEST_SOCKET: &str = "/niri-computer-use/niri.wayland-32.4194304/engine.sock";

/// The root of every desk of one host session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Desks {
    root: PathBuf,
}

impl Desks {
    /// The desks of the host session whose runtime directory is `host_runtime`.
    pub(crate) fn of(host_runtime: &Path) -> Self {
        Self {
            root: host_runtime.join(ROOT),
        }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.root
    }

    /// The host-wide stop flag: while it exists, no desk's server acts.
    pub(crate) fn stop_flag(&self) -> PathBuf {
        self.root.join(STOP)
    }

    /// Sets the host-wide stop flag, creating the root as needed.
    pub(crate) fn stop(&self) -> std::io::Result<()> {
        use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _};
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&self.root)?;
        std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(self.stop_flag())
            .map(drop)
    }

    /// Clears the host-wide stop flag. Each desk's input-dirty marker stays: it is cleared
    /// by `recover --desk` for that desk alone.
    pub(crate) fn resume(&self) -> Result<(), String> {
        match std::fs::remove_file(self.stop_flag()) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                Err(format!("remove {}: {error}", self.stop_flag().display()))
            }
            _ => Ok(()),
        }
    }

    /// The directory of the desk called `name`, refused when its longest socket wouldn't
    /// fit in a Unix socket's path.
    pub(crate) fn desk(&self, name: &DeskName) -> Result<Layout, String> {
        let root = self.root.join(name.as_str());
        fits(&root)?;
        Ok(Layout { root })
    }

    /// The names of the directories under the root that are desk names, sorted.
    pub(crate) fn names(&self) -> std::io::Result<Vec<DeskName>> {
        let entries = match std::fs::read_dir(&self.root) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            entries => entries?,
        };
        let mut names = Vec::new();
        for entry in entries {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            if let Some(name) = entry
                .file_name()
                .to_str()
                .and_then(|name| DeskName::parse(name).ok())
            {
                names.push(name);
            }
        }
        names.sort();
        Ok(names)
    }
}

/// Whether the longest socket a desk at `root` would hold fits in a Unix socket's path.
pub(crate) fn fits(root: &Path) -> Result<(), String> {
    let longest = root.as_os_str().len() + LONGEST_SOCKET.len();
    if longest > SOCKET_PATH_MAX {
        return Err(format!(
            "the desk's directory {} is too long: its server's socket could need {longest} bytes, over the {SOCKET_PATH_MAX} a Unix socket's path may have",
            root.display()
        ));
    }
    Ok(())
}

/// The files of one desk, all in its directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Layout {
    root: PathBuf,
}

impl Layout {
    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    /// cage's `XDG_RUNTIME_DIR`, apart from the desk's, so the nested niri doesn't take
    /// cage's display for its own.
    pub(crate) fn cage_dir(&self) -> PathBuf {
        self.root.join("cage")
    }

    /// The display cage makes in its directory.
    pub(crate) fn cage_display(&self) -> PathBuf {
        self.cage_dir().join("wayland-0")
    }

    pub(crate) fn niri_config(&self) -> PathBuf {
        self.root.join("niri.kdl")
    }

    pub(crate) fn bus_config(&self) -> PathBuf {
        self.root.join("dbus.conf")
    }

    /// The desk's record: its state and its endpoints.
    pub(crate) fn record(&self) -> PathBuf {
        self.root.join("desk.json")
    }

    /// What the desk's server takes from the host session.
    pub(crate) fn host(&self) -> PathBuf {
        self.root.join("host.json")
    }

    /// The supervisor's socket, which `desk stop` asks to stop the desk.
    pub(crate) fn control(&self) -> PathBuf {
        self.root.join("control.sock")
    }

    /// The apps' `TMPDIR`, where Chromium keeps its singleton socket.
    pub(crate) fn tmp(&self) -> PathBuf {
        self.root.join("tmp")
    }

    /// `DBUS_SYSTEM_BUS_ADDRESS` for the apps: nothing exists there, so they have no
    /// system bus.
    pub(crate) fn no_system_bus(&self) -> PathBuf {
        self.root.join("no-system-bus")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(text: &str) -> DeskName {
        DeskName::parse(text).unwrap()
    }

    #[test]
    fn every_name_fits_under_a_logind_runtime_directory_of_any_uid() {
        let desks = Desks::of(Path::new("/run/user/4294967295"));
        let longest = "a".repeat(super::super::name::MAX);
        let layout = desks.desk(&name(&longest)).unwrap();
        let engine = layout
            .root()
            .join("niri-computer-use/niri.wayland-32.4194304/engine.sock");
        assert!(engine.as_os_str().len() <= SOCKET_PATH_MAX);
        assert_eq!(
            Desks::of(Path::new("/run/user/1000"))
                .desk(&name("web"))
                .unwrap()
                .root(),
            Path::new("/run/user/1000/ncu-d/web")
        );
    }

    #[test]
    fn a_runtime_directory_too_long_for_its_sockets_is_refused() {
        let deep = Path::new("/tmp").join("d".repeat(60));
        let error = Desks::of(&deep).desk(&name("web")).unwrap_err();
        assert!(error.contains("too long"), "{error}");
    }

    #[test]
    fn the_stop_flag_is_beside_the_desks_not_in_one() {
        let desks = Desks::of(Path::new("/run/user/1000"));
        assert_eq!(
            desks.stop_flag(),
            Path::new("/run/user/1000/ncu-d/stop.flag")
        );
        assert!(DeskName::parse(STOP).is_err());
        // `names` lists directories alone, and only those named as desks are.
        let dir = crate::test_support::fresh_dir("desks-names");
        let in_dir = Desks::of(&dir);
        assert_eq!(in_dir.names().unwrap(), Vec::<DeskName>::new());
        std::fs::create_dir_all(in_dir.path().join("web")).unwrap();
        std::fs::create_dir_all(in_dir.path().join("Not-A-Name")).unwrap();
        std::fs::write(in_dir.stop_flag(), "").unwrap();
        std::fs::write(in_dir.path().join("stop"), "").unwrap();
        assert_eq!(in_dir.names().unwrap(), vec![name("web")]);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
