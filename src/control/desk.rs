//! The desk: whether this server controls the niri instance. It owns the lease, takes it
//! only when neither the stop flag nor the input-dirty marker is set, and gives it up as
//! soon as the stop flag appears.

use std::sync::{Arc, Weak};
use std::time::Duration;

use serde::Serialize;
use tokio::sync::{Mutex, watch};

use super::lease::{self, Holder, Lease, Refused};
use super::marker;
use super::runtime::RuntimeDir;
use super::stop;
use crate::Env;
use crate::error::{ErrorName, ToolError};

/// How often a held lease checks that its file is still the one at `lease`.
const CHECK: Duration = Duration::from_secs(1);

#[derive(Debug, Clone)]
pub(crate) struct Desk {
    /// Why there is no runtime directory, when there isn't one.
    runtime: Result<RuntimeDir, ToolError>,
    /// Also the action mutex: an action holds it while it runs.
    lease: Arc<Mutex<Option<Lease>>>,
    /// The stop watcher's view of the flag, or why the flag can't be watched.
    stopped: Result<watch::Receiver<bool>, String>,
}

/// What `status` reports about the lease.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct LeaseStatus {
    pub(crate) held_by_me: bool,
    pub(crate) holder: Option<Holder>,
}

impl Desk {
    /// Starts watching the stop flag on the current Tokio runtime, when `env` names a niri
    /// instance.
    pub(crate) fn start(env: &Env) -> Self {
        let lease = Arc::new(Mutex::new(None));
        let runtime = RuntimeDir::of(env).map_err(|detail| {
            let name = if env.niri_socket.is_none() {
                ErrorName::NiriUnavailable
            } else {
                ErrorName::UpstreamError
            };
            ToolError::new(name, detail)
        });
        let stopped = match &runtime {
            Ok(runtime) => stop::watch(runtime.clone())
                .inspect(|stopped| release_on_stop(stopped.clone(), Arc::downgrade(&lease)))
                .map_err(|error| {
                    format!(
                        "watch {} for the stop flag: {error}",
                        runtime.path().display()
                    )
                }),
            Err(error) => Err(error.detail.clone()),
        };
        Self {
            runtime,
            lease,
            stopped,
        }
    }

    /// Takes the lease for `label`, or returns the holder if this server has it already.
    pub(crate) async fn acquire(&self, label: &str) -> Result<Holder, ToolError> {
        let runtime = self.runtime.as_ref().map_err(Clone::clone)?;
        let mut held = self.lease.lock().await;
        if let Some(lease) = held.as_ref() {
            return Ok(lease.holder().clone());
        }
        // Without a live watcher a stop couldn't take the lease back.
        let stopped = self
            .stopped
            .as_ref()
            .map_err(|detail| ToolError::new(ErrorName::UpstreamError, detail.clone()))?;
        if stopped.has_changed().is_err() {
            return Err(ToolError::new(
                ErrorName::UpstreamError,
                "the stop watcher ended, because the runtime directory was removed or moved; restart the server",
            ));
        }
        if *stopped.borrow() || runtime.stopped().map_err(|error| unreadable(&error))? {
            return Err(ToolError::new(
                ErrorName::Stopped,
                "the stop flag is set; the user clears it with `niri-computer-use resume`",
            ));
        }
        if runtime.input_dirty().map_err(|error| unreadable(&error))? {
            let marker = marker::read(runtime).map_or_else(String::new, |found| found.summary());
            return Err(ToolError::new(
                ErrorName::RecoveryRequired,
                format!("input may be stuck ({marker}); the user runs `niri-computer-use recover`"),
            ));
        }
        let lease = Lease::acquire(runtime, label).map_err(refused)?;
        let holder = lease.holder().clone();
        *held = Some(lease);
        drop(held);
        Ok(holder)
    }

    /// Gives the lease up. Returns whether this server held it.
    pub(crate) async fn release(&self) -> bool {
        self.lease.lock().await.take().is_some()
    }

    pub(crate) async fn status(&self) -> LeaseStatus {
        let held = self.lease.lock().await;
        LeaseStatus {
            held_by_me: held.is_some(),
            holder: held
                .as_ref()
                .map(|lease| lease.holder().clone())
                .or_else(|| self.runtime.as_ref().ok().and_then(lease::holder)),
        }
    }
}

/// What `status` reports when there is no desk, as in the `status` subcommand.
pub(crate) fn status_without_desk(env: &Env) -> LeaseStatus {
    LeaseStatus {
        held_by_me: false,
        holder: RuntimeDir::of(env)
            .ok()
            .and_then(|runtime| lease::holder(&runtime)),
    }
}

/// Drops the lease whenever the flag is seen set, and when the watcher ends, until the desk
/// is gone. It reads the latest value on every change, so a stop is never lost between a
/// resume and the next stop.
fn release_on_stop(stopped: watch::Receiver<bool>, lease: Weak<Mutex<Option<Lease>>>) {
    tokio::spawn(release_loop(stopped, lease));
}

async fn release_loop(mut stopped: watch::Receiver<bool>, lease: Weak<Mutex<Option<Lease>>>) {
    let mut check = tokio::time::interval(CHECK);
    check.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        if *stopped.borrow_and_update() && !give_up(&lease).await {
            return;
        }
        tokio::select! {
            changed = stopped.changed() => if changed.is_err() {
                give_up(&lease).await;
                return;
            },
            _ = check.tick() => if !give_up_if_moved(&lease).await {
                return;
            },
        }
    }
}

