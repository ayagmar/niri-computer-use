//! `desk guard <supervisor pid> <name> <generation>`: what takes a desk down when its
//! supervisor dies, however it dies. The supervisor starts it in the desk's scope before it
//! starts anything else. It makes a session of its own, opens a pidfd on the supervisor,
//! its parent, and waits for it to exit; then it asks systemd to stop the scope, which
//! ends everything left in it, the guard as well. Once the supervisor has stopped the desk
//! in order, only the guard is left, so that stop ends nothing else; and a `desk stop`'s
//! own stop of the same unit joins the same job.

use std::os::fd::OwnedFd;
use std::time::Duration;

use rustix::event::{PollFd, PollFlags, poll};
use rustix::process::{Pid, PidfdFlags, getppid, pidfd_open};
use tokio::time::Instant;

use super::name::DeskName;
use super::scope::{self, Generation};

/// How long the request to stop the scope may take; the stop itself runs on in systemd.
const STOP: Duration = Duration::from_secs(10);

/// Waits for the supervisor `supervisor` to exit, then stops the scope of the start
/// `generation` of desk `name`.
pub(crate) async fn run(
    supervisor: u32,
    name: &DeskName,
    generation: &Generation,
) -> Result<(), String> {
    rustix::process::setsid().map_err(|error| format!("make the guard's session: {error}"))?;
    let unit = scope::unit(name, generation);
    if let Some(pidfd) = watch(supervisor)? {
        // No deadline: the guard exists to wait for the supervisor's end, which comes when
        // the desk stops, days later or never. It holds nothing meanwhile.
        exits(pidfd).await?;
    }
    scope::request_stop(&unit, Instant::now() + STOP)
        .await
        .map_err(|error| format!("stop {unit} after its supervisor {supervisor} exited: {error}"))
}

/// A pidfd on the supervisor, or `None` when it has already exited: then the parent the
/// kernel gives the guard is another process, which a pidfd opened by PID alone could name.
fn watch(supervisor: u32) -> Result<Option<OwnedFd>, String> {
    let pid = i32::try_from(supervisor)
        .ok()
        .and_then(Pid::from_raw)
        .ok_or_else(|| format!("{supervisor} isn't a process ID"))?;
    let opened = pidfd_open(pid, PidfdFlags::empty());
    if !is_parent(supervisor, getppid()) {
        return Ok(None);
    }
    // Still the guard's parent after the open, so the pidfd is the supervisor's.
    opened
        .map(Some)
        .map_err(|error| format!("pidfd_open {supervisor}: {error}"))
}

/// Whether the guard's parent, `parent` as `getppid` gave it, is still `supervisor`.
fn is_parent(supervisor: u32, parent: Option<Pid>) -> bool {
    parent.is_some_and(|pid| pid.as_raw_nonzero().get().cast_unsigned() == supervisor)
}

/// Waits until the process `pidfd` names has exited: the pidfd turns readable. The wait
/// blocks in `poll` on a thread of its own; tokio's safe ways to watch a descriptor are
/// deprecated as unsound, and the one left takes `unsafe`.
async fn exits(pidfd: OwnedFd) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        let mut fds = [PollFd::new(&pidfd, PollFlags::IN)];
        loop {
            match poll(&mut fds, None) {
                Ok(_) => return Ok(()),
                Err(rustix::io::Errno::INTR) => {}
                Err(error) => return Err(format!("wait for the supervisor's exit: {error}")),
            }
        }
    })
    .await
    .map_err(|error| format!("wait for the supervisor's exit: {error}"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Catches a guard that, its supervisor gone before it looked, takes the process the
    /// kernel reparented it to for the supervisor and waits on it forever.
    #[test]
    fn only_the_supervisor_still_its_parent_is_watched() {
        let pid = |raw| Pid::from_raw(raw);
        assert!(is_parent(4242, pid(4242)));
        assert!(!is_parent(4242, pid(1)));
        assert!(!is_parent(4242, None));
        assert!(watch(u32::MAX).is_err());
        // The test's own parent isn't this process, so there is nothing to watch.
        assert_eq!(watch(std::process::id()).map(|fd| fd.is_some()), Ok(false));
    }

    /// Catches a wait that returns while the watched process runs, or never returns once it
    /// has exited.
    #[tokio::test]
    async fn the_wait_ends_when_the_process_exits() {
        let (child, pid) = crate::runner::desk::sleeper();
        let pidfd = pidfd_open(pid, PidfdFlags::empty()).unwrap();
        let mut waiting = std::pin::pin!(exits(pidfd));
        assert!(
            tokio::time::timeout(Duration::from_millis(200), &mut waiting)
                .await
                .is_err(),
            "the wait ended while the process ran"
        );
        drop(child);
        tokio::time::timeout(Duration::from_secs(5), waiting)
            .await
            .unwrap()
            .unwrap();
    }
}
