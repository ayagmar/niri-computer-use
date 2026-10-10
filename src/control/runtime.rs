//! The runtime directory of one compositor instance,
//! `$XDG_RUNTIME_DIR/niri-computer-use/<instance>/`, where `<instance>` is the basename of
//! `NIRI_SOCKET` without `.sock`. It holds the flags every server for that instance
//! shares, so a server for another niri, such as the nested harness, never sees them.

use std::fs::{DirBuilder, OpenOptions};
use std::io;
use std::os::unix::fs::{DirBuilderExt as _, MetadataExt as _, OpenOptionsExt as _};
use std::path::{Path, PathBuf};

use crate::Env;

const STOP: &str = "stop";
pub(crate) const INPUT_DIRTY: &str = "input-dirty";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RuntimeDir {
    path: PathBuf,
}

impl RuntimeDir {
    /// The directory for the niri instance `env` names. Nothing is created.
    pub(crate) fn of(env: &Env) -> Result<Self, String> {
        let socket = env
            .niri_socket
            .path()
            .map_err(|error| error.detail.clone())?;
        let runtime = env
            .runtime_dir
            .as_deref()
            .ok_or("XDG_RUNTIME_DIR is not set")?;
        let name = socket
            .file_name()
            .ok_or_else(|| format!("NIRI_SOCKET {} has no file name", socket.display()))?
            .to_string_lossy();
        let instance = name.strip_suffix(".sock").unwrap_or(&name);
        if instance.is_empty() {
            return Err(format!(
                "NIRI_SOCKET {} has no instance name",
                socket.display()
            ));
        }
        Ok(Self {
            path: runtime.join("niri-computer-use").join(instance),
        })
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// Creates the directory, and its parent, with mode `0700` as needed.
    pub(crate) fn create(&self) -> io::Result<()> {
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&self.path)
    }

    /// Whether the stop flag is set. An error means the directory can't be read, which a
    /// caller that gates on the flag treats as set.
    pub(crate) fn stopped(&self) -> io::Result<bool> {
        self.path.join(STOP).try_exists()
    }

    /// Whether the input-dirty marker says input may be stuck. An error means the
    /// directory can't be read.
    pub(crate) fn input_dirty(&self) -> io::Result<bool> {
        self.path.join(INPUT_DIRTY).try_exists()
    }

    /// Sets the stop flag, creating the directory as needed.
    pub(crate) fn stop(&self) -> io::Result<()> {
        self.create()?;
        OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(self.path.join(STOP))
            .map(drop)
    }

    /// Clears the stop flag, unless the input-dirty marker says input may be stuck.
    pub(crate) fn resume(&self) -> Result<(), String> {
        if self.input_dirty().unwrap_or(true) {
            return Err(
                "input may be stuck (input-dirty is set); run `niri-computer-use recover` first"
                    .to_owned(),
            );
        }
        match std::fs::remove_file(self.path.join(STOP)) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(format!(
                "remove {}: {error}",
                self.path.join(STOP).display()
            )),
            _ => Ok(()),
        }
    }
}

/// The device and inode `path` names now, to tell whether it was replaced since.
pub(crate) fn identity(path: &Path) -> io::Result<(u64, u64)> {
    let meta = std::fs::metadata(path)?;
    Ok((meta.dev(), meta.ino()))
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;

    use super::*;

    fn env(dir: &Path) -> Env {
        Env {
            niri_socket: crate::niri::Socket::at(dir.join("niri.wayland-1.42.sock")),
            runtime_dir: Some(dir.join("run")),
            ..Env::default()
        }
    }

    #[test]
    fn is_named_after_the_niri_instance() {
        let dir = Path::new("/r");
        assert_eq!(
            RuntimeDir::of(&env(dir)).unwrap().path(),
            Path::new("/r/run/niri-computer-use/niri.wayland-1.42")
        );
        let unset = Env {
            runtime_dir: None,
            ..env(dir)
        };
        assert_eq!(
            RuntimeDir::of(&unset),
            Err("XDG_RUNTIME_DIR is not set".to_owned())
        );
        assert_eq!(
            RuntimeDir::of(&Env::default()),
            Err("NIRI_SOCKET is not set".to_owned())
        );
        // Without a name, the flag would land in the directory every instance shares.
        let nameless = Env {
            niri_socket: crate::niri::Socket::at(dir.join(".sock")),
            ..env(dir)
        };
        assert_eq!(
            RuntimeDir::of(&nameless),
            Err("NIRI_SOCKET /r/.sock has no instance name".to_owned())
        );
    }

    #[test]
    fn stop_sets_a_private_flag_and_resume_clears_it() {
        let dir = crate::test_support::fresh_dir("runtime");
        let runtime = RuntimeDir::of(&env(&dir)).unwrap();
        assert!(!runtime.stopped().unwrap());
        runtime.stop().unwrap();
        runtime.stop().unwrap();
        assert!(runtime.stopped().unwrap());
        let mode = |path: &Path| path.metadata().unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(runtime.path()), 0o700);
        assert_eq!(mode(runtime.path().parent().unwrap()), 0o700);
        assert_eq!(mode(&runtime.path().join(STOP)), 0o600);
        runtime.resume().unwrap();
        assert!(!runtime.stopped().unwrap());
        // Resuming twice, or before any stop, is fine.
        runtime.resume().unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn resume_refuses_while_input_may_be_stuck() {
        let dir = crate::test_support::fresh_dir("runtime-dirty");
        let runtime = RuntimeDir::of(&env(&dir)).unwrap();
        runtime.stop().unwrap();
        std::fs::write(runtime.path().join(INPUT_DIRTY), "").unwrap();
        let error = runtime.resume().unwrap_err();
        assert!(error.contains("recover"), "{error}");
        assert!(runtime.stopped().unwrap());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