/// Drops the lease if its file was removed or replaced. Returns false once the desk is
/// gone.
async fn give_up_if_moved(lease: &Weak<Mutex<Option<Lease>>>) -> bool {
    let Some(lease) = lease.upgrade() else {
        return false;
    };
    let mut held = lease.lock().await;
    if held.as_ref().is_some_and(|held| !held.intact()) {
        held.take();
    }
    true
}

/// Drops the lease if it is held. Returns false once the desk is gone.
async fn give_up(lease: &Weak<Mutex<Option<Lease>>>) -> bool {
    let Some(lease) = lease.upgrade() else {
        return false;
    };
    lease.lock().await.take();
    true
}

/// The runtime directory can't be read, so neither flag can be ruled out.
fn unreadable(error: &std::io::Error) -> ToolError {
    ToolError::new(
        ErrorName::UpstreamError,
        format!("read the runtime directory: {error}"),
    )
}

fn refused(refused: Refused) -> ToolError {
    match refused {
        Refused::Held(Some(holder)) => ToolError::new(
            ErrorName::LeaseHeld,
            format!(
                "held by PID {} ({}) since {}",
                holder.pid, holder.label, holder.since
            ),
        ),
        Refused::Held(None) => ToolError::new(
            ErrorName::LeaseHeld,
            "held by another process that left no record",
        ),
        Refused::Io(detail) => ToolError::new(ErrorName::UpstreamError, detail),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn env(dir: &std::path::Path) -> Env {
        Env {
            niri_socket: Some(dir.join("niri.test.sock")),
            runtime_dir: Some(dir.to_path_buf()),
            ..Env::default()
        }
    }

    /// Whether the desk gives the lease up within five seconds.
    async fn released(desk: &Desk) -> bool {
        for _ in 0..500 {
            if !desk.status().await.held_by_me {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        false
    }

    #[tokio::test]
    async fn takes_keeps_and_gives_back_the_lease() {
        let dir = crate::test_support::fresh_dir("desk");
        let desk = Desk::start(&env(&dir));
        let other = Desk::start(&env(&dir));
        let holder = desk.acquire("me/1").await.unwrap();
        assert_eq!(desk.acquire("me/1").await.unwrap(), holder);
        let status = desk.status().await;
        assert!(status.held_by_me);
        assert_eq!(other.status().await.holder, Some(holder.clone()));
        let refused = other.acquire("other/2").await.unwrap_err();
        assert_eq!(refused.name, ErrorName::LeaseHeld);
        assert!(refused.detail.contains("(me/1)"), "{}", refused.detail);
        assert!(desk.release().await);
        assert!(!desk.release().await);
        assert_eq!(other.acquire("other/2").await.unwrap().label, "other/2");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn refuses_while_stopped_or_dirty_and_a_stop_takes_the_lease_back() {
        let dir = crate::test_support::fresh_dir("desk-stop");
        let desk = Desk::start(&env(&dir));
        let runtime = RuntimeDir::of(&env(&dir)).unwrap();
        desk.acquire("me/1").await.unwrap();
        runtime.stop().unwrap();
        assert!(released(&desk).await);
        assert_eq!(lease::holder(&runtime), None);
        assert_eq!(
            desk.acquire("me/1").await.unwrap_err().name,
            ErrorName::Stopped
        );
        runtime.resume().unwrap();
        // Until the watcher has seen the resume, its view still refuses the lease.
        let mut seen = desk.stopped.clone().unwrap();
        tokio::time::timeout(Duration::from_secs(5), seen.wait_for(|stopped| !*stopped))
            .await
            .unwrap()
            .unwrap();
        std::fs::write(runtime.path().join("input-dirty"), "").unwrap();
        assert_eq!(
            desk.acquire("me/1").await.unwrap_err().name,
            ErrorName::RecoveryRequired
        );
        std::fs::remove_file(runtime.path().join("input-dirty")).unwrap();
        desk.acquire("me/1").await.unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn removing_the_runtime_directory_gives_the_lease_up_for_good() {
        let dir = crate::test_support::fresh_dir("desk-removed");
        let desk = Desk::start(&env(&dir));
        let runtime = RuntimeDir::of(&env(&dir)).unwrap();
        desk.acquire("me/1").await.unwrap();
        std::fs::remove_dir_all(runtime.path()).unwrap();
        assert!(released(&desk).await);
        // A new directory has a new lock file, which no stop could reach for this server.
        let error = desk.acquire("me/1").await.unwrap_err();
        assert_eq!(error.name, ErrorName::UpstreamError);
        assert!(
            error.detail.contains("restart the server"),
            "{}",
            error.detail
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn an_unreadable_runtime_directory_refuses_the_lease() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = crate::test_support::fresh_dir("desk-unreadable");
        let runtime = RuntimeDir::of(&env(&dir)).unwrap();
        runtime.create().unwrap();
        let desk = Desk::start(&env(&dir));
        let mode = |bits| std::fs::Permissions::from_mode(bits);
        std::fs::set_permissions(runtime.path(), mode(0o000)).unwrap();
        let error = desk.acquire("me/1").await.unwrap_err();
        std::fs::set_permissions(runtime.path(), mode(0o700)).unwrap();
        assert_eq!(error.name, ErrorName::UpstreamError);
        assert!(
            error.detail.starts_with("read the runtime directory"),
            "{}",
            error.detail
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn without_a_niri_instance_there_is_nothing_to_take() {
        let desk = Desk::start(&Env::default());
        let error = desk.acquire("me/1").await.unwrap_err();
        assert_eq!(error.name, ErrorName::NiriUnavailable);
        assert_eq!(error.detail, "NIRI_SOCKET is not set");
        assert_eq!(
            desk.status().await,
            LeaseStatus {
                held_by_me: false,
                holder: None
            }
        );
    }
}
