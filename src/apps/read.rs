//! Reading one entry file, only when the user can't have written it or changed the path to
//! it. An entry's `Exec` line is a command, and the preset rules are a list of names, not a
//! boundary, so the server takes a command only from a file that root, or `owner` in a
//! test, owns and that neither its group nor anyone else may write, reached from `/`
//! through directories that are the same. That keeps `/usr/share`, `/usr/local/share` and
//! `/var/lib/flatpak/exports`, and leaves out `~/.local/share` and a user's flatpak exports.

use std::collections::VecDeque;
use std::ffi::{OsStr, OsString};
use std::io::Read as _;
use std::os::fd::{AsFd as _, OwnedFd};
use std::os::unix::ffi::OsStrExt as _;
use std::path::{Component, Path, PathBuf};

use rustix::fs::{FileType, Mode, OFlags};

/// The longest entry file read; real ones are a few KiB.
const MAX_BYTES: u64 = 64 * 1024;

/// The most symlinks one entry's path may lead through, as the kernel allows.
const MAX_LINKS: usize = 40;

/// The kernel's own files, whose contents other programs choose: a process's `cmdline` is
/// root's when the process is a setuid program, whoever started it.
const KERNEL: [&str; 3] = ["/proc", "/sys", "/dev"];

/// The sticky bit: in a directory, only a name's owner, the directory's or root may
/// rename or remove the name.
const STICKY: u32 = 0o1000;

/// Why a file wasn't read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Unread {
    /// The user, or someone other than root and `owner`, could have written it or changed
    /// the path to it: why.
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

    const fn owned(self, owner: u32) -> bool {
        self.uid == 0 || self.uid == owner
    }
}

/// Whether only root or `owner` may write what `stat` describes: one of them owns it, and
/// neither its group nor others may write it.
pub(super) const fn trusted(stat: Stat, owner: u32) -> bool {
    stat.owned(owner) && stat.mode & 0o022 == 0
}

/// Whether only root or `owner` may rename or replace the names in the directory `dir`
/// that they own: it is `trusted`, or sticky, as `/tmp` is, and theirs.
const fn keeps_names(dir: Stat, owner: u32) -> bool {
    trusted(dir, owner) || (dir.mode & STICKY != 0 && dir.owned(owner))
}

/// Whether `name`, found in the directory `dir`, can't be renamed or replaced by anyone but
/// root or `owner`: `dir` is `trusted`, or it `keeps_names` and they own `name`.
pub(super) const fn trusted_in(dir: Stat, name: Stat, owner: u32) -> bool {
    trusted(dir, owner) || (keeps_names(dir, owner) && name.owned(owner))
}

/// The text of the entry file at `path`, an absolute path, if only root or `owner` could
/// have written it or any directory on the way, from `/` down and through every symlink.
/// A link in a directory the user can write is refused, whatever it leads to.
pub(super) fn read(path: &Path, owner: u32) -> Result<String, Unread> {
    text(open(path, owner)?)
}

fn failed(error: &dyn std::fmt::Display) -> Unread {
    Unread::Failed(format!("can't be read: {error}"))
}

/// The file at `path`, open to read, walked to one name at a time, so what is checked is
/// what is opened.
fn open(path: &Path, owner: u32) -> Result<OwnedFd, Unread> {
    if !path.is_absolute() {
        return Err(Unread::Failed("isn't an absolute path".to_owned()));
    }
    let mut walk = Walk::new(owner)?;
    let mut left: VecDeque<Step> = steps(path.as_os_str()).collect();
    while let Some(step) = left.pop_front() {
        match step {
            Step::Root => walk.root()?,
            Step::Up => walk.up(),
            Step::Name(name) => {
                if let Some(file) = walk.name(&name, &mut left)? {
                    return Ok(file);
                }
            }
        }
    }
    Err(Unread::Failed("isn't a regular file".to_owned()))
}

/// The text of the open entry file `file`, if it isn't too long.
fn text(file: OwnedFd) -> Result<String, Unread> {
    let mut text = String::new();
    std::fs::File::from(file)
        .take(MAX_BYTES + 1)
        .read_to_string(&mut text)
        .map_err(|error| failed(&error))?;
    if text.len() as u64 > MAX_BYTES {
        return Err(Unread::Failed(format!("is over {MAX_BYTES} bytes")));
    }
    Ok(text)
}

/// One step along a path: to `/`, up a directory, or to a name.
enum Step {
    Root,
    Up,
    Name(OsString),
}

