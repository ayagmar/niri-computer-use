//! The desk: whether this server controls the niri instance. It owns the lease, takes it
//! only when neither the stop flag nor the input-dirty marker is set, and gives it up as
//! soon as the stop flag appears. It also gates every action, one at a time, and cancels
//! the running one when the stop flag appears.

use std::sync::{Arc, PoisonError, Weak};
use std::time::Duration;

use serde::Serialize;
use tokio::sync::{Mutex, watch};

use super::lease::{self, Holder, Lease, Refused};
use super::marker;
use super::runtime::RuntimeDir;
use super::stop;
use crate::Env;
use crate::error::{CallError, ErrorName, ToolError};
use crate::refs::{Refs, Shot};

/// How often a held lease checks that its file is still the one at `lease`.
const CHECK: Duration = Duration::from_secs(1);

#[derive(Debug, Clone)]
pub(crate) struct Desk {
    /// Why there is no runtime directory, when there isn't one.
    runtime: Result<RuntimeDir, ToolError>,
    seat: Arc<Seat>,
    /// The stop watcher's view of the flag, or why the flag can't be watched.
    stopped: Result<watch::Receiver<bool>, String>,
}

/// The lease this server holds, if any.
#[derive(Debug)]
struct Seat {
    /// Also the action mutex: an action holds it while it runs.
    lease: Mutex<Option<Lease>>,
    /// The holder while the lease is held, which `status` reads without waiting for a
    /// running action. Changed only together with `lease`.
    holder: watch::Sender<Option<Holder>>,
    /// The screenshot refs of the lease held, which a screenshot adds to without waiting
    /// for a running action. Started and ended together with `lease`.
    refs: std::sync::Mutex<Refs>,
    /// The window that had keyboard focus when the lease was taken, to give it back to.
    /// Set and cleared together with `lease`.
    users_window: std::sync::Mutex<Option<u64>>,
}

impl Seat {
    fn put(&self, held: &mut Option<Lease>, lease: Lease, users_window: Option<u64>) {
        self.holder.send_replace(Some(lease.holder().clone()));
        self.refs().start();
        *self.users_window() = users_window;
        *held = Some(lease);
    }

    /// Gives the lease up. Returns whether it was held.
    fn take(&self, held: &mut Option<Lease>) -> bool {
        let had = held.take().is_some();
        self.holder.send_replace(None);
        self.refs().end();
        *self.users_window() = None;
        had
    }

