//! The desk: whether a session of this server controls the niri instance. It owns the
//! lease, takes it for one session at a time, only when neither the stop flag nor the
//! input-dirty marker is set, and gives it up as soon as the stop flag appears. The lease,
//! its refs and the window to give focus back to belong to the session that took it: no
//! other session can act, release, or use them. It also gates every action, one at a time,
//! and cancels the running one when the stop flag appears. A session that ends gives the
//! lease up at once: its running action is dropped in whatever phase it is.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, PoisonError, Weak};
use std::time::Duration;

use serde::Serialize;
use tokio::sync::{Mutex, watch};
use tokio::time::Instant;

use super::lease::{self, Holder, Lease, Refused, Unpublished};
use super::runtime::RuntimeDir;
use super::stop;
use super::{marker, procs};
use crate::Env;
use crate::a11y::ElementRef;
use crate::act::Outcome;
use crate::error::{CallError, ErrorName, ToolError};
use crate::refs::{Refs, Shot};
use crate::session::{Session, SessionId};

/// How often a held lease checks that its file is still the one at `lease`.
const CHECK: Duration = Duration::from_secs(1);
/// How often a running action checks the same.
const MOVED_CHECK: Duration = Duration::from_millis(100);
/// How long `Desk::close` waits for the action mutex, which the lease's periodic check
/// and a session's end hold only briefly.
const CLOSE_WAIT: Duration = Duration::from_millis(100);
/// The oldest an answer the action gate relies on may be when the call that acts goes out:
/// the lock state for every action, and for an element action every answer its last gate
/// read. The gate takes 8 to 15 ms in the protocol tests; niri, Noctalia, logind or an app
/// that takes this long is busy, and what it said may have changed since (see
/// docs/decisions.md).
pub(crate) const LAST_GATE: Duration = Duration::from_millis(250);

/// `error` from `Desk::fresh` once part of a call went out. Its `lock_stale` detail tells
/// the agent to try once more, which is right only while nothing went out; here something
/// did, and repeating the call could repeat what that part did.
pub(crate) fn partway(error: ToolError) -> ToolError {
    if error.name != ErrorName::LockStale {
        return error;
    }
    ToolError::new(
        ErrorName::LockStale,
        format!(
            "the lock state was last read over {} ms before the call's next part could go out, so it isn't known to be unlocked now",
            LAST_GATE.as_millis()
        ),
    )
}

#[derive(Debug, Clone)]
pub(crate) struct Desk {
    /// Why there is no runtime directory, when there isn't one.
    runtime: Result<RuntimeDir, ToolError>,
    seat: Arc<Seat>,
    /// The stop watcher's view of the flag, or why the flag can't be watched.
    stopped: Result<watch::Receiver<bool>, String>,
}

/// The lease a session holds, if any.
#[derive(Debug)]
struct Seat {
    /// Also the action mutex: an action holds it while it runs.
    lease: Mutex<Option<Grant>>,
    /// The grant's owner while the lease is held, which `status` and the checks before an
    /// action read without waiting for a running action. Changed only together with
    /// `lease`.
    owner: watch::Sender<Option<Owner>>,
    /// The refs of the lease held, which a screenshot adds to without waiting for a running
    /// action. Started and ended together with `lease`.
    refs: std::sync::Mutex<Refs>,
    /// Whether the running action's one call that acts may have gone out
    /// (`Desk::sending`). Cleared when an action starts, under the action mutex.
    sending: AtomicBool,
    /// The running action's outcome as far as it is known once the app answered that
    /// call (`Desk::answered`). Cleared with `sending`.
    answered: std::sync::Mutex<Option<Outcome>>,
    /// When the latest read of the lock state began (`Desk::lock_read`). Cleared when an
    /// action starts, before its readiness report.
    lock_read: std::sync::Mutex<Option<Instant>>,
    /// The session taking the lease under the action mutex, with no grant yet, while it
    /// waits for the policy's answer and the user's.
    taking: std::sync::Mutex<Option<SessionId>>,
    /// Set under the action mutex by `Desk::close`, for good: no lease is granted again.
    closed: AtomicBool,
    /// Whether the engine exits, set once a `Closing` is dropped.
    exit: watch::Sender<bool>,
}

/// Says the engine exits once dropped, however the code holding it ends: `Desk::close`
/// gives it to the connection that closed the desk, which answers its bridge first.
#[derive(Debug)]
pub(crate) struct Closing(Arc<Seat>);

impl Drop for Closing {
    fn drop(&mut self) {
        self.0.exit.send_replace(true);
    }
}

/// Marks a session as taking the lease until dropped.
struct Taking<'a>(&'a Seat);

impl Drop for Taking<'_> {
    fn drop(&mut self) {
        self.0.taking().take();
    }
}

/// The lease, held for one session.
#[derive(Debug)]
struct Grant {
    owner: SessionId,
    lease: Lease,
}

/// Who holds the lease, as read without the action mutex.
#[derive(Debug, Clone)]
struct Owner {
    session: SessionId,
    holder: Holder,
    /// The lease's file, to check it is still the one locked.
    locked: lease::Locked,
    /// The window that had keyboard focus when the lease was taken, to give it back to.
    users_window: Option<u64>,
}

impl Seat {
    /// No code panics while holding the lock, so a poisoned one still holds a sound value.
    fn answer(&self) -> std::sync::MutexGuard<'_, Option<Outcome>> {
        self.answered.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The same.
    fn taking(&self) -> std::sync::MutexGuard<'_, Option<SessionId>> {
        self.taking.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The same.
    fn lock_read(&self) -> std::sync::MutexGuard<'_, Option<Instant>> {
        self.lock_read
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn put(&self, held: &mut Option<Grant>, grant: Grant, users_window: Option<u64>) {
        self.owner.send_replace(Some(Owner {
            session: grant.owner,
            holder: grant.lease.holder().clone(),
            locked: grant.lease.locked().clone(),
            users_window,
        }));
        self.refs().start(grant.owner);
        *held = Some(grant);
    }

    /// Gives the lease up, whoever holds it. Returns whether it was held.
    fn take(&self, held: &mut Option<Grant>) -> bool {
        let had = held.take().is_some();
        self.owner.send_replace(None);
        self.refs().end();
        had
    }

    /// Gives the lease up once any running action has ended, if `whose` holds it, or
    /// whoever does for `None`. Returns whether it was given up. Another session holding the
    /// lease, or taking it, means `whose` has nothing to give up, so it doesn't wait for that
    /// session, which may be waiting a minute for its user's answer.
    async fn give_up(&self, whose: Option<SessionId>) -> bool {
        let others = |session| {
            self.held_by_other(session).is_some()
                || self.taking().is_some_and(|taker| taker != session)
        };
        if whose.is_some_and(others) {
            return false;
        }
        let mut held = self.lease.lock().await;
        let owned = held
            .as_ref()
            .is_some_and(|grant| whose.is_none_or(|session| grant.owner == session));
        let released = owned && self.take(&mut held);
        drop(held);
        released
    }

    /// The holder, if a session other than `session` holds the lease. Read without the
    /// action mutex: while another session holds the lease, `session` can't be granted it,
    /// so the owner's running action has nothing to make `session` wait for. With the lease
    /// free, `session` may be taking it right now, so the caller waits for the mutex.
    fn held_by_other(&self, session: SessionId) -> Option<Holder> {
        self.owner
            .borrow()
            .as_ref()
            .filter(|owner| owner.session != session)
            .map(|owner| owner.holder.clone())
    }

    /// The owner, if it is `session`.
    fn owned_by(&self, session: SessionId) -> Option<Owner> {
        self.owner
            .borrow()
            .as_ref()
            .filter(|owner| owner.session == session)
            .cloned()
    }

    /// No code panics while holding the lock, so a poisoned one still holds sound refs.
    fn refs(&self) -> std::sync::MutexGuard<'_, Refs> {
        self.refs.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// What `Desk::acquire_with` asks for last, before the lease is granted.
#[derive(Debug)]
pub(crate) struct Prepared<W> {
    /// The window with keyboard focus, to give focus back to.
    pub(crate) users_window: Option<u64>,
    /// What must exist before the grant is published.
    pub(crate) with: W,
}

/// What `status` reports about the lease.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct LeaseStatus {
    pub(crate) held_by_me: bool,
    pub(crate) holder: Option<Holder>,
    /// Why no lease can be taken for this niri at all, as `acquire_desktop` says it.
    pub(crate) error: Option<String>,
}

