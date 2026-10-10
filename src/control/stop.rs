//! Watches the runtime directory with inotify and reports whether the stop flag is set.
//! Any change in the directory triggers a fresh check of the flag itself, so the watcher
//! never has to interpret event names or order.

use std::io;
use std::os::unix::fs::MetadataExt as _;
use std::path::Path;
use std::time::Duration;

use rustix::fs::inotify;
use rustix::io::Errno;
use tokio::io::unix::AsyncFd;
use tokio::sync::watch;

use super::runtime::RuntimeDir;

/// How often the directory's identity is checked besides on events. The kernel delays the
/// directory's own deletion event while a file inside it is open, as the held lease is.
const CHECK: Duration = Duration::from_secs(1);

/// Starts watching on the current Tokio runtime. The receiver's value is whether the stop
/// flag is set. If the directory itself is removed, moved or replaced, which shows as its
/// path no longer naming the watched inode, the watch can't follow it:
/// the value becomes `true` and the task ends, closing the channel, so a holder gives the
/// lease up and nobody takes one that no stop could reach. The task also ends once every
/// receiver is gone.
pub(crate) fn watch(runtime: RuntimeDir) -> io::Result<watch::Receiver<bool>> {
    runtime.create()?;
    let fd = inotify::init(inotify::CreateFlags::NONBLOCK | inotify::CreateFlags::CLOEXEC)?;
    inotify::add_watch(
        &fd,
        runtime.path(),
        inotify::WatchFlags::CREATE
            | inotify::WatchFlags::DELETE
            | inotify::WatchFlags::MOVED_TO
            | inotify::WatchFlags::MOVED_FROM
            | inotify::WatchFlags::DELETE_SELF
            | inotify::WatchFlags::MOVE_SELF
            | inotify::WatchFlags::ONLYDIR,
    )?;
    let fd = AsyncFd::new(fd)?;
    let watched = identity(runtime.path())?;
    // Checked after the watch exists, so a flag created in between isn't missed.
    let (sender, receiver) = watch::channel(flag_set(&runtime));
    let mut check = tokio::time::interval(CHECK);
    check.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    tokio::spawn(async move {
        loop {
            tokio::select! {
                () = sender.closed() => return,
                _ = check.tick() => {}
                ready = fd.readable() => {
                    let Ok(mut guard) = ready else { break };
                    match drain(guard.get_inner()) {
                        Ok(Watch::Kept) => guard.clear_ready(),
                        Ok(Watch::Lost) | Err(_) => break,
                    }
                }
            }
            if identity(runtime.path()).ok() != Some(watched) {
                break;
            }
            sender.send_if_modified(|stopped| {
                let now = flag_set(&runtime);
                let changed = *stopped != now;
                *stopped = now;
                changed
            });
        }
        sender.send_replace(true);
    });
    Ok(receiver)
}

/// The flag, counting a directory that can't be read as stopped, so the lease is given up.
fn flag_set(runtime: &RuntimeDir) -> bool {
    runtime.stopped().unwrap_or(true)
}

/// The device and inode a path names now.
fn identity(path: &Path) -> io::Result<(u64, u64)> {
    let meta = std::fs::metadata(path)?;
    Ok((meta.dev(), meta.ino()))
}

/// Whether the directory is still watched after a batch of events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Watch {
    Kept,
    /// The directory was removed or moved away.
    Lost,
}

/// Reads every queued event. Only whether the directory itself went away matters.
fn drain(fd: &rustix::fd::OwnedFd) -> io::Result<Watch> {
    let gone = inotify::ReadFlags::DELETE_SELF
        | inotify::ReadFlags::MOVE_SELF
        | inotify::ReadFlags::IGNORED;
    let mut buffer = [std::mem::MaybeUninit::uninit(); 4096];
    let mut reader = inotify::Reader::new(fd, &mut buffer);
    let mut watch = Watch::Kept;
    loop {
        match reader.next() {
            Ok(event) if event.events().intersects(gone) => watch = Watch::Lost,
            Ok(_) => {}
            Err(Errno::AGAIN) => return Ok(watch),
            Err(error) => return Err(error.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::Env;

    /// Whether the watcher reports `want` within five seconds.
    async fn settles(stopped: &watch::Receiver<bool>, want: bool) -> bool {
        let mut stopped = stopped.clone();
        tokio::time::timeout(Duration::from_secs(5), stopped.wait_for(|s| *s == want))
            .await
            .is_ok()
    }

    /// Returns once the watcher has closed the channel.
    async fn closed(stopped: &mut watch::Receiver<bool>) {
        while stopped.changed().await.is_ok() {}
    }

    #[tokio::test]
    async fn follows_the_flag_as_stop_and_resume_change_it() {
        let dir = crate::test_support::fresh_dir("stop-watch");
        let runtime = RuntimeDir::of(&Env {
            niri_socket: crate::niri::Socket::at(dir.join("niri.test.sock")),
            runtime_dir: Some(dir.clone()),
            ..Env::default()
        })
        .unwrap();
        let stopped = watch(runtime.clone()).unwrap();
        assert!(!*stopped.borrow());
        runtime.stop().unwrap();
        assert!(settles(&stopped, true).await);
        // Unrelated files don't change the answer.
        std::fs::write(runtime.path().join("other"), "").unwrap();
        runtime.resume().unwrap();
        assert!(settles(&stopped, false).await);
        runtime.stop().unwrap();
        assert!(settles(&stopped, true).await);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn removing_the_directory_counts_as_stopped_and_ends_the_watch() {
        let dir = crate::test_support::fresh_dir("stop-watch-gone");
        let runtime = RuntimeDir::of(&Env {
            niri_socket: crate::niri::Socket::at(dir.join("niri.test.sock")),
            runtime_dir: Some(dir.clone()),
            ..Env::default()
        })
        .unwrap();
        let mut stopped = watch(runtime.clone()).unwrap();
        std::fs::remove_dir_all(runtime.path()).unwrap();
        assert!(settles(&stopped, true).await);
        // The task ended, so the channel is closed.
        assert!(
            tokio::time::timeout(Duration::from_secs(5), closed(&mut stopped))
                .await
                .is_ok()
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn replacing_the_directory_ends_the_watch() {
        let dir = crate::test_support::fresh_dir("stop-watch-replaced");
        let runtime = RuntimeDir::of(&Env {
            niri_socket: crate::niri::Socket::at(dir.join("niri.test.sock")),
            runtime_dir: Some(dir.clone()),
            ..Env::default()
        })
        .unwrap();
        let mut stopped = watch(runtime.clone()).unwrap();
        // An open file inside delays the kernel's deletion event, as the held lease does.
        let pinned = std::fs::File::create(runtime.path().join("pin")).unwrap();
        std::fs::remove_dir_all(runtime.path()).unwrap();
        runtime.create().unwrap();
        assert!(settles(&stopped, true).await);
        assert!(
            tokio::time::timeout(Duration::from_secs(5), closed(&mut stopped))
                .await
                .is_ok()
        );
        drop(pinned);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn a_flag_set_before_watching_is_seen() {
        let dir = crate::test_support::fresh_dir("stop-watch-early");
        let runtime = RuntimeDir::of(&Env {
            niri_socket: crate::niri::Socket::at(dir.join("niri.test.sock")),
            runtime_dir: Some(dir.clone()),
            ..Env::default()
        })
        .unwrap();
        runtime.stop().unwrap();
        assert!(*watch(runtime).unwrap().borrow());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
