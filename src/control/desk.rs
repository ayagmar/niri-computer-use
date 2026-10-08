//! The desk: whether this server controls the niri instance. It owns the lease, takes it
//! only when neither the stop flag nor the input-dirty marker is set, and gives it up as
//! soon as the stop flag appears.

use std::sync::{Arc, Weak};

use serde::Serialize;
use tokio::sync::{Mutex, watch};

use super::lease::{self, Holder, Lease, Refused};
use super::runtime::RuntimeDir;
use super::stop;
use crate::Env;
use crate::error::{ErrorName, ToolError};

#[derive(Debug, Clone)]
pub(crate) struct Desk {
    /// Why there is no runtime directory, when there isn't one.
    runtime: Result<RuntimeDir, ToolError>,
    /// Also the action mutex: an action holds it while it runs.
    lease: Arc<Mutex<Option<Lease>>>,
    /// Why the stop flag can't be watched, when it can't.
    watching: Result<(), String>,
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
        let watching = match &runtime {
            Ok(runtime) => stop::watch(runtime.clone())
                .map(|stopped| release_on_stop(stopped, Arc::downgrade(&lease)))
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
            watching,
        }
    }

    /// Takes the lease for `label`, or returns the holder if this server has it already.
    pub(crate) async fn acquire(&self, label: &str) -> Result<Holder, ToolError> {
        let runtime = self.runtime.as_ref().map_err(Clone::clone)?;
        let mut held = self.lease.lock().await;
        if let Some(lease) = held.as_ref() {
            return Ok(lease.holder().clone());
        }
        if let Err(detail) = &self.watching {
            // Without the watcher a stop couldn't take the lease back.
            return Err(ToolError::new(ErrorName::UpstreamError, detail.clone()));
        }
        if runtime.stopped().map_err(|error| unreadable(&error))? {
            return Err(ToolError::new(
                ErrorName::Stopped,
                "the stop flag is set; the user clears it with `niri-computer-use resume`",
            ));
        }
        if runtime.input_dirty().map_err(|error| unreadable(&error))? {
            return Err(ToolError::new(
                ErrorName::RecoveryRequired,
                "input may be stuck; the user runs `niri-computer-use recover`",
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

/// Drops the lease every time the stop flag appears, until the desk is gone.
fn release_on_stop(mut stopped: watch::Receiver<bool>, lease: Weak<Mutex<Option<Lease>>>) {
    tokio::spawn(async move {
        while stopped.wait_for(|stopped| *stopped).await.is_ok() {
            let Some(lease) = lease.upgrade() else { return };
            lease.lock().await.take();
            drop(lease);
            if stopped.wait_for(|stopped| !*stopped).await.is_err() {
                return;
            }
        }
    });
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