impl Desk {
    /// Starts watching the stop flag on the current Tokio runtime, when `env` names a niri
    /// instance.
    pub(crate) fn start(env: &Env) -> Self {
        let seat = Arc::new(Seat {
            lease: Mutex::new(None),
            owner: watch::Sender::new(None),
            refs: std::sync::Mutex::new(Refs::default()),
            sending: AtomicBool::new(false),
            answered: std::sync::Mutex::new(None),
            lock_read: std::sync::Mutex::new(None),
            taking: std::sync::Mutex::new(None),
            closed: AtomicBool::new(false),
            exit: watch::Sender::new(false),
        });
        let runtime = RuntimeDir::of(env).map_err(|detail| {
            let name = if env.instance.socket().is_err() {
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

    /// `acquire_with`, for a lease that needs nothing prepared but `users_window`.
    #[cfg(test)]
    pub(crate) async fn acquire(
        &self,
        session: &Session,
        label: &str,
        refusal: impl Future<Output = Option<ToolError>>,
        users_window: impl Future<Output = Option<u64>>,
    ) -> Result<Holder, ToolError> {
        let prepare = |_| async {
            Prepared {
                users_window: users_window.await,
                with: (),
            }
        };
        let (holder, _) = self.acquire_with(session, label, refusal, prepare).await?;
        Ok(holder)
    }

    /// Takes the lease for `session`, labelled `label`, or returns the holder if that
    /// session has it already. Another session's lease refuses at once, without waiting
    /// for its owner's running action, and another server's lock refuses before anything
    /// slow is asked. `refusal`, the policy's answer, is asked under the action mutex with
    /// the lock taken, after the stop flag and the input-dirty marker, so a wait for the
    /// mutex can't make it stale; a refusal gives the lock back. A stop or the session's end
    /// while `refusal` runs refuses at once, since it may wait for the user. `prepare`, asked
    /// last with the number the lease's refs will have, finds the window with keyboard
    /// focus, kept with the lease, and makes what must exist before the grant is published,
    /// which is returned with the holder. A call dropped before then leaves no lease. Since both
    /// take time, the session's end, the stop flag and the marker are checked again before
    /// the lease is granted. A session that has the lease already gets no new `with`.
    pub(crate) async fn acquire_with<W, F>(
        &self,
        session: &Session,
        label: &str,
        refusal: impl Future<Output = Option<ToolError>>,
        prepare: impl FnOnce(u64) -> F,
    ) -> Result<(Holder, Option<W>), ToolError>
    where
        F: Future<Output = Prepared<W>>,
    {
        let runtime = self.runtime.as_ref().map_err(Clone::clone)?;
        // The same checks, in the same order, as under the mutex.
        self.unblocked(runtime)?;
        if let Some(holder) = self.seat.held_by_other(session.id()) {
            return Err(refused(Refused::Held(Some(holder))));
        }
        let mut held = self.seat.lease.lock().await;
        // Checked under the mutex, so `end_session` either sees this grant or refuses it.
        if session.has_ended() {
            return Err(session_ended());
        }
        if self.seat.closed.load(Ordering::Relaxed) {
            drop(held);
            // The engine exits once its `Closing` is dropped, which ends the connection:
            // the bridge answers this call `engine_lost` then, as every call in flight.
            return std::future::pending().await;
        }
        if let Some(grant) = held.as_ref().filter(|grant| grant.owner == session.id()) {
            return Ok((grant.lease.holder().clone(), None));
        }
        let mut stopped = self.unblocked(runtime)?;
        // Another session's grant refuses where another server's lock would.
        if let Some(grant) = held.as_ref() {
            return Err(refused(Refused::Held(Some(grant.lease.holder().clone()))));
        }
        let mut lease = Lease::acquire(runtime, label).map_err(refused)?;
        // No grant exists now, so no other session has a lease to give up meanwhile.
        *self.seat.taking() = Some(session.id());
        let _taking = Taking(&self.seat);
        // `refusal` may wait for the user's answer, so a stop or the session's end ends it.
        let refusal = tokio::select! {
            biased;
            () = session.ended() => Some(session_ended()),
            waited = stopped.wait_for(|stopped| *stopped) => {
                Some(waited.map_or_else(|_| watcher_ended(), |_| stopped_before_grant()))
            }
            answered = refusal => answered,
        };
        if let Some(refusal) = refusal {
            return Err(refusal);
        }
        let next = self.seat.refs().next();
        let prepared = prepare(next).await;
        if session.has_ended() {
            return Err(session_ended());
        }
        self.unblocked(runtime)?;
        lease.publish().map_err(|unpublished| match unpublished {
            Unpublished::Moved => lease_moved("granted nothing; call acquire_desktop again"),
            Unpublished::Io(detail) => ToolError::new(ErrorName::UpstreamError, detail),
        })?;
        let holder = lease.holder().clone();
        let grant = Grant {
            owner: session.id(),
            lease,
        };
        self.seat.put(&mut held, grant, prepared.users_window);
        drop(held);
        Ok((holder, Some(prepared.with)))
    }

    /// Runs one action of `session` with the action mutex held. It runs only while neither
    /// the stop flag nor the input-dirty marker is set, `session` holds the lease, and
    /// `refusal`, the policy's answer asked once those checks pass, has none; the stop flag
    /// and the marker are checked again after it, since it takes time. A session
    /// without the lease is refused without waiting for another session's action. The lease
    /// file must still be the one locked right before `work` starts. A stop that arrives
    /// while `work` runs cancels it; the stop watcher then takes the lease back. A lease
    /// file removed or replaced meanwhile cancels it too, and the lease is given up at once.
    /// `finish` turns the work's result into the call's, still under the mutex but past the
    /// stop, so a stop can't discard what the work already found. A stop or a moved lease
    /// that cancels the work after it called `sending` is no refusal either: `finish` gets
    /// `Worked::CutAfterSending`, since the call may have taken effect. The session ending
    /// drops the action in any phase: waiting for the mutex, `refusal`, `work` or `finish`.
    pub(crate) async fn act<T, U, F>(
        &self,
        session: &Session,
        refusal: impl Future<Output = Option<ToolError>>,
        work: impl Future<Output = Result<T, CallError>>,
        finish: impl FnOnce(Worked<T>) -> F,
    ) -> Result<U, CallError>
    where
        F: Future<Output = U>,
    {
        let runtime = self.runtime.as_ref().map_err(Clone::clone)?;
        // The same checks, in the same order, as under the mutex.
        self.unblocked(runtime)?;
        if self.seat.owned_by(session.id()).is_none() {
            return Err(lease_required().into());
        }
        tokio::select! {
            biased;
            () = session.ended() => Err(session_ended().into()),
            result = self.gated(session, refusal, work, finish) => result,
        }
    }

    /// `act` under the action mutex.
    async fn gated<T, U, F>(
        &self,
        session: &Session,
        refusal: impl Future<Output = Option<ToolError>>,
        work: impl Future<Output = Result<T, CallError>>,
        finish: impl FnOnce(Worked<T>) -> F,
    ) -> Result<U, CallError>
    where
        F: Future<Output = U>,
    {
        let runtime = self.runtime.as_ref().map_err(Clone::clone)?;
        let mut held = self.seat.lease.lock().await;
        self.unblocked(runtime)?;
        let Some(grant) = held.as_ref().filter(|grant| grant.owner == session.id()) else {
            return Err(lease_required().into());
        };
        self.seat.lock_read().take();
        if let Some(refusal) = refusal.await {
            return Err(refusal.into());
        }
        // `refusal` takes time, so a stop or another input's marker may have come meanwhile.
        let mut stopped = self.unblocked(runtime)?;
        self.seat.sending.store(false, Ordering::Relaxed);
        self.seat.answer().take();
        let ended = if grant.lease.intact() {
            tokio::select! {
                biased;
                _ = stopped.wait_for(|stopped| *stopped) => Ended::Stopped,
                () = moved(&grant.lease) => Ended::Moved("cancelled the action; anything already sent may have taken effect"),
                done = work => Ended::Done(done),
            }
        } else {
            Ended::Moved("sent nothing")
        };
        let sending = self.seat.sending.load(Ordering::Relaxed);
        let cut = match ended {
            Ended::Done(done) => {
                let finished = finish(Worked::Done(done?)).await;
                drop(held);
                return Ok(finished);
            }
            Ended::Stopped => cancelled(&stopped, sending),
            Ended::Moved(what) => {
                self.seat.take(&mut held);
                let what = if sending { AFTER_SENDING } else { what };
                // Without the directory, the stop watcher is about to end too.
                if runtime.path().exists() {
                    lease_moved(what)
                } else {
                    watcher_ended()
                }
            }
        };
        if !sending {
            return Err(cut.into());
        }
        let answered = self.seat.answer().take().map(Box::new);
        Ok(finish(Worked::CutAfterSending(cut, answered)).await)
    }

    /// Says the running action's one call that acts is going out, so a stop or a moved
    /// lease that cancels the action from now on reports it as uncertain rather than
    /// refused. It may go out only while neither the stop flag nor the input-dirty marker is
    /// set, since the work may have waited on Noctalia or the app since the desk checked, and
    /// as `fresh` says; it is refused otherwise, with nothing marked. Only the action holding
    /// the action mutex calls it, from its work.
    pub(crate) fn sending(&self) -> Result<(), ToolError> {
        let runtime = self.runtime.as_ref().map_err(Clone::clone)?;
        self.unblocked(runtime)?;
        self.sending_input()
    }

    /// `sending`, for input whose own input-dirty marker is set by now: only `fresh`'s
    /// checks, the stop flag and the lock state's age.
    pub(crate) fn sending_input(&self) -> Result<(), ToolError> {
        self.fresh()?;
        self.seat.sending.store(true, Ordering::Relaxed);
        Ok(())
    }

    /// Whether a call that acts, or the next batch of a call's input, may go out now: the
    /// stop flag isn't set, read right now rather than waited for from the stop watcher,
    /// which may not have seen a flag set a moment ago, and the latest read of the lock
    /// state began at most `LAST_GATE` ago. Otherwise `stopped`, `lock_stale`, since that
    /// read may no longer hold, or `screen_locked` when the action read none. The
    /// input-dirty marker isn't checked: input writes its own before it goes out.
    pub(crate) fn fresh(&self) -> Result<(), ToolError> {
        let runtime = self.runtime.as_ref().map_err(Clone::clone)?;
        self.not_stopped(runtime)?;
        let began = *self.seat.lock_read();
        match began {
            Some(began) if began.elapsed() <= LAST_GATE => Ok(()),
            Some(_) => Err(ToolError::new(
                ErrorName::LockStale,
                format!(
                    "the lock state was read over {} ms before the call that acts could go out, so it isn't known to be unlocked now, and that call wasn't sent; the desktop answered slowly, so try once more",
                    LAST_GATE.as_millis()
                ),
            )),
            None => Err(ToolError::new(
                ErrorName::ScreenLocked,
                "the lock state wasn't read for this action, so it isn't known to be unlocked, and nothing was sent",
            )),
        }
    }

    /// Whether the latest read of the lock state began at most `LAST_GATE` ago.
    pub(crate) fn lock_fresh(&self) -> bool {
        self.seat
            .lock_read()
            .is_some_and(|began| began.elapsed() <= LAST_GATE)
    }

    /// Notes that a read of the lock state began at `began`, for the action holding the
    /// action mutex, or for `acquire`'s refusal.
    pub(crate) fn lock_read(&self, began: Instant) {
        *self.seat.lock_read() = Some(began);
    }

    /// Says the app or niri answered the running action's call that acts, with `outcome` as
    /// far as it is known then, which a stop or a moved lease that cancels the action from
    /// now on reports with its reason. Only the action holding the action mutex calls it.
    pub(crate) fn answered(&self, outcome: Outcome) {
        *self.seat.answer() = Some(outcome);
    }

    /// The desk's checks before an action, asked again while `session`'s action runs, as an
    /// element action does right before the one call that acts: neither the stop flag nor
    /// the input-dirty marker is set, `session` holds the lease, and the lease file is
    /// still the one locked. Doesn't wait for the action mutex, which the running action
    /// holds.
    pub(crate) fn checkpoint(&self, session: &Session) -> Result<(), ToolError> {
        let runtime = self.runtime.as_ref().map_err(Clone::clone)?;
        self.unblocked(runtime)?;
        let owner = self
            .seat
            .owned_by(session.id())
            .ok_or_else(lease_required)?;
        if !owner.locked.intact() {
            return Err(lease_moved("sent nothing"));
        }
        Ok(())
    }

    /// Checks that a stop can reach this server and that neither the stop flag nor the
    /// input-dirty marker is set. Returns the watcher's view of the flag.
    fn unblocked(&self, runtime: &RuntimeDir) -> Result<watch::Receiver<bool>, ToolError> {
        let stopped = self.not_stopped(runtime)?;
        if runtime.input_dirty().map_err(|error| unreadable(&error))? {
            let found = marker::read(runtime);
            return Err(input_dirty(found.as_ref(), Path::new("/proc")));
        }
        Ok(stopped)
    }

    /// Checks that a stop can reach this server and that the stop flag isn't set, by the
    /// watcher's view or the file itself. Returns the watcher's view of the flag.
    fn not_stopped(&self, runtime: &RuntimeDir) -> Result<watch::Receiver<bool>, ToolError> {
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
        Ok(stopped.clone())
    }

    /// Gives `session`'s lease up, once any running action has ended. Returns whether
    /// `session` held it; another session's lease stays.
    pub(crate) async fn release(&self, session: &Session) -> bool {
        self.seat.give_up(Some(session.id())).await
    }

    /// Ends `session` for good: drops its running action or observation, whatever phase it
    /// is in, and gives its lease up the way a stop does, without giving focus back, since
    /// no one is left to have asked for that. It can't take the lease again.
    pub(crate) async fn end_session(&self, session: &Session) {
        session.end();
        self.seat.give_up(Some(session.id())).await;
        self.seat.refs().forget(session.id());
    }

    /// Serializes an observation of `session` with this server's actions and lease changes
    /// while `session` holds the lease, so a ref issued for it matches the desktop its
    /// actions left. It needs no lease and does not freeze external input or redraws.
    /// Action-return captures already hold the mutex and must not call this again. The
    /// session ending drops an observation that holds the mutex.
    pub(crate) async fn observe<T>(
        &self,
        session: &Session,
        capture: impl Future<Output = Result<T, CallError>>,
    ) -> Result<T, CallError> {
        if self.seat.owned_by(session.id()).is_none() {
            return capture.await;
        }
        let serialized = async {
            let held = self.seat.lease.lock().await;
            let result = capture.await;
            drop(held);
            result
        };
        tokio::select! {
            biased;
            () = session.ended() => Err(session_ended().into()),
            result = serialized => result,
        }
    }

    /// The window that had keyboard focus when `session` took the lease it holds.
    pub(crate) fn users_window(&self, session: &Session) -> Option<u64> {
        self.seat
            .owned_by(session.id())
            .and_then(|owner| owner.users_window)
    }

    /// Whether `lease` is the next lease, not granted yet.
    pub(crate) fn pending(&self, lease: u64) -> bool {
        self.seat.refs().next() == lease
    }

    /// The lease a screenshot of `session` starting now would issue its ref under, if
    /// `session` holds one. Doesn't wait for a running action.
    pub(crate) fn ref_lease(&self, session: &Session) -> Option<u64> {
        self.seat.owned_by(session.id())?;
        self.seat.refs().lease()
    }

    /// Keeps `shot` as a ref of `lease` and returns its id, unless that lease has ended
    /// since the capture started.
    pub(crate) fn remember(&self, lease: u64, shot: Shot) -> Option<String> {
        self.seat.refs().insert(lease, shot)
    }

    /// The stretch without the lease that a screenshot of `session` starting now would keep
    /// its ref in.
    pub(crate) fn stretch(&self, session: &Session) -> u64 {
        self.seat.refs().stretch(session.id())
    }

    /// Keeps `shot`, which `session` took without the lease during `stretch`, as a ref only a
    /// zoom takes, and returns its id, unless `session` has ended, holds the lease by now,
    /// or has taken it since the capture started.
    pub(crate) fn remember_view(
        &self,
        session: &Session,
        stretch: u64,
        shot: Shot,
    ) -> Option<String> {
        let mut refs = self.seat.refs();
        if session.has_ended() || self.seat.owned_by(session.id()).is_some() {
            return None;
        }
        refs.insert_view(session.id(), stretch, shot)
    }

    /// The ref named `id` that a zoom of `session` takes: one of the lease it holds, or,
    /// without the lease, one it took since it last held it.
    pub(crate) fn zoomable(&self, session: &Session, id: &str) -> Result<Shot, ToolError> {
        let refs = self.seat.refs();
        if self.seat.owned_by(session.id()).is_some() {
            return refs.get(id);
        }
        refs.view(session.id(), id)
    }

    /// Keeps `element` as an element ref of `lease` and returns its id, unless that lease
    /// has ended since.
    pub(crate) fn remember_element(&self, lease: u64, element: ElementRef) -> Option<String> {
        self.seat.refs().insert_element(lease, element)
    }

    /// The element ref named `id` of the lease `session` holds.
    pub(crate) fn element(&self, session: &Session, id: &str) -> Result<ElementRef, ToolError> {
        self.owned_refs(session)
            .map_or_else(|| Refs::default().element(id), |refs| refs.element(id))
    }

    /// The runtime directory, where input writes its marker.
    pub(crate) fn runtime(&self) -> Result<&RuntimeDir, ToolError> {
        self.runtime.as_ref().map_err(Clone::clone)
    }

    /// The ref named `id` of the lease `session` holds.
    pub(crate) fn shot(&self, session: &Session, id: &str) -> Result<Shot, ToolError> {
        self.owned_refs(session)
            .map_or_else(|| Refs::default().get(id), |refs| refs.get(id))
    }

    /// The refs of the lease `session` holds, if it holds the lease. Taken before the
    /// owner is read, so a lease change can't come in between.
    fn owned_refs(&self, session: &Session) -> Option<std::sync::MutexGuard<'_, Refs>> {
        let refs = self.seat.refs();
        self.seat.owned_by(session.id()).map(|_| refs)
    }

    /// Whether a session holds the lease. Doesn't wait for a running action.
    pub(crate) fn held(&self) -> bool {
        self.seat.owner.borrow().is_some()
    }

    /// Closes the desk for good, for an engine that exits, when no session holds or is
    /// taking the lease and `settled`, asked under the action mutex, says no input cleanup
    /// is pending: checked first without the mutex, so a lease holder's action is never
    /// waited for, then under it, waiting at most `CLOSE_WAIT`. A closed desk grants no
    /// lease again. Returns what says the engine exits once dropped, or `None` with
    /// nothing changed. Closing a closed desk gives another.
    pub(crate) async fn close(&self, settled: impl FnOnce() -> bool) -> Option<Closing> {
        if self.held() || self.seat.taking().is_some() {
            return None;
        }
        let held = tokio::time::timeout(CLOSE_WAIT, self.seat.lease.lock())
            .await
            .ok()?;
        if held.is_some() || !settled() {
            return None;
        }
        self.seat.closed.store(true, Ordering::Relaxed);
        drop(held);
        Some(Closing(Arc::clone(&self.seat)))
    }

    /// Returns once a `Closing` has been dropped: at once if one was.
    pub(crate) async fn exiting(&self) {
        self.seat.exit.subscribe().wait_for(|exit| *exit).await.ok();
    }

    /// Returns once the stop watcher has ended, for good: never, without a watcher.
    pub(crate) async fn watcher_ended(&self) {
        let Ok(stopped) = &self.stopped else {
            return std::future::pending().await;
        };
        let mut stopped = stopped.clone();
        while stopped.changed().await.is_ok() {}
    }

    /// The lease as `session` sees it. Doesn't wait for a running action.
    pub(crate) fn status(&self, session: &Session) -> LeaseStatus {
        let owner = self.seat.owner.borrow().clone();
        LeaseStatus {
            held_by_me: owner
                .as_ref()
                .is_some_and(|owner| owner.session == session.id()),
            holder: owner
                .map(|owner| owner.holder)
                .or_else(|| self.runtime.as_ref().ok().and_then(lease::holder)),
            error: self
                .runtime
                .as_ref()
                .err()
                .map(|error| error.detail.clone()),
        }
    }
}

/// What `status` reports when there is no desk, as in the `status` subcommand.
pub(crate) fn status_without_desk(env: &Env) -> LeaseStatus {
    let runtime = RuntimeDir::of(env);
    LeaseStatus {
        held_by_me: false,
        holder: runtime.as_ref().ok().and_then(lease::holder),
        error: runtime.err(),
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

/// What an action's work came to, for `finish` to make the call's result of.
#[derive(Debug)]
pub(crate) enum Worked<T> {
    Done(T),
    /// A stop or the lease moving cancelled the work after it said its one call that acts
    /// was going out (`Desk::sending`), so that call may have taken effect: why, and the
    /// outcome so far when the app or niri had answered it (`Desk::answered`).
    CutAfterSending(ToolError, Option<Box<Outcome>>),
}

/// How an action's work ended.
enum Ended<T> {
    Done(T),
    /// The stop flag cancelled it.
    Stopped,
    /// The lease file was removed or replaced, before the work or during it, as said.
    Moved(&'static str),
}

/// Returns once `lease` no longer names the file it locked. A running action checks this
/// itself, because the periodic check waits for the action mutex the action holds.
async fn moved(lease: &Lease) {
    let mut check = tokio::time::interval(MOVED_CHECK);
    loop {
        check.tick().await;
        if !lease.intact() {
            return;
        }
    }
}

fn lease_moved(what: &str) -> ToolError {
    ToolError::new(
        ErrorName::LeaseRequired,
        format!(
            "the lease file was removed or replaced, so another server may hold the lease; this server gave it up and {what}"
        ),
    )
}

/// Drops the lease if its file was removed or replaced. Returns false once the desk is
/// gone.
async fn give_up_if_moved(seat: &Weak<Seat>) -> bool {
    let Some(seat) = seat.upgrade() else {
        return false;
    };
    let mut held = seat.lease.lock().await;
    if held.as_ref().is_some_and(|grant| !grant.lease.intact()) {
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
    seat.give_up(None).await;
    true
}

fn session_ended() -> ToolError {
    ToolError::new(
        ErrorName::LeaseRequired,
        "this session ended, so its action was dropped and its lease given up",
    )
}

fn lease_required() -> ToolError {
    ToolError::new(
        ErrorName::LeaseRequired,
        "this session doesn't hold the lease; call acquire_desktop first",
    )
}

fn stopped_before_grant() -> ToolError {
    ToolError::new(
        ErrorName::Stopped,
        "the user's stop flag was set before the lease was granted",
    )
}

/// Why a running action was cancelled. A watcher that loses the runtime directory reports
/// a stop and then ends, which on this single-threaded runtime happens before the action's
/// task runs again.
fn cancelled(stopped: &watch::Receiver<bool>, sending: bool) -> ToolError {
    if stopped.has_changed().is_err() {
        return watcher_ended();
    }
    let what = if sending {
        AFTER_SENDING
    } else {
        "cancelled this action; anything already sent may have taken effect"
    };
    ToolError::new(ErrorName::Stopped, format!("the user's stop flag {what}"))
}

/// What a stop or a moved lease did to an action whose call that acts had gone out.
const AFTER_SENDING: &str = "cancelled the action after its call that acts went out, so that call may have taken effect; nothing more was asked";

/// The refusal while the input-dirty marker is set. A marker whose input this server still
/// holds belongs to a dropped call's input that is still finishing, which `recover` would
/// cut short. One another live server wrote may be too, which only that server knows, so
/// the agent tries again once before `recover`. Any other stays until `recover`, which
/// needs the lease.
fn input_dirty(found: Option<&marker::Found>, proc_root: &Path) -> ToolError {
    let summary = found.map_or_else(String::new, marker::Found::summary);
    let marker = match found {
        Some(marker::Found::Marker(marker)) => Some(marker),
        _ => None,
    };
    let detail = match marker {
        Some(marker) if marker.finishing() => format!(
            "a cancelled call's input is still finishing ({summary}); try again once it has, within seconds"
        ),
        Some(marker) if another_server_may_finish(marker, proc_root) => format!(
            "input may be stuck ({summary}), or another server's input may still be finishing; try again in a few seconds, and if it still refuses, call release_desktop, then the user runs `niri-computer-use recover`, which needs the lease"
        ),
        _ => format!(
            "input may be stuck ({summary}); call release_desktop, then the user runs `niri-computer-use recover`, which needs the lease"
        ),
    };
    ToolError::new(ErrorName::RecoveryRequired, detail)
}

/// Whether another server wrote `marker` and may still clear it: it runs, as far as
/// `proc_root` tells, and its crash guardian hasn't sent the releases, which it does once
/// the server has died. The marker records no start time, so a PID reused since the
/// server died reads as running.
fn another_server_may_finish(marker: &marker::Marker, proc_root: &Path) -> bool {
    marker.server_pid != std::process::id()
        && marker.released.is_none()
        && procs::stat(proc_root, marker.server_pid).is_some_and(|stat| !stat.exited())
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
        Refused::Held(Some(holder)) if holder.pending => ToolError::new(
            ErrorName::LeaseHeld,
            format!(
                "being taken by PID {} ({}) since {}, which may be waiting for its user to allow it",
                holder.pid, holder.label, holder.since
            ),
        ),
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
    use std::future::ready;
    use std::time::Duration;

    use super::*;

    use crate::test_support::session;

    fn env(dir: &Path) -> Env {
        crate::test_support::niri_env(dir)
    }

    /// Whether the desk gives the lease up within five seconds.
    async fn released(desk: &Desk) -> bool {
        let me = session(1);
        for _ in 0..500 {
            if !desk.status(&me).held_by_me {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        false
    }

    #[tokio::test]
    async fn takes_keeps_and_gives_back_the_lease() {
        let me = session(1);
        let dir = crate::test_support::fresh_dir("desk");
        let desk = Desk::start(&env(&dir));
        let other = Desk::start(&env(&dir));
        let holder = desk
            .acquire(&me, "me/1", ready(None), ready(None))
            .await
            .unwrap();
        assert_eq!(
            desk.acquire(&me, "me/1", ready(None), ready(None))
                .await
                .unwrap(),
            holder
        );
        let status = desk.status(&me);
        assert!(status.held_by_me);
        assert_eq!(other.status(&me).holder, Some(holder.clone()));
        let refused = other
            .acquire(&me, "other/2", ready(None), ready(None))
            .await
            .unwrap_err();
        assert_eq!(refused.name, ErrorName::LeaseHeld);
        assert!(refused.detail.contains("(me/1)"), "{}", refused.detail);
        assert!(desk.release(&me).await);
        assert!(!desk.release(&me).await);
        assert_eq!(
            other
                .acquire(&me, "other/2", ready(None), ready(None))
                .await
                .unwrap()
                .label,
            "other/2"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// An engine closes its desk only with no lease held or being taken, and once closed
    /// grants none, so no lease is cut by its exit; the exit is said once the guard goes,
    /// whatever its holder did meanwhile.
    #[tokio::test]
    async fn a_desk_closes_only_with_the_lease_free_and_then_grants_none() {
        let me = session(1);
        let dir = crate::test_support::fresh_dir("desk-close");
        let desk = Desk::start(&env(&dir));
        desk.acquire(&me, "me/1", ready(None), ready(None))
            .await
            .unwrap();
        assert!(
            desk.close(|| true).await.is_none(),
            "closed with the lease held"
        );
        assert!(desk.release(&me).await);
        *desk.seat.taking() = Some(me.id());
        assert!(
            desk.close(|| true).await.is_none(),
            "closed while the lease was taken"
        );
        desk.seat.taking().take();
        // A mutex held past `CLOSE_WAIT`, as by an observation, keeps the desk open.
        let observing = desk.seat.lease.lock().await;
        assert!(
            desk.close(|| true).await.is_none(),
            "closed with the mutex held"
        );
        drop(observing);
        assert!(
            desk.close(|| false).await.is_none(),
            "closed with cleanup pending"
        );
        let closing = desk.close(|| true).await.expect("a free desk closes");
        let again = desk.close(|| true).await.expect("closing again");
        let other = session(2);
        let acquire = desk.acquire(&other, "other/2", ready(None), ready(None));
        assert!(
            tokio::time::timeout(Duration::from_millis(200), acquire)
                .await
                .is_err(),
            "a closed desk answered an acquire"
        );
        assert!(!desk.held());
        let exiting = tokio::time::timeout(Duration::from_millis(50), desk.exiting());
        assert!(exiting.await.is_err(), "exiting before the guard went");
        drop((closing, again));
        tokio::time::timeout(Duration::from_secs(1), desk.exiting())
            .await
            .expect("exiting once the guard went");
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// The lease's periodic check and a session's end take the action mutex for a moment;
    /// a hand-over asked meanwhile still closes the desk.
    #[tokio::test]
    async fn a_brief_hold_of_the_mutex_doesnt_keep_the_desk_open() {
        let dir = crate::test_support::fresh_dir("desk-close-brief");
        let desk = Desk::start(&env(&dir));
        let brief = desk.seat.lease.lock().await;
        let holder = desk.clone();
        let (closed, ()) = tokio::join!(holder.close(|| true), async {
            tokio::time::sleep(Duration::from_millis(20)).await;
            drop(brief);
        });
        assert!(closed.is_some());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn status_reports_the_lease_while_an_action_holds_the_mutex() {
        let me = session(1);
        let dir = crate::test_support::fresh_dir("desk-busy");
        let desk = Desk::start(&env(&dir));
        let holder = desk
            .acquire(&me, "me/1", ready(None), ready(None))
            .await
            .unwrap();
        let action = desk.seat.lease.lock().await;
        assert_eq!(
            desk.status(&me),
            LeaseStatus {
                held_by_me: true,
                holder: Some(holder),
                error: None
            }
        );
        drop(action);
        assert!(desk.release(&me).await);
        assert!(!desk.status(&me).held_by_me);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn release_waits_for_capture_and_a_cancelled_capture_unlocks_the_desk() {
        let me = session(1);
        let dir = crate::test_support::fresh_dir("desk-observe");
        let desk = Desk::start(&env(&dir));
        desk.acquire(&me, "me/1", ready(None), ready(None))
            .await
            .unwrap();
        let (started, running) = tokio::sync::oneshot::channel();
        let capture_desk = desk.clone();
        let work = async move {
            started.send(()).unwrap();
            std::future::pending::<Result<(), CallError>>().await
        };
        let capture_me = me.clone();
        let capture = tokio::spawn(async move { capture_desk.observe(&capture_me, work).await });
        running.await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(20), desk.release(&me))
                .await
                .is_err()
        );
        assert!(desk.status(&me).held_by_me);
        capture.abort();
        assert!(capture.await.unwrap_err().is_cancelled());
        assert!(desk.release(&me).await);
        // Observation is still available without a lease.
        assert_eq!(desk.observe(&me, async { Ok(7) }).await.unwrap(), 7);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn a_call_that_acts_goes_out_only_within_the_last_gate_of_the_lock_read() {
        let me = session(1);
        let dir = crate::test_support::fresh_dir("desk-lock-age");
        let desk = Desk::start(&env(&dir));
        desk.acquire(&me, "me/1", ready(None), ready(None))
            .await
            .unwrap();
        let send = || async { desk.sending().map_err(CallError::from) };
        let late = async {
            tokio::time::sleep(LAST_GATE + Duration::from_millis(20)).await;
            send().await
        };
        let refused = desk.act(&me, read_lock(&desk), late, done).await;
        assert!(
            matches!(&refused, Err(CallError::Tool(error)) if error.name == ErrorName::LockStale),
            "{refused:?}"
        );
        // A refusal that read no lock state lets nothing go out either.
        let unread = desk.act(&me, ready(None), send(), done).await;
        assert!(
            matches!(&unread, Err(CallError::Tool(error)) if error.name == ErrorName::ScreenLocked),
            "{unread:?}"
        );
        desk.act(&me, read_lock(&desk), send(), done).await.unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn a_marker_set_while_an_action_runs_keeps_its_call_from_going_out() {
        let me = session(1);
        let dir = crate::test_support::fresh_dir("desk-marker-send");
        let desk = Desk::start(&env(&dir));
        let runtime = RuntimeDir::of(&env(&dir)).unwrap();
        desk.acquire(&me, "me/1", ready(None), ready(None))
            .await
            .unwrap();
        let marked = async {
            std::fs::write(runtime.path().join("input-dirty"), "").unwrap();
            desk.sending().map_err(CallError::from)
        };
        let refused = desk.act(&me, read_lock(&desk), marked, done).await;
        assert!(
            matches!(&refused, Err(CallError::Tool(error)) if error.name == ErrorName::RecoveryRequired),
            "{refused:?}"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// `finish` for work that is never cut short after sending: its own result.
    fn done<T>(worked: Worked<T>) -> std::future::Ready<T> {
        match worked {
            Worked::Done(done) => ready(done),
            Worked::CutAfterSending(cut, _) => panic!("{cut:?}"),
        }
    }

    /// An action whose work records that it ran and returns `value`.
    async fn act(desk: &Desk, refusal: Option<ToolError>) -> Result<u8, ErrorName> {
        let me = session(1);
        desk.act(&me, async { refusal }, async { Ok(7) }, done)
            .await
            .map_err(|error| match error {
                CallError::Tool(error) => error.name,
                CallError::InvalidArguments(message) => panic!("{message}"),
            })
    }

    #[tokio::test]
    async fn an_action_needs_the_lease_and_a_clear_gate() {
        let me = session(1);
        let dir = crate::test_support::fresh_dir("desk-act");
        let desk = Desk::start(&env(&dir));
        let runtime = RuntimeDir::of(&env(&dir)).unwrap();
        assert_eq!(act(&desk, None).await, Err(ErrorName::LeaseRequired));
        desk.acquire(&me, "me/1", ready(None), ready(None))
            .await
            .unwrap();
        assert_eq!(act(&desk, None).await, Ok(7));
        let locked = ToolError::new(ErrorName::ScreenLocked, "locked");
        assert_eq!(act(&desk, Some(locked)).await, Err(ErrorName::ScreenLocked));
        // The marker comes before the lease and the policy, so an agent is told to stop.
        std::fs::write(runtime.path().join("input-dirty"), "").unwrap();
        desk.release(&me).await;
        assert_eq!(act(&desk, None).await, Err(ErrorName::RecoveryRequired));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn only_a_marker_whose_input_is_still_finishing_spares_the_user_recover() {
        let dir = crate::test_support::fresh_dir("desk-finishing");
        let runtime = RuntimeDir::of(&env(&dir)).unwrap();
        runtime.create().unwrap();
        // No process runs in this `/proc` yet.
        let proc_root = dir.join("proc");
        let detail = |found: Option<marker::Found>| input_dirty(found.as_ref(), &proc_root).detail;
        // Input this server still holds clears its own marker, which `recover` would cut
        // short.
        let written =
            marker::Written::write(&runtime, marker::Marker::pending("paste", Vec::new()))
                .await
                .unwrap();
        let held = detail(marker::read(&runtime));
        assert!(
            held.contains("still finishing") && !held.contains("recover"),
            "{held}"
        );
        // Input that gave up on its marker, such as a cleanup past its deadline, leaves it
        // for the user, though this server wrote it.
        drop(written);
        let given_up = detail(marker::read(&runtime));
        assert!(
            given_up
                .contains("call release_desktop, then the user runs `niri-computer-use recover`")
                && !given_up.contains("try again"),
            "{given_up}"
        );
        assert!(detail(None).contains("`niri-computer-use recover`"));
        // Another server's input, such as a shared engine's beside this standalone server,
        // may still be finishing while that server runs; only it knows.
        let mut other = marker::Marker::pending("key", Vec::new());
        other.server_pid = std::process::id() + 1;
        let others = |marker: &marker::Marker| {
            let path = runtime.path().join(crate::control::runtime::INPUT_DIRTY);
            std::fs::write(path, serde_json::to_vec(marker).unwrap()).unwrap();
            detail(marker::read(&runtime))
        };
        let dead = others(&other);
        assert!(
            dead.contains("call release_desktop, then the user runs `niri-computer-use recover`")
                && !dead.contains("try again"),
            "{dead}"
        );
        let pid = other.server_pid;
        let stat = |state: char| {
            let process = proc_root.join(pid.to_string());
            std::fs::create_dir_all(&process).unwrap();
            let stat = format!(
                "{pid} (niri-computer-u) {state} 1 {pid} {pid} 0 -1 0 0 0 0 0 0 0 0 0 20 0 1 0 98765 0 0"
            );
            std::fs::write(process.join("stat"), stat).unwrap();
        };
        stat('S');
        let running = others(&other);
        assert!(
            running.contains(
                "another server's input may still be finishing; try again in a few seconds"
            ) && running.contains("then the user runs `niri-computer-use recover`"),
            "{running}"
        );
        // A server that has exited, or whose crash guardian has sent the releases, which
        // it does once the server died, finishes nothing, whatever runs under its PID now.
        stat('Z');
        assert!(!others(&other).contains("try again"));
        stat('S');
        other.released = Some(marker::now());
        let released = others(&other);
        assert!(
            released.contains("the crash guardian sent its releases")
                && !released.contains("try again"),
            "{released}"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn a_stop_cancels_the_running_action_then_takes_the_lease_back() {
        let me = session(1);
        let dir = crate::test_support::fresh_dir("desk-act-stop");
        let desk = Desk::start(&env(&dir));
        let runtime = RuntimeDir::of(&env(&dir)).unwrap();
        desk.acquire(&me, "me/1", ready(None), ready(None))
            .await
            .unwrap();
        let (started, running) = tokio::sync::oneshot::channel::<()>();
        let (held, dropped) = tokio::sync::oneshot::channel::<()>();
        let action = desk.act(
            &me,
            async { None },
            async move {
                started.send(()).unwrap();
                let _held = held;
                std::future::pending::<Result<(), CallError>>().await
            },
            done,
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

    /// Removes the `lease` file and lets `other` lock the new one, as a second server would.
    async fn replace_lease(runtime: &RuntimeDir, other: &Desk) {
        let me = session(1);
        std::fs::remove_file(runtime.path().join("lease")).unwrap();
        other
            .acquire(&me, "other/2", ready(None), ready(None))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn a_lease_replaced_during_the_readiness_check_refuses_the_work() {
        let me = session(1);
        let dir = crate::test_support::fresh_dir("desk-act-replaced");
        let desk = Desk::start(&env(&dir));
        let other = Desk::start(&env(&dir));
        let runtime = RuntimeDir::of(&env(&dir)).unwrap();
        desk.acquire(&me, "me/1", ready(None), ready(None))
            .await
            .unwrap();
        let readiness = async {
            replace_lease(&runtime, &other).await;
            None
        };
        let mut ran = false;
        let result = desk
            .act(
                &me,
                readiness,
                async {
                    ran = true;
                    Ok(())
                },
                done,
            )
            .await;
        let Err(CallError::Tool(error)) = result else {
            panic!("{result:?}");
        };
        assert_eq!(error.name, ErrorName::LeaseRequired);
        assert!(!ran);
        assert!(!desk.status(&me).held_by_me);
        assert!(other.status(&me).held_by_me);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn a_lease_replaced_mid_action_cancels_it_and_gives_the_lease_up() {
        let me = session(1);
        let dir = crate::test_support::fresh_dir("desk-act-moved");
        let desk = Desk::start(&env(&dir));
        let other = Desk::start(&env(&dir));
        let runtime = RuntimeDir::of(&env(&dir)).unwrap();
        desk.acquire(&me, "me/1", ready(None), ready(None))
            .await
            .unwrap();
        let (started, running) = tokio::sync::oneshot::channel::<()>();
        let (held, dropped) = tokio::sync::oneshot::channel::<()>();
        let action = desk.act(
            &me,
            async { None },
            async move {
                started.send(()).unwrap();
                let _held = held;
                std::future::pending::<Result<(), CallError>>().await
            },
            done,
        );
        let replace = async {
            running.await.unwrap();
            replace_lease(&runtime, &other).await;
        };
        let (result, ()) = tokio::time::timeout(Duration::from_secs(5), async {
            tokio::join!(action, replace)
        })
        .await
        .unwrap();
        let Err(CallError::Tool(error)) = result else {
            panic!("{result:?}");
        };
        assert_eq!(error.name, ErrorName::LeaseRequired);
        assert!(dropped.await.is_err());
        assert!(!desk.status(&me).held_by_me);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A refusal that reads the lock state, once the action asks it, and finds nothing to
    /// refuse.
    #[expect(
        clippy::unused_async,
        reason = "the desk clears the last lock read before it polls the refusal, so the read must happen when the refusal is polled, not when it is made"
    )]
    async fn read_lock(desk: &Desk) -> Option<ToolError> {
        desk.lock_read(Instant::now());
        None
    }

    /// Runs an action whose work says its call is going out when `sends`, then waits,
    /// while `cut` stops it or moves the lease. Returns how it ended.
    async fn cut_short(
        desk: &Desk,
        sends: bool,
        cut: impl Future<Output = ()>,
    ) -> Result<Worked<()>, CallError> {
        let me = session(1);
        let (started, running) = tokio::sync::oneshot::channel::<()>();
        let action = desk.act(
            &me,
            read_lock(desk),
            async move {
                if sends {
                    desk.sending().unwrap();
                }
                started.send(()).unwrap();
                std::future::pending::<Result<(), CallError>>().await
            },
            ready,
        );
        let cut = async {
            running.await.unwrap();
            cut.await;
        };
        let (result, ()) =
            tokio::time::timeout(Duration::from_secs(5), async { tokio::join!(action, cut) })
                .await
                .unwrap();
        result
    }

    #[tokio::test]
    async fn a_stop_or_a_moved_lease_after_sending_is_no_refusal_and_still_ends_the_lease() {
        let me = session(1);
        let dir = crate::test_support::fresh_dir("desk-act-sent");
        let desk = Desk::start(&env(&dir));
        let other = Desk::start(&env(&dir));
        let runtime = RuntimeDir::of(&env(&dir)).unwrap();
        let take = async || {
            other.release(&me).await;
            desk.acquire(&me, "me/1", ready(None), ready(None))
                .await
                .unwrap();
        };
        take().await;
        // An action that sent, was answered and finished leaves nothing for the next one.
        let sends = async {
            desk.sending().unwrap();
            desk.answered(Outcome::uncertain(Some(true), None, String::new()));
            Ok(())
        };
        desk.act(&me, read_lock(&desk), sends, done).await.unwrap();
        let unsent = cut_short(&desk, false, replace_lease(&runtime, &other)).await;
        assert!(
            matches!(&unsent, Err(CallError::Tool(error)) if error.name == ErrorName::LeaseRequired),
            "{unsent:?}"
        );
        take().await;
        let replaced = cut_short(&desk, true, replace_lease(&runtime, &other)).await;
        let Ok(Worked::CutAfterSending(moved, None)) = replaced else {
            panic!("{replaced:?}");
        };
        assert_eq!(moved.name, ErrorName::LeaseRequired);
        assert!(moved.detail.contains("may have taken effect"), "{moved:?}");
        assert!(!desk.status(&me).held_by_me);
        take().await;
        let stop = async { runtime.stop().unwrap() };
        let stopped = cut_short(&desk, true, stop).await;
        let Ok(Worked::CutAfterSending(error, None)) = stopped else {
            panic!("{stopped:?}");
        };
        assert_eq!(error.name, ErrorName::Stopped);
        assert!(error.detail.contains("may have taken effect"), "{error:?}");
        assert!(released(&desk).await);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn the_checkpoint_refuses_what_would_refuse_an_action_now() {
        let me = session(1);
        let dir = crate::test_support::fresh_dir("desk-checkpoint");
        let desk = Desk::start(&env(&dir));
        let runtime = RuntimeDir::of(&env(&dir)).unwrap();
        let checked = |session: &Session| desk.checkpoint(session).map_err(|error| error.name);
        desk.acquire(&me, "me/1", ready(None), ready(None))
            .await
            .unwrap();
        assert_eq!(checked(&me), Ok(()));
        assert_eq!(checked(&session(2)), Err(ErrorName::LeaseRequired));
        let marker = runtime.path().join("input-dirty");
        std::fs::write(&marker, "").unwrap();
        assert_eq!(checked(&me), Err(ErrorName::RecoveryRequired));
        std::fs::remove_file(&marker).unwrap();
        // Read from the file, before the stop watcher has seen it.
        runtime.stop().unwrap();
        assert_eq!(checked(&me), Err(ErrorName::Stopped));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn the_checkpoint_refuses_a_replaced_lease_file() {
        let me = session(1);
        let dir = crate::test_support::fresh_dir("desk-checkpoint-lease");
        let desk = Desk::start(&env(&dir));
        desk.acquire(&me, "me/1", ready(None), ready(None))
            .await
            .unwrap();
        let lease = RuntimeDir::of(&env(&dir)).unwrap().path().join("lease");
        std::fs::remove_file(&lease).unwrap();
        std::fs::write(&lease, "").unwrap();
        let moved = desk.checkpoint(&me).unwrap_err();
        assert_eq!(moved.name, ErrorName::LeaseRequired);
        assert!(moved.detail.contains("removed or replaced"), "{moved:?}");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn losing_the_stop_watcher_mid_action_says_to_restart() {
        let me = session(1);
        let dir = crate::test_support::fresh_dir("desk-act-gone");
        let desk = Desk::start(&env(&dir));
        let runtime = RuntimeDir::of(&env(&dir)).unwrap();
        desk.acquire(&me, "me/1", ready(None), ready(None))
            .await
            .unwrap();
        let (started, running) = tokio::sync::oneshot::channel::<()>();
        let action = desk.act(
            &me,
            async { None },
            async move {
                started.send(()).unwrap();
                std::future::pending::<Result<(), CallError>>().await
            },
            done,
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
        let me = session(1);
        let dir = crate::test_support::fresh_dir("desk-stop");
        let desk = Desk::start(&env(&dir));
        let runtime = RuntimeDir::of(&env(&dir)).unwrap();
        desk.acquire(&me, "me/1", ready(None), ready(None))
            .await
            .unwrap();
        runtime.stop().unwrap();
        assert!(released(&desk).await);
        assert_eq!(lease::holder(&runtime), None);
        assert_eq!(
            desk.acquire(&me, "me/1", ready(None), ready(None))
                .await
                .unwrap_err()
                .name,
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
            desk.acquire(&me, "me/1", ready(None), ready(None))
                .await
                .unwrap_err()
                .name,
            ErrorName::RecoveryRequired
        );
        std::fs::remove_file(runtime.path().join("input-dirty")).unwrap();
        desk.acquire(&me, "me/1", ready(None), ready(None))
            .await
            .unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn removing_the_runtime_directory_gives_the_lease_up_for_good() {
        let me = session(1);
        let dir = crate::test_support::fresh_dir("desk-removed");
        let desk = Desk::start(&env(&dir));
        let runtime = RuntimeDir::of(&env(&dir)).unwrap();
        desk.acquire(&me, "me/1", ready(None), ready(None))
            .await
            .unwrap();
        std::fs::remove_dir_all(runtime.path()).unwrap();
        assert!(released(&desk).await);
        // A new directory has a new lock file, which no stop could reach for this server.
        let error = desk
            .acquire(&me, "me/1", ready(None), ready(None))
            .await
            .unwrap_err();
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
        let me = session(1);
        let dir = crate::test_support::fresh_dir("desk-unreadable");
        let runtime = RuntimeDir::of(&env(&dir)).unwrap();
        runtime.create().unwrap();
        let desk = Desk::start(&env(&dir));
        let mode = |bits| std::fs::Permissions::from_mode(bits);
        std::fs::set_permissions(runtime.path(), mode(0o000)).unwrap();
        let error = desk
            .acquire(&me, "me/1", ready(None), ready(None))
            .await
            .unwrap_err();
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
        let me = session(1);
        let desk = Desk::start(&Env::default());
        let error = desk
            .acquire(&me, "me/1", ready(None), ready(None))
            .await
            .unwrap_err();
        assert_eq!(error.name, ErrorName::NiriUnavailable);
        assert_eq!(error.detail, "NIRI_SOCKET is not set");
        assert_eq!(
            desk.status(&me),
            LeaseStatus {
                held_by_me: false,
                holder: None,
                error: Some("NIRI_SOCKET is not set".to_owned())
            }
        );
    }

    #[tokio::test]
    async fn one_session_holds_the_lease_and_another_can_neither_take_nor_release_it() {
        let me = session(1);
        let other = session(2);
        let dir = crate::test_support::fresh_dir("desk-sessions");
        let desk = Desk::start(&env(&dir));
        let holder = desk
            .acquire(&me, "me/1", ready(None), ready(Some(5)))
            .await
            .unwrap();
        let refused = desk
            .acquire(&other, "other/2", ready(None), ready(None))
            .await
            .unwrap_err();
        assert_eq!(refused.name, ErrorName::LeaseHeld);
        assert!(refused.detail.contains("(me/1)"), "{}", refused.detail);
        assert!(!desk.release(&other).await);
        assert_eq!(
            desk.status(&other),
            LeaseStatus {
                held_by_me: false,
                holder: Some(holder.clone()),
                error: None
            }
        );
        assert!(desk.status(&me).held_by_me);
        assert_eq!(desk.users_window(&other), None);
        assert_eq!(desk.users_window(&me), Some(5));
        assert_eq!(act_as(&desk, &other).await, Err(ErrorName::LeaseRequired));
        assert_eq!(act_as(&desk, &me).await, Ok(7));
        // Released by its owner, the lease goes to whoever takes it next, with nothing of
        // the last owner's.
        assert!(desk.release(&me).await);
        assert_eq!(
            desk.acquire(&other, "other/2", ready(None), ready(None))
                .await
                .unwrap()
                .label,
            "other/2"
        );
        assert_eq!(desk.users_window(&other), None);
        assert_eq!(act_as(&desk, &me).await, Err(ErrorName::LeaseRequired));
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// An action of `session` whose work returns 7.
    async fn act_as(desk: &Desk, session: &Session) -> Result<u8, ErrorName> {
        desk.act(session, async { None }, async { Ok(7) }, done)
            .await
            .map_err(|error| match error {
                CallError::Tool(error) => error.name,
                CallError::InvalidArguments(message) => panic!("{message}"),
            })
    }

    #[tokio::test]
    async fn a_session_without_the_lease_is_refused_while_the_owners_action_runs() {
        let me = session(1);
        let other = session(2);
        let dir = crate::test_support::fresh_dir("desk-sessions-busy");
        let desk = Desk::start(&env(&dir));
        desk.acquire(&me, "me/1", ready(None), ready(None))
            .await
            .unwrap();
        let (started, running) = tokio::sync::oneshot::channel::<()>();
        let owners = desk.act(
            &me,
            async { None },
            async move {
                started.send(()).unwrap();
                std::future::pending::<Result<(), CallError>>().await
            },
            done,
        );
        let others = async {
            running.await.unwrap();
            tokio::time::timeout(Duration::from_secs(1), act_as(&desk, &other)).await
        };
        tokio::select! {
            _ = owners => panic!("the owner's action ended"),
            refused = others => assert_eq!(refused.unwrap(), Err(ErrorName::LeaseRequired)),
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn another_sessions_acquire_is_refused_at_once_while_the_owners_action_runs() {
        let me = session(1);
        let other = session(2);
        let dir = crate::test_support::fresh_dir("desk-sessions-acquire");
        let desk = Desk::start(&env(&dir));
        desk.acquire(&me, "me/1", ready(None), ready(None))
            .await
            .unwrap();
        let (started, running) = tokio::sync::oneshot::channel::<()>();
        let owners = desk.act(
            &me,
            async { None },
            async move {
                started.send(()).unwrap();
                std::future::pending::<Result<(), CallError>>().await
            },
            done,
        );
        let others = async {
            running.await.unwrap();
            let acquire = desk.acquire(&other, "other/2", ready(None), ready(None));
            tokio::time::timeout(Duration::from_millis(100), acquire).await
        };
        tokio::select! {
            _ = owners => panic!("the owner's action ended"),
            refused = others => {
                let error = refused.expect("waited for the owner's action").unwrap_err();
                assert_eq!(error.name, ErrorName::LeaseHeld);
                assert!(error.detail.contains("(me/1)"), "{}", error.detail);
            }
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn acquire_asks_for_readiness_and_focus_once_the_running_action_has_ended() {
        let me = session(1);
        let dir = crate::test_support::fresh_dir("desk-acquire-fresh");
        let desk = Desk::start(&env(&dir));
        let action = desk.seat.lease.lock().await;
        let asked = std::cell::Cell::new(false);
        let refusal = async {
            asked.set(true);
            Some(ToolError::new(ErrorName::ScreenLocked, "locked meanwhile"))
        };
        let acquire = desk.acquire(&me, "me/1", refusal, ready(None));
        let release = async {
            tokio::time::sleep(Duration::from_millis(20)).await;
            assert!(!asked.get(), "asked before the running action ended");
            drop(action);
        };
        let (result, ()) = tokio::join!(acquire, release);
        assert_eq!(result.unwrap_err().name, ErrorName::ScreenLocked);
        // The refusal gave the lock it had taken back.
        let runtime = RuntimeDir::of(&env(&dir)).unwrap();
        assert_eq!(lease::holder(&runtime), None);
        drop(Lease::acquire(&runtime, "another server").unwrap());
        let focused = ready(Some(9));
        desk.acquire(&me, "me/1", async { None }, focused)
            .await
            .unwrap();
        assert_eq!(desk.users_window(&me), Some(9));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn another_servers_lock_refuses_before_readiness_is_asked() {
        let me = session(1);
        let dir = crate::test_support::fresh_dir("desk-acquire-locked");
        let desk = Desk::start(&env(&dir));
        let runtime = RuntimeDir::of(&env(&dir)).unwrap();
        let theirs = Lease::acquire(&runtime, "them/2").unwrap();
        let refusal = async { panic!("readiness asked while another server holds the lock") };
        let error = desk
            .acquire(&me, "me/1", refusal, ready(None))
            .await
            .unwrap_err();
        assert_eq!(error.name, ErrorName::LeaseHeld);
        assert!(error.detail.contains("(them/2)"), "{}", error.detail);
        drop(theirs);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn a_stop_or_the_sessions_end_during_readiness_grants_nothing() {
        let me = session(1);
        let dir = crate::test_support::fresh_dir("desk-acquire-late");
        let desk = Desk::start(&env(&dir));
        let runtime = RuntimeDir::of(&env(&dir)).unwrap();
        let stopping = async {
            runtime.stop().unwrap();
            None
        };
        let error = desk
            .acquire(&me, "me/1", stopping, ready(None))
            .await
            .unwrap_err();
        assert_eq!(error.name, ErrorName::Stopped);
        assert_eq!(lease::holder(&runtime), None);
        runtime.resume().unwrap();
        let ending = async {
            me.end();
            None
        };
        let ended = desk
            .acquire(&me, "me/1", ready(None), ending)
            .await
            .unwrap_err();
        assert_eq!(ended.detail, session_ended().detail);
        assert!(desk.seat.owned_by(me.id()).is_none());
        assert_eq!(lease::holder(&runtime), None);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A refusal that never answers, like a prompt the user hasn't answered, and a receiver
    /// that says once it is being waited for, so a test acts only after that.
    fn unanswered() -> (
        impl Future<Output = Option<ToolError>>,
        tokio::sync::oneshot::Receiver<()>,
    ) {
        let (asked, waiting) = tokio::sync::oneshot::channel();
        let refusal = async move {
            asked.send(()).ok();
            std::future::pending::<Option<ToolError>>().await
        };
        (refusal, waiting)
    }

    /// A refusal can wait up to a minute for the user's answer to the lease prompt; a stop
    /// or the client leaving meanwhile must end it, not leave it holding the lease's lock.
    #[tokio::test]
    async fn a_stop_ends_a_refusal_still_waiting_for_the_user() {
        let me = session(1);
        let dir = crate::test_support::fresh_dir("desk-acquire-waiting");
        let desk = Desk::start(&env(&dir));
        let runtime = RuntimeDir::of(&env(&dir)).unwrap();
        let (waiting, asked) = unanswered();
        let acquire = desk.acquire(&me, "me/1", waiting, ready(None));
        let stop = async {
            asked.await.unwrap();
            runtime.stop().unwrap();
        };
        let within = Duration::from_secs(5);
        let (stopped, ()) = tokio::join!(tokio::time::timeout(within, acquire), stop);
        let error = stopped.unwrap().unwrap_err();
        assert_eq!(error.detail, stopped_before_grant().detail);
        assert_eq!(lease::holder(&runtime), None);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn the_sessions_end_ends_a_refusal_still_waiting_for_the_user() {
        let me = session(1);
        let within = Duration::from_secs(5);
        let dir = crate::test_support::fresh_dir("desk-acquire-waiting-end");
        let desk = Desk::start(&env(&dir));
        let runtime = RuntimeDir::of(&env(&dir)).unwrap();
        let (waiting, asked) = unanswered();
        let acquire = desk.acquire(&me, "me/1", waiting, ready(None));
        let end = async {
            asked.await.unwrap();
            me.end();
        };
        let (ended, ()) = tokio::join!(tokio::time::timeout(within, acquire), end);
        assert_eq!(ended.unwrap().unwrap_err().detail, session_ended().detail);
        assert_eq!(lease::holder(&runtime), None);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// While one session waits for its user's Allow, others have no lease to give up and
    /// must not wait a minute for that answer, and the record shows the lease as pending
    /// until it is granted.
    #[tokio::test]
    async fn a_session_waiting_for_its_user_holds_up_no_other_session() {
        let (me, other) = (session(1), session(2));
        let dir = crate::test_support::fresh_dir("desk-acquire-pending");
        let desk = Desk::start(&env(&dir));
        let runtime = RuntimeDir::of(&env(&dir)).unwrap();
        let (allow, allowed) = tokio::sync::oneshot::channel::<()>();
        let (asked, waiting) = tokio::sync::oneshot::channel();
        let refusal = async move {
            asked.send(()).unwrap();
            allowed.await.ok();
            None
        };
        let acquire = desk.acquire(&me, "me/1", refusal, ready(None));
        let others = async {
            waiting.await.unwrap();
            assert!(lease::holder(&runtime).unwrap().pending);
            let within = Duration::from_millis(500);
            let released = tokio::time::timeout(within, release_and_end(&desk, &other)).await;
            assert_eq!(released, Ok(false));
            allow.send(()).unwrap();
        };
        let (holder, ()) = tokio::join!(acquire, others);
        let holder = holder.unwrap();
        assert!(!holder.pending);
        assert_eq!(lease::holder(&runtime), Some(holder));
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Releases `session`'s lease and ends it. Returns whether it held the lease.
    async fn release_and_end(desk: &Desk, session: &Session) -> bool {
        let released = desk.release(session).await;
        desk.end_session(session).await;
        released
    }

    #[tokio::test]
    async fn another_session_releases_and_ends_at_once_while_the_owners_action_runs() {
        let me = session(1);
        let other = session(2);
        let dir = crate::test_support::fresh_dir("desk-sessions-release");
        let desk = Desk::start(&env(&dir));
        desk.acquire(&me, "me/1", ready(None), ready(None))
            .await
            .unwrap();
        let (started, running) = tokio::sync::oneshot::channel::<()>();
        let owners = desk.act(
            &me,
            async { None },
            async move {
                started.send(()).unwrap();
                std::future::pending::<Result<(), CallError>>().await
            },
            done,
        );
        let others = async {
            running.await.unwrap();
            tokio::time::timeout(Duration::from_secs(1), release_and_end(&desk, &other)).await
        };
        tokio::select! {
            _ = owners => panic!("the owner's action ended"),
            released = others => assert_eq!(released, Ok(false)),
        }
        assert!(desk.status(&me).held_by_me);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// The lease file can be replaced while the user is asked, and another server can take
    /// the new one. An Allow then grants nothing and leaves that server's record alone.
    #[tokio::test]
    async fn an_allow_after_the_lease_file_was_replaced_grants_nothing() {
        let me = session(1);
        let dir = crate::test_support::fresh_dir("desk-acquire-replaced");
        let desk = Desk::start(&env(&dir));
        let other = Desk::start(&env(&dir));
        let runtime = RuntimeDir::of(&env(&dir)).unwrap();
        let (allow, allowed) = tokio::sync::oneshot::channel::<()>();
        let (asked, waiting) = tokio::sync::oneshot::channel();
        let refusal = async move {
            asked.send(()).unwrap();
            allowed.await.ok();
            None
        };
        let acquire = desk.acquire(&me, "me/1", refusal, ready(None));
        let replace = async {
            waiting.await.unwrap();
            replace_lease(&runtime, &other).await;
            allow.send(()).unwrap();
        };
        let (granted, ()) = tokio::join!(acquire, replace);
        let error = granted.unwrap_err();
        assert!(error.detail.contains("removed or replaced"), "{error:?}");
        assert!(!desk.status(&me).held_by_me);
        let holder = lease::holder(&runtime).unwrap();
        assert_eq!((holder.label.as_str(), holder.pending), ("other/2", false));
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A screenshot without the lease can take longer than an acquire and a release: one
    /// that started before the session took the lease belongs to no stretch it still has,
    /// and one that ends after the session did gets no ref either.
    #[tokio::test]
    async fn a_screenshot_started_before_the_lease_gets_no_ref_after_it() {
        let me = session(1);
        let dir = crate::test_support::fresh_dir("desk-view-stretch");
        let desk = Desk::start(&env(&dir));
        let shot = crate::test_support::shot;
        let started = desk.stretch(&me);
        desk.acquire(&me, "me/1", ready(None), ready(None))
            .await
            .unwrap();
        assert!(desk.release(&me).await);
        assert_eq!(desk.remember_view(&me, started, shot()), None);
        let id = desk.remember_view(&me, desk.stretch(&me), shot()).unwrap();
        assert!(desk.zoomable(&me, &id).is_ok());
        // A session that never took the lease is still in its first stretch when it ends.
        let other = session(2);
        let late = desk.stretch(&other);
        desk.end_session(&other).await;
        assert_eq!(desk.remember_view(&other, late, shot()), None);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn refs_belong_to_the_session_that_holds_the_lease() {
        let me = session(1);
        let other = session(2);
        let dir = crate::test_support::fresh_dir("desk-sessions-refs");
        let desk = Desk::start(&env(&dir));
        desk.acquire(&me, "me/1", ready(None), ready(None))
            .await
            .unwrap();
        assert_eq!(desk.ref_lease(&other), None);
        let lease = desk.ref_lease(&me).unwrap();
        let id = desk.remember(lease, crate::test_support::shot()).unwrap();
        assert!(desk.shot(&me, &id).is_ok());
        assert_eq!(
            desk.shot(&other, &id).unwrap_err().name,
            ErrorName::RefInvalid
        );
        // The lease changing hands drops them, even for the next owner.
        desk.release(&me).await;
        desk.acquire(&other, "other/2", ready(None), ready(None))
            .await
            .unwrap();
        assert_eq!(
            desk.shot(&other, &id).unwrap_err().name,
            ErrorName::RefInvalid
        );
        assert_eq!(desk.shot(&me, &id).unwrap_err().name, ErrorName::RefInvalid);
        assert_eq!(
            desk.element(&me, "elem-1").unwrap_err().name,
            ErrorName::ElementStale
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A phase of an action.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Phase {
        Refusal,
        Work,
        Finish,
    }

    #[tokio::test]
    async fn ending_a_session_drops_its_action_in_any_phase_and_frees_the_lease() {
        for stalled in [Phase::Refusal, Phase::Work, Phase::Finish] {
            let (me, other) = (session(1), session(2));
            let dir = crate::test_support::fresh_dir("desk-ended");
            let desk = Desk::start(&env(&dir));
            desk.acquire(&me, "me/1", ready(None), ready(None))
                .await
                .unwrap();
            // Another session ending leaves the lease alone.
            desk.end_session(&session(3)).await;
            assert!(desk.status(&me).held_by_me);
            let error = end_while_stalled(&desk, &me, stalled).await;
            assert_eq!(error.name, ErrorName::LeaseRequired);
            assert!(error.detail.contains("session ended"), "{error:?}");
            assert_eq!(desk.status(&other).holder, None);
            // The ended session can't take it again; another can.
            let refused = desk
                .acquire(&me, "me/1", ready(None), ready(None))
                .await
                .unwrap_err();
            assert_eq!(refused.name, ErrorName::LeaseRequired);
            desk.acquire(&other, "other/2", ready(None), ready(None))
                .await
                .unwrap();
            std::fs::remove_dir_all(dir).unwrap();
        }
    }

    /// Waits forever, once `started` is told, if it `stalls`.
    async fn stall(stalls: bool, started: &tokio::sync::Notify) {
        if stalls {
            started.notify_one();
            std::future::pending::<()>().await;
        }
    }

    /// Ends `me` while its action waits forever in `stalled`, and returns the action's
    /// error, failing unless both end within a second.
    async fn end_while_stalled(desk: &Desk, me: &Session, stalled: Phase) -> ToolError {
        let started = tokio::sync::Notify::new();
        let stall = |phase| stall(phase == stalled, &started);
        let refusal = async {
            stall(Phase::Refusal).await;
            None
        };
        let work = async {
            stall(Phase::Work).await;
            Ok(())
        };
        let action = desk.act(me, refusal, work, |_| stall(Phase::Finish));
        let end = async {
            started.notified().await;
            desk.end_session(me).await;
        };
        let (result, ()) =
            tokio::time::timeout(Duration::from_secs(1), async { tokio::join!(action, end) })
                .await
                .unwrap_or_else(|_| panic!("stalled in {stalled:?}"));
        match result {
            Err(CallError::Tool(error)) => error,
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn ending_a_session_drops_its_observation_and_frees_the_lease() {
        let me = session(1);
        let other = session(2);
        let dir = crate::test_support::fresh_dir("desk-ended-observe");
        let desk = Desk::start(&env(&dir));
        desk.acquire(&me, "me/1", ready(None), ready(None))
            .await
            .unwrap();
        let started = tokio::sync::Notify::new();
        let capture = async {
            started.notify_one();
            std::future::pending::<Result<(), CallError>>().await
        };
        let end = async {
            started.notified().await;
            desk.end_session(&me).await;
        };
        let observation = desk.observe(&me, capture);
        let (result, ()) = tokio::time::timeout(Duration::from_secs(1), async {
            tokio::join!(observation, end)
        })
        .await
        .unwrap();
        assert!(result.is_err());
        assert_eq!(desk.status(&other).holder, None);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// An observation of `session` that returns 7, if it ran within 50 ms.
    async fn observed(desk: &Desk, session: &Session) -> Option<u8> {
        let observation = desk.observe(session, async { Ok(7) });
        tokio::time::timeout(Duration::from_millis(50), observation)
            .await
            .ok()
            .map(Result::unwrap)
    }

    #[tokio::test]
    async fn only_the_owners_observations_wait_for_its_action() {
        let me = session(1);
        let other = session(2);
        let dir = crate::test_support::fresh_dir("desk-sessions-observe");
        let desk = Desk::start(&env(&dir));
        desk.acquire(&me, "me/1", ready(None), ready(None))
            .await
            .unwrap();
        let action = desk.seat.lease.lock().await;
        assert_eq!(observed(&desk, &other).await, Some(7));
        assert_eq!(observed(&desk, &me).await, None);
        drop(action);
        assert_eq!(observed(&desk, &me).await, Some(7));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
