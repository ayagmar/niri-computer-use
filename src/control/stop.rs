//! Watches the runtime directory with inotify and reports whether the stop flag is set.
//! Any change in the directory triggers a fresh check of the flag itself, so the watcher
//! never has to interpret event names or order.

use std::io;

use rustix::fs::inotify;
use rustix::io::Errno;
use tokio::io::unix::AsyncFd;
use tokio::sync::watch;

use super::runtime::RuntimeDir;

/// Starts watching on the current Tokio runtime. The receiver's value is whether the stop
/// flag is set; the task ends once every receiver is gone.
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
            | inotify::WatchFlags::ONLYDIR,
    )?;
    let fd = AsyncFd::new(fd)?;
    // Checked after the watch exists, so a flag created in between isn't missed.
    let (sender, receiver) = watch::channel(flag_set(&runtime));
    tokio::spawn(async move {
        loop {
            tokio::select! {
                () = sender.closed() => return,
                ready = fd.readable() => {
                    let Ok(mut guard) = ready else { return };
                    if drain(guard.get_inner()).is_err() {
                        return;
                    }
                    guard.clear_ready();
                }
            }
            sender.send_if_modified(|stopped| {
                let now = flag_set(&runtime);
                let changed = *stopped != now;
                *stopped = now;
                changed
            });
        }
    });
    Ok(receiver)
}

/// The flag, counting a directory that can't be read as stopped, so the lease is given up.
fn flag_set(runtime: &RuntimeDir) -> bool {
    runtime.stopped().unwrap_or(true)
}

/// Reads every queued event. Their contents don't matter.
fn drain(fd: &rustix::fd::OwnedFd) -> io::Result<()> {
    let mut buffer = [std::mem::MaybeUninit::uninit(); 4096];
    let mut reader = inotify::Reader::new(fd, &mut buffer);
    loop {
        match reader.next() {
            Ok(_) => {}
            Err(Errno::AGAIN) => return Ok(()),
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

    #[tokio::test]
    async fn follows_the_flag_as_stop_and_resume_change_it() {
        let dir = crate::test_support::fresh_dir("stop-watch");
        let runtime = RuntimeDir::of(&Env {
            niri_socket: Some(dir.join("niri.test.sock")),
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
    async fn a_flag_set_before_watching_is_seen() {
        let dir = crate::test_support::fresh_dir("stop-watch-early");
        let runtime = RuntimeDir::of(&Env {
            niri_socket: Some(dir.join("niri.test.sock")),
            runtime_dir: Some(dir.clone()),
            ..Env::default()
        })
        .unwrap();
        runtime.stop().unwrap();
        assert!(*watch(runtime).unwrap().borrow());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
