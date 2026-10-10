//! Writes a saved screenshot inside the policy's capture directory. Each subdirectory is
//! opened without following a symlink, and the file is created exclusively without
//! following one, so a save can't leave the directory or replace a file.

use std::fs::{DirBuilder, File};
use std::io::Write as _;
use std::os::fd::OwnedFd;
use std::os::unix::fs::DirBuilderExt as _;
use std::path::{Path, PathBuf};

use rustix::fs::{AtFlags, Mode, OFlags};
use rustix::io::Errno;

use crate::error::{CallError, ErrorName, ToolError};
use crate::policy::SaveTarget;

/// Writes `png` to `target`, creating the capture directory with mode `0700` if it is
/// missing, and returns the file's path. The file gets mode `0600`.
pub(crate) fn write(target: &SaveTarget, png: &[u8]) -> Result<PathBuf, CallError> {
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&target.dir)
        .map_err(|error| failed(&target.dir, &error.to_string()))?;
    let flags = OFlags::DIRECTORY | OFlags::RDONLY | OFlags::CLOEXEC;
    let mut dir = rustix::fs::open(&target.dir, flags, Mode::empty())
        .map_err(|errno| failed(&target.dir, &errno.to_string()))?;
    let mut path = target.dir.clone();
    for name in &target.path.dirs {
        path.push(name);
        dir = subdirectory(&dir, name, &path)?;
    }
    path.push(&target.path.file);
    let name = target.path.file.as_str();
    let create =
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let file =
        rustix::fs::openat(&dir, name, create, Mode::RUSR | Mode::WUSR).map_err(|errno| {
            if errno == Errno::EXIST {
                CallError::InvalidArguments(format!(
                    "{} already exists; pick another save_path",
                    path.display()
                ))
            } else {
                failed(&path, &errno.to_string())
            }
        })?;
    if let Err(error) = File::from(file).write_all(png) {
        let removed = rustix::fs::unlinkat(&dir, name, AtFlags::empty()).map_or_else(
            |errno| format!(", and removing it failed: {errno}"),
            |()| String::new(),
        );
        return Err(failed(&path, &format!("{error}{removed}")));
    }
    Ok(path)
}

/// Opens the subdirectory `name` of `dir`, refusing a symlink.
fn subdirectory(dir: &OwnedFd, name: &str, path: &Path) -> Result<OwnedFd, CallError> {
    let flags = OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::RDONLY | OFlags::CLOEXEC;
    rustix::fs::openat(dir, name, flags, Mode::empty()).map_err(|errno| match errno {
        Errno::LOOP | Errno::NOTDIR => CallError::InvalidArguments(format!(
            "{} is a symlink or not a directory; save_path can't leave capture_dir",
            path.display()
        )),
        Errno::NOENT => CallError::InvalidArguments(format!(
            "{} doesn't exist; save_path's directories must exist",
            path.display()
        )),
        other => failed(path, &other.to_string()),
    })
}

fn failed(path: &Path, error: &str) -> CallError {
    ToolError::new(
        ErrorName::UpstreamError,
        format!("save {}: {error}", path.display()),
    )
    .into()
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::{PermissionsExt as _, symlink};

    use super::*;
    use crate::policy::SavePath;
    use crate::test_support::fresh_dir;

    fn target(dir: &Path, path: &str) -> SaveTarget {
        SaveTarget {
            dir: dir.to_path_buf(),
            path: SavePath::parse(path).unwrap(),
        }
    }

    fn mistake(result: Result<PathBuf, CallError>) -> String {
        match result {
            Err(CallError::InvalidArguments(message)) => message,
            other => panic!("not an argument mistake: {other:?}"),
        }
    }

    #[test]
    fn writes_a_new_private_file_and_never_overwrites() {
        let root = fresh_dir("save-new");
        let dir = root.join("shots");
        let saved = write(&target(&dir, "one.png"), b"png").unwrap();
        assert_eq!(saved, dir.join("one.png"));
        assert_eq!(std::fs::read(&saved).unwrap(), b"png");
        let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!((mode(&dir), mode(&saved)), (0o700, 0o600));
        assert!(mistake(write(&target(&dir, "one.png"), b"other")).contains("already exists"));
        assert_eq!(std::fs::read(&saved).unwrap(), b"png");
        std::fs::create_dir(dir.join("readme")).unwrap();
        assert_eq!(
            write(&target(&dir, "readme/two.png"), b"png").unwrap(),
            dir.join("readme/two.png")
        );
        assert!(mistake(write(&target(&dir, "missing/two.png"), b"png")).contains("doesn't exist"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn symlinks_cant_lead_out_of_the_capture_dir() {
        let root = fresh_dir("save-symlink");
        let (dir, outside) = (root.join("shots"), root.join("outside"));
        std::fs::create_dir(&dir).unwrap();
        std::fs::create_dir(&outside).unwrap();
        symlink(&outside, dir.join("escape")).unwrap();
        assert!(mistake(write(&target(&dir, "escape/x.png"), b"png")).contains("symlink"));
        // A symlink at the file's name, even a dangling one, isn't followed.
        symlink(outside.join("x.png"), dir.join("x.png")).unwrap();
        assert!(mistake(write(&target(&dir, "x.png"), b"png")).contains("already exists"));
        assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
        std::fs::remove_dir_all(root).unwrap();
    }
}