    /// No code panics while holding the lock, so a poisoned one still holds a sound id.
    fn users_window(&self) -> std::sync::MutexGuard<'_, Option<u64>> {
        self.users_window
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// No code panics while holding the lock, so a poisoned one still holds sound refs.
    fn refs(&self) -> std::sync::MutexGuard<'_, Refs> {
        self.refs.lock().unwrap_or_else(PoisonError::into_inner)
    }
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
        let seat = Arc::new(Seat {
            lease: Mutex::new(None),
            holder: watch::Sender::new(None),
            refs: std::sync::Mutex::new(Refs::default()),
            users_window: std::sync::Mutex::new(None),
        });
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
                .inspect(|stopped| release_on_stop(stopped.clone(), Arc::downgrade(&seat)))
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
            seat,
            stopped,
        }
    }

    /// Takes the lease for `label`, or returns the holder if this server has it already.
    /// `refusal` is the policy's answer for the moment, checked after the stop flag and the
    /// input-dirty marker. `users_window`, the window with keyboard focus now, is kept with
    /// a lease newly taken.
    pub(crate) async fn acquire(
        &self,
        label: &str,
        refusal: Option<ToolError>,
        users_window: Option<u64>,
    ) -> Result<Holder, ToolError> {
        let runtime = self.runtime.as_ref().map_err(Clone::clone)?;
        let mut held = self.seat.lease.lock().await;
        if let Some(lease) = held.as_ref() {
            return Ok(lease.holder().clone());
        }
        self.unblocked(runtime)?;
        if let Some(refusal) = refusal {
            return Err(refusal);
        }
        let lease = Lease::acquire(runtime, label).map_err(refused)?;
        let holder = lease.holder().clone();
        self.seat.put(&mut held, lease, users_window);
        drop(held);
        Ok(holder)
    }

    /// Runs one action with the action mutex held. It runs only while neither the stop flag
    /// nor the input-dirty marker is set, this server holds the lease, and `refusal`, the
    /// policy's answer asked once those checks pass, has none. A stop that arrives while
    /// `work` runs cancels it; the stop watcher then takes the lease back. `finish` turns
    /// the work's result into the call's, still under the mutex but past the stop, so a
    /// stop can't discard what the work already found.
    pub(crate) async fn act<T, U, F>(
        &self,
        refusal: impl Future<Output = Option<ToolError>>,
        work: impl Future<Output = Result<T, CallError>>,
        finish: impl FnOnce(T) -> F,
    ) -> Result<U, CallError>
    where
        F: Future<Output = U>,
    {
        let runtime = self.runtime.as_ref().map_err(Clone::clone)?;
        let held = self.seat.lease.lock().await;
        let mut stopped = self.unblocked(runtime)?;
        if held.is_none() {
            return Err(ToolError::new(
                ErrorName::LeaseRequired,
                "this server doesn't hold the lease; call acquire_desktop first",
            )
            .into());
        }
        if let Some(refusal) = refusal.await {
            return Err(refusal.into());
        }
        let done = tokio::select! {
            biased;
            _ = stopped.wait_for(|stopped| *stopped) => None,
            done = work => Some(done),
        };
        let Some(done) = done else {
            return Err(cancelled(&stopped).into());
        };
        let finished = finish(done?).await;
        drop(held);
        Ok(finished)
    }

    /// Checks that a stop can reach this server and that neither the stop flag nor the
    /// input-dirty marker is set. Returns the watcher's view of the flag.
    fn unblocked(&self, runtime: &RuntimeDir) -> Result<watch::Receiver<bool>, ToolError> {
        // Without a live watcher a stop couldn't take the lease back.
        let stopped = self
            .stopped
            .as_ref()
            .map_err(|detail| ToolError::new(ErrorName::UpstreamError, detail.clone()))?;
        if stopped.has_changed().is_err() {
            return Err(watcher_ended());
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
        Ok(stopped.clone())
    }

    /// Gives the lease up, once any running action has ended. Returns whether this server
    /// held it.
    pub(crate) async fn release(&self) -> bool {
        self.seat.take(&mut *self.seat.lease.lock().await)
    }

    /// Serializes an observation with this server's actions and lease changes. It needs
    /// no lease and does not freeze external input or redraws. Action-return captures
    /// already hold the mutex and must not call this again.
    pub(crate) async fn observe<T>(&self, capture: impl Future<Output = T>) -> T {
        let held = self.seat.lease.lock().await;
        let result = capture.await;
        drop(held);
        result
    }

    /// The window that had keyboard focus when this server took the lease it holds.
    pub(crate) fn users_window(&self) -> Option<u64> {
        *self.seat.users_window()
    }

    /// The lease a screenshot starting now would issue its ref under, if this server holds
    /// one. Doesn't wait for a running action.
    pub(crate) fn ref_lease(&self) -> Option<u64> {
        self.seat.refs().lease()
    }

    /// Keeps `shot` as a ref of `lease` and returns its id, unless that lease has ended
    /// since the capture started.
    pub(crate) fn remember(&self, lease: u64, shot: Shot) -> Option<String> {
        self.seat.refs().insert(lease, shot)
    }

    /// The runtime directory, where input writes its marker.
    pub(crate) fn runtime(&self) -> Result<&RuntimeDir, ToolError> {
        self.runtime.as_ref().map_err(Clone::clone)
    }

    /// The ref named `id` of the lease held.
    pub(crate) fn shot(&self, id: &str) -> Result<Shot, ToolError> {
        self.seat.refs().get(id)
    }

    /// Doesn't wait for a running action.
    pub(crate) fn status(&self) -> LeaseStatus {
        let mine = self.seat.holder.borrow().clone();
        LeaseStatus {
            held_by_me: mine.is_some(),
            holder: mine.or_else(|| self.runtime.as_ref().ok().and_then(lease::holder)),
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
fn release_on_stop(stopped: watch::Receiver<bool>, seat: Weak<Seat>) {
    tokio::spawn(release_loop(stopped, seat));
}

async fn release_loop(mut stopped: watch::Receiver<bool>, seat: Weak<Seat>) {
    let mut check = tokio::time::interval(CHECK);
    check.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        if *stopped.borrow_and_update() && !give_up(&seat).await {
            return;
        }
        tokio::select! {
            changed = stopped.changed() => if changed.is_err() {
                give_up(&seat).await;
                return;
            },
            _ = check.tick() => if !give_up_if_moved(&seat).await {
                return;
            },
        }
    }
}

/// Drops the lease if its file was removed or replaced. Returns false once the desk is
/// gone.
async fn give_up_if_moved(seat: &Weak<Seat>) -> bool {
    let Some(seat) = seat.upgrade() else {
        return false;
    };
    let mut held = seat.lease.lock().await;
    if held.as_ref().is_some_and(|held| !held.intact()) {
        seat.take(&mut held);
    }
    drop(held);
    true
}

/// Drops the lease if it is held. Returns false once the desk is gone.
async fn give_up(seat: &Weak<Seat>) -> bool {
    let Some(seat) = seat.upgrade() else {
        return false;
    };
    seat.take(&mut *seat.lease.lock().await);
    true
}

/// Why a running action was cancelled. A watcher that loses the runtime directory reports
/// a stop and then ends, which on this single-threaded runtime happens before the action's
/// task runs again.
fn cancelled(stopped: &watch::Receiver<bool>) -> ToolError {
    if stopped.has_changed().is_err() {
        return watcher_ended();
    }
    ToolError::new(
        ErrorName::Stopped,
        "the user's stop flag cancelled this action; anything niri had already accepted may have taken effect",
    )
}

/// A stop can't reach this server any more.
fn watcher_ended() -> ToolError {
    ToolError::new(
        ErrorName::UpstreamError,
        "the stop watcher ended, because the runtime directory was removed or moved; restart the server",
    )
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
            if !desk.status().held_by_me {
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
        let holder = desk.acquire("me/1", None, None).await.unwrap();
        assert_eq!(desk.acquire("me/1", None, None).await.unwrap(), holder);
        let status = desk.status();
        assert!(status.held_by_me);
        assert_eq!(other.status().holder, Some(holder.clone()));
        let refused = other.acquire("other/2", None, None).await.unwrap_err();
        assert_eq!(refused.name, ErrorName::LeaseHeld);
        assert!(refused.detail.contains("(me/1)"), "{}", refused.detail);
        assert!(desk.release().await);
        assert!(!desk.release().await);
        assert_eq!(
            other.acquire("other/2", None, None).await.unwrap().label,
            "other/2"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn status_reports_the_lease_while_an_action_holds_the_mutex() {
        let dir = crate::test_support::fresh_dir("desk-busy");
        let desk = Desk::start(&env(&dir));
        let holder = desk.acquire("me/1", None, None).await.unwrap();
        let action = desk.seat.lease.lock().await;
        assert_eq!(
            desk.status(),
            LeaseStatus {
                held_by_me: true,
                holder: Some(holder)
            }
        );
        drop(action);
        assert!(desk.release().await);
        assert!(!desk.status().held_by_me);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn release_waits_for_capture_and_a_cancelled_capture_unlocks_the_desk() {
        let dir = crate::test_support::fresh_dir("desk-observe");
        let desk = Desk::start(&env(&dir));
        desk.acquire("me/1", None, None).await.unwrap();
        let (started, running) = tokio::sync::oneshot::channel();
        let capture_desk = desk.clone();
        let work = async move {
            started.send(()).unwrap();
            std::future::pending::<()>().await;
        };
        let capture = tokio::spawn(async move { capture_desk.observe(work).await });
        running.await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(20), desk.release())
                .await
                .is_err()
        );
        assert!(desk.status().held_by_me);
        capture.abort();
        assert!(capture.await.unwrap_err().is_cancelled());
        assert!(desk.release().await);
        // Observation is still available without a lease.
        assert_eq!(desk.observe(async { 7 }).await, 7);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// An action whose work records that it ran and returns `value`.
    async fn act(desk: &Desk, refusal: Option<ToolError>) -> Result<u8, ErrorName> {
        desk.act(async { refusal }, async { Ok(7) }, std::future::ready)
            .await
            .map_err(|error| match error {
                CallError::Tool(error) => error.name,
                CallError::InvalidArguments(message) => panic!("{message}"),
            })
    }

    #[tokio::test]
    async fn an_action_needs_the_lease_and_a_clear_gate() {
        let dir = crate::test_support::fresh_dir("desk-act");
        let desk = Desk::start(&env(&dir));
        let runtime = RuntimeDir::of(&env(&dir)).unwrap();
        assert_eq!(act(&desk, None).await, Err(ErrorName::LeaseRequired));
        desk.acquire("me/1", None, None).await.unwrap();
        assert_eq!(act(&desk, None).await, Ok(7));
        let locked = ToolError::new(ErrorName::ScreenLocked, "locked");
        assert_eq!(act(&desk, Some(locked)).await, Err(ErrorName::ScreenLocked));
        // The marker comes before the lease and the policy, so an agent is told to stop.
        std::fs::write(runtime.path().join("input-dirty"), "").unwrap();
        desk.release().await;
        assert_eq!(act(&desk, None).await, Err(ErrorName::RecoveryRequired));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn a_stop_cancels_the_running_action_then_takes_the_lease_back() {
        let dir = crate::test_support::fresh_dir("desk-act-stop");
        let desk = Desk::start(&env(&dir));
        let runtime = RuntimeDir::of(&env(&dir)).unwrap();
        desk.acquire("me/1", None, None).await.unwrap();
        let (started, running) = tokio::sync::oneshot::channel::<()>();
        let (held, dropped) = tokio::sync::oneshot::channel::<()>();
        let action = desk.act(
            async { None },
            async move {
                started.send(()).unwrap();
                let _held = held;
                std::future::pending::<Result<(), CallError>>().await
            },
            std::future::ready,
        );
        let stop = async {
            running.await.unwrap();
            runtime.stop().unwrap();
        };
        let (result, ()) =
            tokio::time::timeout(Duration::from_secs(5), async { tokio::join!(action, stop) })
                .await
                .unwrap();
        assert!(matches!(
            result,
            Err(CallError::Tool(ToolError {
                name: ErrorName::Stopped,
                ..
            }))
        ));
        // The work was dropped, and with it its sender.
        assert!(dropped.await.is_err());
        assert!(released(&desk).await);
        assert_eq!(act(&desk, None).await, Err(ErrorName::Stopped));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn losing_the_stop_watcher_mid_action_says_to_restart() {
        let dir = crate::test_support::fresh_dir("desk-act-gone");
        let desk = Desk::start(&env(&dir));
        let runtime = RuntimeDir::of(&env(&dir)).unwrap();
        desk.acquire("me/1", None, None).await.unwrap();
        let (started, running) = tokio::sync::oneshot::channel::<()>();
        let action = desk.act(
            async { None },
            async move {
                started.send(()).unwrap();
                std::future::pending::<Result<(), CallError>>().await
            },
            std::future::ready,
        );
        let remove = async {
            running.await.unwrap();
            std::fs::remove_dir_all(runtime.path()).unwrap();
        };
        let (result, ()) = tokio::time::timeout(Duration::from_secs(5), async {
            tokio::join!(action, remove)
        })
        .await
        .unwrap();
        let Err(CallError::Tool(error)) = result else {
            panic!("{result:?}");
        };
        assert_eq!(error.name, ErrorName::UpstreamError);
        assert!(error.detail.contains("restart the server"), "{error:?}");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn refuses_while_stopped_or_dirty_and_a_stop_takes_the_lease_back() {
        let dir = crate::test_support::fresh_dir("desk-stop");
        let desk = Desk::start(&env(&dir));
        let runtime = RuntimeDir::of(&env(&dir)).unwrap();
        desk.acquire("me/1", None, None).await.unwrap();
        runtime.stop().unwrap();
        assert!(released(&desk).await);
        assert_eq!(lease::holder(&runtime), None);
        assert_eq!(
            desk.acquire("me/1", None, None).await.unwrap_err().name,
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
            desk.acquire("me/1", None, None).await.unwrap_err().name,
            ErrorName::RecoveryRequired
        );
        std::fs::remove_file(runtime.path().join("input-dirty")).unwrap();
        desk.acquire("me/1", None, None).await.unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn removing_the_runtime_directory_gives_the_lease_up_for_good() {
        let dir = crate::test_support::fresh_dir("desk-removed");
        let desk = Desk::start(&env(&dir));
        let runtime = RuntimeDir::of(&env(&dir)).unwrap();
        desk.acquire("me/1", None, None).await.unwrap();
        std::fs::remove_dir_all(runtime.path()).unwrap();
        assert!(released(&desk).await);
        // A new directory has a new lock file, which no stop could reach for this server.
        let error = desk.acquire("me/1", None, None).await.unwrap_err();
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
        let error = desk.acquire("me/1", None, None).await.unwrap_err();
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
        let error = desk.acquire("me/1", None, None).await.unwrap_err();
        assert_eq!(error.name, ErrorName::NiriUnavailable);
        assert_eq!(error.detail, "NIRI_SOCKET is not set");
        assert_eq!(
            desk.status(),
            LeaseStatus {
                held_by_me: false,
                holder: None
            }
        );
    }
}
