//! Writes a saved screenshot inside the capture directory, the policy's or the default. Each subdirectory is
//! opened without following a symlink, and the file is created exclusively without
//! following one, so a save can't leave the directory or replace a file.
//!
//! The write runs on a thread of its own, one at a time, and a call waits for it at most
//! `LIMIT`, so a capture directory on a file system that stalls, such as NFS or FUSE,
//! never holds up the runtime and holds at most one thread.

use std::fmt;
use std::fs::{DirBuilder, File};
use std::io::Write as _;
use std::os::fd::OwnedFd;
use std::os::unix::fs::DirBuilderExt as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use rustix::fs::{AtFlags, Mode, OFlags};
use rustix::io::Errno;

use crate::error::{CallError, ErrorName, ToolError};
use crate::policy::SaveTarget;

/// How long a call waits for its save.
const LIMIT: Duration = Duration::from_secs(5);

/// Whether a save is running; another is refused until it ends.
static SAVING: AtomicBool = AtomicBool::new(false);

/// Writes `png` to `target` as `write` does, on a thread of its own, within `LIMIT`.
pub(crate) async fn write_off_runtime(
    target: &SaveTarget,
    png: Vec<u8>,
) -> Result<PathBuf, CallError> {
    let owned = target.clone();
    match off_runtime(&SAVING, LIMIT, move || write(&owned, &png)).await {
        Ok(written) => written,
        Err(stalled @ Stalled::Late(_)) => Err(ToolError::new(
            ErrorName::DeadlineExceeded,
            format!("save {}: {stalled}", target.dir.display()),
        )
        .into()),
        Err(stalled) => Err(failed(&target.dir, &stalled.to_string())),
    }
}

/// Why a job run off the runtime gave no answer.
#[derive(Debug, PartialEq, Eq)]
enum Stalled {
    /// The last job hasn't ended.
    Busy,
    /// The job took longer than this.
    Late(Duration),
    /// The job's thread ended without an answer.
    Ended,
    /// The thread couldn't be started.
    Unstarted(String),
}

impl fmt::Display for Stalled {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Busy => f.write_str(
                "the last save hasn't finished, as on a stalled file system; nothing was saved",
            ),
            Self::Late(limit) => write!(
                f,
                "took over {} ms, as on a stalled file system; the file may still appear",
                limit.as_millis()
            ),
            Self::Ended => f.write_str("the save's thread ended without an answer"),
            Self::Unstarted(error) => write!(f, "couldn't start the save's thread: {error}"),
        }
    }
}

/// Clears its flag when dropped.
struct Clear(&'static AtomicBool);

impl Drop for Clear {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

/// Runs `job` on a thread of its own while `busy` is clear, and waits for it at most
/// `limit`. A job that runs past it keeps `busy` set until it ends.
async fn off_runtime<T: Send + 'static>(
    busy: &'static AtomicBool,
    limit: Duration,
    job: impl FnOnce() -> T + Send + 'static,
) -> Result<T, Stalled> {
    if busy.swap(true, Ordering::AcqRel) {
        return Err(Stalled::Busy);
    }
    let (sent, received) = tokio::sync::oneshot::channel();
    let spawned = std::thread::Builder::new()
        .name("save".to_owned())
        .spawn(move || {
            // Clears `busy` however the job ends, a panic included.
            let _clear = Clear(busy);
            let done = job();
            // The call may have stopped waiting; the job's work is done all the same.
            let _ignored = sent.send(done);
        });
    if let Err(error) = spawned {
        busy.store(false, Ordering::Release);
        return Err(Stalled::Unstarted(error.to_string()));
    }
    match tokio::time::timeout(limit, received).await {
        Ok(Ok(done)) => Ok(done),
        Ok(Err(_)) => Err(Stalled::Ended),
        Err(_) => Err(Stalled::Late(limit)),
    }
}

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

    /// A save that stalls, as on a capture directory over NFS, ends the call at its limit,
    /// refuses the next one while it still runs, and lets saves through once it ends.
    #[tokio::test]
    async fn a_stalled_save_ends_the_call_at_its_limit_and_holds_one_thread() {
        static BUSY: AtomicBool = AtomicBool::new(false);
        let (release, stalled) = std::sync::mpsc::channel::<()>();
        let limit = Duration::from_millis(50);
        let started = std::time::Instant::now();

        let first = off_runtime(&BUSY, limit, move || stalled.recv().is_ok()).await;

        assert_eq!(first, Err(Stalled::Late(limit)));
        assert!(started.elapsed() < Duration::from_secs(2));
        assert_eq!(off_runtime(&BUSY, limit, || true).await, Err(Stalled::Busy));
        release.send(()).unwrap();
        let by = std::time::Instant::now() + Duration::from_secs(5);
        while BUSY.load(Ordering::Acquire) && std::time::Instant::now() < by {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(off_runtime(&BUSY, limit, || 7).await, Ok(7));
    }

    /// A save whose thread panics fails that call and leaves the next one free to run.
    #[tokio::test]
    async fn a_save_that_panics_doesnt_refuse_the_next() {
        static BUSY: AtomicBool = AtomicBool::new(false);
        let limit = Duration::from_secs(5);

        let panicked = off_runtime(&BUSY, limit, || -> i32 { panic!("the save panicked") }).await;

        assert_eq!(panicked, Err(Stalled::Ended));
        assert_eq!(off_runtime(&BUSY, limit, || 7).await, Ok(7));
    }

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
