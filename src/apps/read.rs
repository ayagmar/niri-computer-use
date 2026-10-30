//! Reading one entry file, only when the user can't have written it. An entry's `Exec`
//! line is a command, and the preset rules are a list of names, not a boundary, so the
//! server takes a command only from a file that `owner`, root in a real session, owns and
//! that neither its group nor anyone else may write, in directories that are the same, from
//! the file up to its data directory. That keeps `/usr/share`, `/usr/local/share` and
//! `/var/lib/flatpak/exports`, and leaves out `~/.local/share` and a user's flatpak exports.

use std::io::Read as _;
use std::os::fd::{AsFd as _, AsRawFd as _};
use std::path::{Path, PathBuf};

use rustix::fs::{FileType, Mode, OFlags};

/// The longest entry file read; real ones are a few KiB.
const MAX_BYTES: u64 = 64 * 1024;

/// Why a file wasn't read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Unread {
    /// The user, or someone other than `owner`, could have written it: why.
    Writable(String),
    /// It couldn't be opened or read, or isn't an entry file: why.
    Failed(String),
}

/// What decides who may have written a file or directory: its owner and its mode bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Stat {
    pub(super) uid: u32,
    pub(super) mode: u32,
}

impl Stat {
    const fn of(stat: &rustix::fs::Stat) -> Self {
        Self {
            uid: stat.st_uid,
            mode: stat.st_mode,
        }
    }
}

/// Whether only `owner` may write what `stat` describes: `owner` owns it, and neither its
/// group nor others may write it. A sticky directory such as `/tmp` is world-writable, so
/// it doesn't count.
pub(super) const fn trusted(stat: Stat, owner: u32) -> bool {
    stat.uid == owner && stat.mode & 0o022 == 0
}

/// The text of the entry file at `path`, found under `data_dir`, if only `owner` could have
/// written it. It is opened without blocking, so a FIFO put in its place can't hang the read,
/// and checked through the open file: a regular file, then each directory of the path it
/// resolves to, up to `data_dir` or, for a file outside it as a symlink can make it, up to
/// `/`.
pub(super) fn read(path: &Path, data_dir: &Path, owner: u32) -> Result<String, Unread> {
    let failed = |error: &dyn std::fmt::Display| Unread::Failed(format!("can't be read: {error}"));
    let flags = OFlags::RDONLY | OFlags::NONBLOCK | OFlags::CLOEXEC | OFlags::NOCTTY;
    let fd = rustix::fs::open(path, flags, Mode::empty()).map_err(|error| failed(&error))?;
    let stat = rustix::fs::fstat(fd.as_fd()).map_err(|error| failed(&error))?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
        return Err(Unread::Failed("isn't a regular file".to_owned()));
    }
    let resolved = std::fs::read_link(format!("/proc/self/fd/{}", fd.as_raw_fd()))
        .map_err(|error| failed(&error))?;
    writable(&resolved, Stat::of(&stat), data_dir, owner)?;
    let mut text = String::new();
    std::fs::File::from(fd)
        .take(MAX_BYTES + 1)
        .read_to_string(&mut text)
        .map_err(|error| failed(&error))?;
    if text.len() as u64 > MAX_BYTES {
        return Err(Unread::Failed(format!("is over {MAX_BYTES} bytes")));
    }
    Ok(text)
}

/// `Writable` unless the file `resolved`, with `stat`, and each directory above it up to
/// `data_dir`, or `/`, is `trusted`.
fn writable(resolved: &Path, file: Stat, data_dir: &Path, owner: u32) -> Result<(), Unread> {
    let refuse = |what: &Path, found: Stat| {
        Err(Unread::Writable(format!(
            "{} belongs to uid {} with mode {:o}",
            what.display(),
            found.uid,
            found.mode & 0o7777
        )))
    };
    if !trusted(file, owner) {
        return refuse(resolved, file);
    }
    let top: PathBuf = std::fs::canonicalize(data_dir).unwrap_or_else(|_| data_dir.to_owned());
    let stop = if resolved.starts_with(&top) {
        top
    } else {
        PathBuf::from("/")
    };
    for dir in resolved.ancestors().skip(1) {
        let above = rustix::fs::stat(dir)
            .map_err(|error| Unread::Failed(format!("can't check {}: {error}", dir.display())))?;
        if !trusted(Stat::of(&above), owner) {
            return refuse(dir, Stat::of(&above));
        }
        if dir == stop {
            break;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;

    use super::*;

    #[test]
    fn only_a_root_owned_file_nobody_else_may_write_is_trusted() {
        let stat = |uid, mode| Stat { uid, mode };
        // A file and a directory as a package manager installs them.
        assert!(trusted(stat(0, 0o100_644), 0));
        assert!(trusted(stat(0, 0o040_755), 0));
        assert!(trusted(stat(0, 0o100_444), 0));
        // The user's own, or writable by a group or by anyone.
        assert!(!trusted(stat(1000, 0o100_644), 0));
        assert!(!trusted(stat(0, 0o100_664), 0));
        assert!(!trusted(stat(0, 0o100_646), 0));
        assert!(!trusted(stat(0, 0o041_777), 0));
        assert!(!trusted(stat(0, 0o040_775), 0));
    }

    #[test]
    fn a_file_the_user_owns_is_never_read() {
        let dir = crate::test_support::fresh_dir("apps-read-own");
        let file = dir.join("applications/evil.desktop");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(
            &file,
            "[Desktop Entry]\nType=Application\nExec=touch pwned\n",
        )
        .unwrap();
        assert!(
            matches!(read(&file, &dir, 0), Err(Unread::Writable(why)) if why.contains("evil.desktop")),
            "{:?}",
            read(&file, &dir, 0)
        );
        // Under the owner a test trusts, the same file is read, but not once its directory
        // lets the group write.
        let me = rustix::process::geteuid().as_raw();
        for path in [&dir, &dir.join("applications")] {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(read(&file, &dir, me).is_ok());
        let applications = dir.join("applications");
        std::fs::set_permissions(&applications, std::fs::Permissions::from_mode(0o775)).unwrap();
        assert!(matches!(read(&file, &dir, me), Err(Unread::Writable(_))));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_fifo_in_place_of_an_entry_is_refused_without_blocking() {
        let dir = crate::test_support::fresh_dir("apps-read-fifo");
        let fifo = dir.join("x.desktop");
        rustix::fs::mknodat(
            rustix::fs::CWD,
            &fifo,
            FileType::Fifo,
            Mode::from_raw_mode(0o644),
            0,
        )
        .unwrap();
        let me = rustix::process::geteuid().as_raw();
        assert_eq!(
            read(&fifo, &dir, me),
            Err(Unread::Failed("isn't a regular file".to_owned()))
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
