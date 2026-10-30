//! Wakes `watch` as soon as something changes in the runtime directory, so a stop, a
//! lease and the action in progress show at once instead of at the next poll. The poll
//! stays the deadline: a directory that doesn't exist yet, or was replaced, is watched
//! again from the next poll on, and until then the poll alone serves, as before.

use std::io;
use std::path::PathBuf;

use rustix::fd::OwnedFd;
use rustix::fs::inotify;
use rustix::io::Errno;
use tokio::io::unix::AsyncFd;

use crate::live;

/// Names added, removed and moved, and the directory itself going away: not writes, so
/// `engine.log`'s lines don't wake `watch`.
const MASK: inotify::WatchFlags = inotify::WatchFlags::CREATE
    .union(inotify::WatchFlags::DELETE)
    .union(inotify::WatchFlags::MOVED_TO)
    .union(inotify::WatchFlags::MOVED_FROM)
    .union(inotify::WatchFlags::DELETE_SELF)
    .union(inotify::WatchFlags::MOVE_SELF)
    .union(inotify::WatchFlags::ONLYDIR);

/// The runtime directory, watched when it can be.
#[derive(Debug)]
pub(crate) struct Wake {
    dir: PathBuf,
    /// One inotify instance for the whole run, so the kernel tells whether the directory
    /// at `dir` is still the one watched.
    fd: Option<AsyncFd<OwnedFd>>,
    /// The watch on the directory at `dir`, while there is one.
    wd: Option<i32>,
}

/// What a batch of events says.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Woken {
    /// `live.json` was replaced, or events were lost and it may have been.
    pub(crate) live: bool,
    /// The watched directory itself was removed or moved away.
    lost: bool,
}

impl Wake {
    pub(crate) fn new(dir: PathBuf) -> Self {
        let mut wake = Self {
            dir,
            fd: None,
            wd: None,
        };
        wake.rearm();
        wake
    }

    /// Watches the directory at `dir`. For the directory already watched the kernel gives
    /// the same watch back; for one that replaced it, a new watch, even when the new
    /// directory has the old one's inode number, as ext4 gives a directory made right after
    /// one was removed. Comparing device and inode numbers kept the dead watch then.
    pub(crate) fn rearm(&mut self) {
        if self.fd.is_none() {
            self.fd = instance().ok();
        }
        let Some(fd) = &self.fd else {
            return;
        };
        let now = inotify::add_watch(fd.get_ref(), &self.dir, MASK).ok();
        if let Some(old) = self.wd
            && now != Some(old)
        {
            // A directory moved away is still watched; one removed is gone already.
            inotify::remove_watch(fd.get_ref(), old).ok();
        }
        self.wd = now;
    }

    /// The next batch of events; never, while the directory isn't watched.
    pub(crate) async fn changed(&mut self) -> Woken {
        let (Some(fd), Some(wd)) = (&self.fd, self.wd) else {
            return std::future::pending().await;
        };
        let read = match fd.readable().await {
            Ok(mut ready) => {
                let read = drain(ready.get_inner(), wd);
                ready.clear_ready();
                read
            }
            Err(error) => Err(error),
        };
        let woken = read.unwrap_or_else(|_| {
            // The instance can't be read: start another at the next poll.
            self.fd = None;
            Woken {
                live: true,
                lost: true,
            }
        });
        if woken.lost {
            self.wd = None;
        }
        woken
    }
}

fn instance() -> io::Result<AsyncFd<OwnedFd>> {
    let fd = inotify::init(inotify::CreateFlags::NONBLOCK | inotify::CreateFlags::CLOEXEC)?;
    #[expect(
        deprecated,
        reason = "the AsyncFd owns its OwnedFd, so nothing can close or replace the descriptor while it is registered (RUSTSEC-2026-0386)"
    )]
    let fd = AsyncFd::new(fd)?;
    Ok(fd)
}

/// Reads every queued event. Only `wd` going away loses the watch: a replaced directory's
/// last events can still be queued after the new one is watched.
fn drain(fd: &OwnedFd, wd: i32) -> io::Result<Woken> {
    let gone = inotify::ReadFlags::DELETE_SELF
        | inotify::ReadFlags::MOVE_SELF
        | inotify::ReadFlags::IGNORED;
    let mut buffer = [std::mem::MaybeUninit::uninit(); 4096];
    let mut reader = inotify::Reader::new(fd, &mut buffer);
    let mut woken = Woken::default();
    loop {
        match reader.next() {
            Ok(event) => {
                let flags = event.events();
                woken.lost |= event.wd() == wd && flags.intersects(gone);
                woken.live |= flags.contains(inotify::ReadFlags::QUEUE_OVERFLOW)
                    || (flags.contains(inotify::ReadFlags::MOVED_TO)
                        && event
                            .file_name()
                            .is_some_and(|name| name.to_bytes() == live::FILE.as_bytes()));
            }
            Err(Errno::AGAIN) => return Ok(woken),
            Err(error) => return Err(error.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    async fn within_a_second(wake: &mut Wake) -> Option<Woken> {
        tokio::time::timeout(Duration::from_secs(1), wake.changed())
            .await
            .ok()
    }

    fn replace_live(dir: &std::path::Path) {
        std::fs::write(dir.join("live.json.1.tmp"), "{}").unwrap();
        std::fs::rename(dir.join("live.json.1.tmp"), dir.join(live::FILE)).unwrap();
    }

    // The bugs: a `watch` started before the engine made the directory, or after it was
    // made again, that never woke for it and showed every change a poll late; and, where
    // the new directory got the old one's inode number (ext4), one that kept the old
    // directory's dead watch.
    #[tokio::test]
    async fn watches_a_directory_made_after_it_started_and_again_once_replaced() {
        let root = crate::test_support::fresh_dir("watch-wake");
        let dir = root.join("instance");
        let mut wake = Wake::new(dir.clone());
        assert!(wake.wd.is_none());
        std::fs::create_dir(&dir).unwrap();
        wake.rearm();
        replace_live(&dir);
        let woken = within_a_second(&mut wake).await;
        assert!(woken.is_some_and(|woken| woken.live), "{woken:?}");
        std::fs::remove_dir_all(&dir).unwrap();
        std::fs::create_dir(&dir).unwrap();
        wake.rearm();
        // The old directory's last events are still queued: they don't lose the new watch.
        replace_live(&dir);
        let again = within_a_second(&mut wake).await;
        assert!(again.is_some_and(|again| again.live), "{again:?}");
        assert!(wake.wd.is_some());
        std::fs::remove_dir_all(root).unwrap();
    }

    // The bug: every line the engine writes to `engine.log` waking `watch` to read
    // `live.json` again.
    #[tokio::test]
    async fn only_a_replaced_record_says_to_read_it() {
        let dir = crate::test_support::fresh_dir("watch-wake-names");
        let mut wake = Wake::new(dir.clone());
        std::fs::write(dir.join("stop"), "").unwrap();
        let woken = within_a_second(&mut wake).await;
        assert_eq!(woken.map(|woken| woken.live), Some(false));
        let log = dir.join("engine.log");
        std::fs::write(&log, "").unwrap();
        within_a_second(&mut wake).await;
        std::fs::write(&log, "a line\n").unwrap();
        assert_eq!(within_a_second(&mut wake).await, None);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