fn steps(path: &OsStr) -> impl DoubleEndedIterator<Item = Step> + '_ {
    Path::new(path)
        .components()
        .filter_map(|component| match component {
            Component::RootDir => Some(Step::Root),
            Component::ParentDir => Some(Step::Up),
            Component::Normal(name) => Some(Step::Name(name.to_owned())),
            Component::CurDir | Component::Prefix(_) => None,
        })
}

/// A directory the walk went through, and its `Stat`, which decides what names in it count.
struct Dir {
    fd: OwnedFd,
    stat: Stat,
}

/// A walk along an entry's path from `/`, through directories only root or `owner` may
/// change, each checked as it is entered.
struct Walk {
    owner: u32,
    /// The directory the walk is in.
    here: Dir,
    /// The directories it went down through to `here`, for `..`.
    above: Vec<Dir>,
    /// The path of `here`.
    path: PathBuf,
    links: usize,
}

impl Walk {
    const DIR: OFlags = OFlags::PATH.union(OFlags::DIRECTORY).union(OFlags::CLOEXEC);

    fn new(owner: u32) -> Result<Self, Unread> {
        let fd = rustix::fs::open("/", Self::DIR, Mode::empty()).map_err(|e| failed(&e))?;
        let path = PathBuf::from("/");
        let stat = Self::checked(&path, &fd, owner)?;
        Ok(Self {
            owner,
            here: Dir { fd, stat },
            above: Vec::new(),
            path,
            links: 0,
        })
    }

    /// The `Stat` of the directory `fd` at `path`, or `Writable` if it is the kernel's or
    /// doesn't keep its names.
    fn checked(path: &Path, fd: &OwnedFd, owner: u32) -> Result<Stat, Unread> {
        if let Some(root) = KERNEL.iter().find(|root| path.starts_with(root)) {
            return Err(Unread::Writable(format!(
                "{} is in {root}, whose files other programs write",
                path.display()
            )));
        }
        let stat = Stat::of(&rustix::fs::fstat(fd).map_err(|error| failed(&error))?);
        if keeps_names(stat, owner) {
            Ok(stat)
        } else {
            Err(refused(path, stat))
        }
    }

    fn root(&mut self) -> Result<(), Unread> {
        *self = Self {
            links: self.links,
            ..Self::new(self.owner)?
        };
        Ok(())
    }

    /// Goes up to the directory `here` was entered from. Only `/` has none, and its `..` is
    /// itself.
    fn up(&mut self) {
        if let Some(parent) = self.above.pop() {
            self.path.pop();
            self.here = parent;
        }
    }

    /// Goes to `name` in `here`: into a directory, along a symlink, whose target's steps
    /// come before those `left`, or, at the end of the path, to the file, which it returns.
    fn name(&mut self, name: &OsStr, left: &mut VecDeque<Step>) -> Result<Option<OwnedFd>, Unread> {
        let flags = OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        let found = rustix::fs::openat(self.here.fd.as_fd(), name, flags, Mode::empty())
            .map_err(|error| failed(&error))?;
        let named = Stat::of(&rustix::fs::fstat(&found).map_err(|error| failed(&error))?);
        let path = self.path.join(name);
        if !trusted_in(self.here.stat, named, self.owner) {
            return Err(refused(&path, named));
        }
        match FileType::from_raw_mode(named.mode) {
            FileType::Directory => {
                let stat = Self::checked(&path, &found, self.owner)?;
                let parent = std::mem::replace(&mut self.here, Dir { fd: found, stat });
                self.above.push(parent);
                self.path = path;
            }
            FileType::Symlink => self.follow(name, left)?,
            FileType::RegularFile if left.is_empty() => return self.file(name).map(Some),
            FileType::RegularFile
            | FileType::Fifo
            | FileType::Socket
            | FileType::CharacterDevice
            | FileType::BlockDevice
            | FileType::Unknown => {
                return Err(Unread::Failed("isn't a regular file".to_owned()));
            }
        }
        Ok(None)
    }

    /// Puts the steps of the target of the symlink `name` in `here` before those `left`.
    fn follow(&mut self, name: &OsStr, left: &mut VecDeque<Step>) -> Result<(), Unread> {
        self.links += 1;
        if self.links > MAX_LINKS {
            return Err(Unread::Failed(format!(
                "leads through over {MAX_LINKS} symlinks"
            )));
        }
        let target = rustix::fs::readlinkat(self.here.fd.as_fd(), name, Vec::new())
            .map_err(|error| failed(&error))?;
        for step in steps(OsStr::from_bytes(target.as_bytes())).rev() {
            left.push_front(step);
        }
        Ok(())
    }

