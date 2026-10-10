//! The runtime directory of one compositor instance, `<dir>/niri-computer-use/<instance>/`,
//! where `<dir>` is the directory that holds niri's socket and `<instance>` the socket's
//! name without `.sock`, both with every symlink resolved. It holds the lease, the flags
//! and the engine's socket every server for that instance shares, so a server for another
//! niri, such as the nested harness, never sees them, and every server for this one does,
//! whichever `XDG_RUNTIME_DIR` it inherited and however its `NIRI_SOCKET` spells the path.
//! A process resolves the instance once, at its start, and keeps it: a symlink retargeted
//! later, or a niri restarted under another name, doesn't move it.

use std::fs::{DirBuilder, OpenOptions};
use std::io;
use std::os::unix::fs::{
    DirBuilderExt as _, FileTypeExt as _, MetadataExt as _, OpenOptionsExt as _,
};
use std::path::{Path, PathBuf};

use crate::Env;

const STOP: &str = "stop";
pub(crate) const INPUT_DIRTY: &str = "input-dirty";

/// niri's socket with every symlink resolved, in a directory of the user's with mode
/// `0700`: the compositor instance a process coordinates for, or why it has none. Two
/// spellings of one socket's path resolve to the same instance; two hard links to it
/// don't.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Instance(Result<PathBuf, String>);

impl Default for Instance {
    fn default() -> Self {
        Self(Err("NIRI_SOCKET is not set".to_owned()))
    }
}

impl Instance {
    /// Resolves `socket`, which must be a socket owned by `euid` in a private directory
    /// of `euid`'s once resolved. Nothing is taken in its place when it isn't.
    pub(crate) fn resolve(socket: &Path, euid: u32) -> Self {
        Self(resolved(socket, euid).map_err(|why| {
            format!(
                "NIRI_SOCKET {} {why}, so this process coordinates with no other: no lease, stop flag or marker",
                socket.display()
            )
        }))
    }

    /// No instance, for the reason given.
    pub(crate) const fn unknown(why: String) -> Self {
        Self(Err(why))
    }

    /// The resolved socket.
    pub(crate) fn socket(&self) -> Result<&Path, &String> {
        self.0.as_deref()
    }
}

fn resolved(socket: &Path, euid: u32) -> Result<PathBuf, String> {
    let canonical =
        std::fs::canonicalize(socket).map_err(|error| format!("can't be resolved: {error}"))?;
    let meta = std::fs::metadata(&canonical)
        .map_err(|error| format!("can't be read once resolved: {error}"))?;
    if !meta.file_type().is_socket() || meta.uid() != euid {
        return Err(format!(
            "resolves to {}, which isn't a socket of user {euid}'s",
            canonical.display()
        ));
    }
    let dir = canonical.parent().unwrap_or_else(|| Path::new("/"));
    crate::discover::private(dir, euid)
        .map_err(|why| format!("resolves to {}, whose directory {why}", canonical.display()))?;
    Ok(canonical)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RuntimeDir {
    path: PathBuf,
}

impl RuntimeDir {
    /// The directory for the niri instance `env` resolved. Nothing is created.
    pub(crate) fn of(env: &Env) -> Result<Self, String> {
        let socket = env.instance.socket().map_err(Clone::clone)?;
        let (Some(dir), Some(name)) = (socket.parent(), socket.file_name()) else {
            return Err(format!("NIRI_SOCKET {} has no file name", socket.display()));
        };
        let name = name.to_string_lossy();
        let instance = name.strip_suffix(".sock").unwrap_or(&name);
        if instance.is_empty() {
            return Err(format!(
                "NIRI_SOCKET {} has no instance name",
                socket.display()
            ));
        }
        Ok(Self {
            path: dir.join("niri-computer-use").join(instance),
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

    fn euid() -> u32 {
        rustix::process::geteuid().as_raw()
    }

    /// A private `dir` holding a socket file called `name`.
    fn socket_in(dir: &Path, name: &str) -> PathBuf {
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let socket = dir.join(name);
        drop(std::os::unix::net::UnixListener::bind(&socket).unwrap());
        socket
    }

    fn of(socket: &Path) -> Result<RuntimeDir, String> {
        RuntimeDir::of(&Env {
            instance: Instance::resolve(socket, euid()),
            ..Env::default()
        })
    }

    #[test]
    fn is_named_after_the_niri_instance() {
        let dir = crate::test_support::fresh_dir("runtime-named");
        let socket = socket_in(&dir, "niri.wayland-1.42.sock");
        let canonical = std::fs::canonicalize(&dir).unwrap();
        assert_eq!(
            of(&socket).unwrap().path(),
            canonical.join("niri-computer-use/niri.wayland-1.42")
        );
        assert_eq!(
            RuntimeDir::of(&Env::default()),
            Err("NIRI_SOCKET is not set".to_owned())
        );
        // Without a name, the flag would land in the directory every instance shares.
        let nameless = socket_in(&dir, ".sock");
        let error = of(&nameless).unwrap_err();
        assert!(error.ends_with("has no instance name"), "{error}");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn every_spelling_of_one_socket_names_one_directory() {
        let dir = crate::test_support::fresh_dir("runtime-spelling");
        let real = dir.join("real");
        std::fs::create_dir(&real).unwrap();
        let socket = socket_in(&real, "niri.wayland-1.42.sock");
        // Another runtime root, as a client with another `XDG_RUNTIME_DIR` would see it,
        // and a socket link whose own name says nothing of the instance.
        std::os::unix::fs::symlink(&real, dir.join("root")).unwrap();
        std::os::unix::fs::symlink(&socket, dir.join("anything")).unwrap();
        let expected = of(&socket).unwrap();
        for spelling in [
            dir.join("root/niri.wayland-1.42.sock"),
            dir.join("anything"),
            real.join("./niri.wayland-1.42.sock"),
        ] {
            assert_eq!(of(&spelling).unwrap(), expected, "{}", spelling.display());
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_socket_that_is_not_the_users_own_private_one_names_no_directory() {
        let dir = crate::test_support::fresh_dir("runtime-refused");
        let socket = socket_in(&dir, "niri.wayland-1.42.sock");
        std::fs::write(dir.join("niri.file.sock"), "").unwrap();
        for (instance, why) in [
            (
                Instance::resolve(&dir.join("niri.file.sock"), euid()),
                "isn't a socket",
            ),
            (
                Instance::resolve(&dir.join("niri.gone.sock"), euid()),
                "can't be resolved",
            ),
            (
                Instance::resolve(&socket, euid() + 1),
                "isn't a socket of user",
            ),
        ] {
            let error = RuntimeDir::of(&Env {
                instance,
                ..Env::default()
            })
            .unwrap_err();
            assert!(error.contains(why), "{error}");
        }
        // Anyone could have made that socket in a directory others may write to.
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o770)).unwrap();
        let error = of(&socket).unwrap_err();
        assert!(error.contains("whose directory"), "{error}");
        assert!(error.ends_with("no lease, stop flag or marker"), "{error}");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn stop_sets_a_private_flag_and_resume_clears_it() {
        let dir = crate::test_support::fresh_dir("runtime");
        let runtime = of(&socket_in(&dir, "niri.wayland-1.42.sock")).unwrap();
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
        let runtime = of(&socket_in(&dir, "niri.wayland-1.42.sock")).unwrap();
        runtime.stop().unwrap();
        std::fs::write(runtime.path().join(INPUT_DIRTY), "").unwrap();
        let error = runtime.resume().unwrap_err();
        assert!(error.contains("recover"), "{error}");
        assert!(runtime.stopped().unwrap());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