    /// The regular file `name` in `here`, opened without blocking, so a FIFO put in its
    /// place can't hang the read, if it is `trusted`.
    fn file(&self, name: &OsStr) -> Result<OwnedFd, Unread> {
        let flags = OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        let file = rustix::fs::openat(
            self.here.fd.as_fd(),
            name,
            flags | OFlags::NOCTTY,
            Mode::empty(),
        )
        .map_err(|error| failed(&error))?;
        let stat = Stat::of(&rustix::fs::fstat(&file).map_err(|error| failed(&error))?);
        if FileType::from_raw_mode(stat.mode) != FileType::RegularFile {
            return Err(Unread::Failed("isn't a regular file".to_owned()));
        }
        if !trusted(stat, self.owner) {
            return Err(refused(&self.path.join(name), stat));
        }
        Ok(file)
    }
}

/// `Writable`, naming `what` and its `found` owner and mode.
fn refused(what: &Path, found: Stat) -> Unread {
    Unread::Writable(format!(
        "{} belongs to uid {} with mode {:o}",
        what.display(),
        found.uid,
        found.mode & 0o7777
    ))
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;

    use super::*;

    const fn stat(uid: u32, mode: u32) -> Stat {
        Stat { uid, mode }
    }

    #[test]
    fn only_a_root_owned_file_nobody_else_may_write_is_trusted() {
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
    fn in_a_sticky_directory_only_root_s_names_count() {
        let tmp = stat(0, 0o041_777);
        // Anyone may add a name to `/tmp`, but only its owner may rename or remove it.
        assert!(trusted_in(tmp, stat(0, 0o040_755), 0));
        assert!(!trusted_in(tmp, stat(1000, 0o040_755), 0));
        assert!(!trusted_in(tmp, stat(1000, 0o120_777), 0));
        // A sticky directory the user owns, or a plain one anyone may write, keeps nothing.
        assert!(!trusted_in(stat(1000, 0o041_777), stat(0, 0o040_755), 0));
        assert!(!trusted_in(stat(0, 0o040_777), stat(0, 0o040_755), 0));
        // In a trusted directory, whose names only root may change, any name counts.
        assert!(trusted_in(stat(0, 0o040_755), stat(1000, 0o120_777), 0));
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
            matches!(read(&file, 0), Err(Unread::Writable(_))),
            "{:?}",
            read(&file, 0)
        );
        // Under the owner a test trusts, the same file is read, but not once its directory
        // lets the group write, or the file does.
        let me = rustix::process::geteuid().as_raw();
        let mode = |mode| std::fs::Permissions::from_mode(mode);
        let applications = dir.join("applications");
        for path in [&dir, &applications] {
            std::fs::set_permissions(path, mode(0o755)).unwrap();
        }
        std::fs::set_permissions(&file, mode(0o644)).unwrap();
        assert!(read(&file, me).is_ok());
        std::fs::set_permissions(&applications, mode(0o775)).unwrap();
        assert!(matches!(read(&file, me), Err(Unread::Writable(_))));
        std::fs::set_permissions(&applications, mode(0o755)).unwrap();
        std::fs::set_permissions(&file, mode(0o664)).unwrap();
        assert!(
            matches!(read(&file, me), Err(Unread::Writable(why)) if why.contains("evil.desktop"))
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    const ENTRY: &str = "[Desktop Entry]\nType=Application\nExec=true\n";

    /// A directory the test's user may trust, with `real/x.desktop` in it, and a directory
    /// `loose` in it that the group may write, as the user can write theirs.
    fn tree(name: &str) -> PathBuf {
        let dir = crate::test_support::fresh_dir(name);
        let mode = |mode| std::fs::Permissions::from_mode(mode);
        for (sub, bits) in [("applications", 0o755), ("real", 0o755), ("loose", 0o775)] {
            std::fs::create_dir(dir.join(sub)).unwrap();
            std::fs::set_permissions(dir.join(sub), mode(bits)).unwrap();
        }
        std::fs::set_permissions(&dir, mode(0o755)).unwrap();
        std::fs::write(dir.join("real/x.desktop"), ENTRY).unwrap();
        std::fs::set_permissions(dir.join("real/x.desktop"), mode(0o644)).unwrap();
        dir
    }

    fn link(target: impl AsRef<Path>, at: &Path) {
        std::os::unix::fs::symlink(target, at).unwrap();
    }

    fn assert_refused(found: &Result<String, Unread>, naming: &str) {
        assert!(
            matches!(found, Err(Unread::Writable(why)) if why.contains(naming)),
            "{found:?}"
        );
    }

    #[test]
    fn a_link_to_a_trusted_file_is_read_only_from_a_trusted_directory() {
        let dir = tree("apps-read-link");
        let me = rustix::process::geteuid().as_raw();
        link("../real/x.desktop", &dir.join("applications/x.desktop"));
        link(
            dir.join("real/x.desktop"),
            &dir.join("applications/y.desktop"),
        );
        for file in ["applications/x.desktop", "applications/y.desktop"] {
            assert_eq!(read(&dir.join(file), me), Ok(ENTRY.to_owned()), "{file}");
        }
        // In a directory the user may write, the same link is refused: they could point it
        // at any file root owns, such as a setuid process's `cmdline`. So is one in a trusted
        // directory that leads through a link in such a directory.
        link("../real/x.desktop", &dir.join("loose/x.desktop"));
        link("../loose/x.desktop", &dir.join("applications/z.desktop"));
        assert_refused(&read(&dir.join("loose/x.desktop"), me), "loose");
        assert_refused(&read(&dir.join("applications/z.desktop"), me), "loose");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_data_directory_reached_through_a_writable_one_is_refused() {
        // The user can swap their own `~/.local/share` for a link to any tree root owns
        // that has an `applications` directory, so the walk starts at `/`.
        let dir = tree("apps-read-data-link");
        let me = rustix::process::geteuid().as_raw();
        link("..", &dir.join("loose/share"));
        assert_refused(&read(&dir.join("loose/share/real/x.desktop"), me), "loose");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn flatpak_s_exported_links_are_read() {
        // As in `/var/lib/flatpak`: the exported link leads up out of the data directory and
        // through two directory links, all placed by root.
        let root = crate::test_support::fresh_dir("apps-read-flatpak");
        let me = rustix::process::geteuid().as_raw();
        let app = root.join("app/org.x/x86_64/stable/1a2b/export/share/applications");
        std::fs::create_dir_all(&app).unwrap();
        std::fs::write(app.join("org.x.desktop"), ENTRY).unwrap();
        link("x86_64/stable", &root.join("app/org.x/current"));
        link("1a2b", &root.join("app/org.x/x86_64/stable/active"));
        let exports = root.join("exports/share/applications");
        std::fs::create_dir_all(&exports).unwrap();
        let target = "../../../app/org.x/current/active/export/share/applications/org.x.desktop";
        link(target, &exports.join("org.x.desktop"));
        let mode = |mode| std::fs::Permissions::from_mode(mode);
        std::fs::set_permissions(app.join("org.x.desktop"), mode(0o644)).unwrap();
        for dir in app.ancestors().chain(exports.ancestors()) {
            if dir.starts_with(&root) {
                std::fs::set_permissions(dir, mode(0o755)).unwrap();
            }
        }
        assert_eq!(
            read(&exports.join("org.x.desktop"), me),
            Ok(ENTRY.to_owned())
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_entry_in_a_writable_subdirectory_of_a_trusted_one_is_refused() {
        let dir = tree("apps-read-subdir");
        let me = rustix::process::geteuid().as_raw();
        let sub = dir.join("applications/sub");
        std::fs::create_dir(&sub).unwrap();
        std::fs::set_permissions(&sub, std::fs::Permissions::from_mode(0o775)).unwrap();
        link("../../real/x.desktop", &sub.join("x.desktop"));
        assert_refused(&read(&sub.join("x.desktop"), me), "sub");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_file_of_the_kernel_is_never_read() {
        // This process's `cmdline` and its directory are the test's user's and can't be
        // written, as a setuid process's are root's.
        let dir = tree("apps-read-kernel");
        let me = rustix::process::geteuid().as_raw();
        link("/proc/self/cmdline", &dir.join("applications/x.desktop"));
        assert_refused(&read(&dir.join("applications/x.desktop"), me), "/proc");
        assert_refused(&read(Path::new("/proc/self/cmdline"), me), "/proc");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_file_replaced_after_its_check_is_read_as_checked() {
        let dir = tree("apps-read-replaced");
        let me = rustix::process::geteuid().as_raw();
        let file = open(&dir.join("real/x.desktop"), me).unwrap();
        std::fs::write(dir.join("evil"), "[Desktop Entry]\nExec=touch pwned\n").unwrap();
        std::fs::rename(dir.join("evil"), dir.join("real/x.desktop")).unwrap();
        assert_eq!(text(file), Ok(ENTRY.to_owned()));
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
            read(&fifo, me),
            Err(Unread::Failed("isn't a regular file".to_owned()))
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
