//! Waiters: how an action tool watches niri's event stream for the effect it expects.
//!
//! A waiter is registered before the action is dispatched (plan §7). It starts from the
//! windows and workspaces of the replica at that moment and then applies every later event
//! itself, in order, so a check sees each intermediate state, such as focus passing
//! through another window, which a snapshot of the replica could skip. A disconnect ends
//! every wait, because the state it was built on is gone.

use std::collections::HashMap;
use std::time::Duration;

use niri_ipc::state::{EventStreamState, EventStreamStatePart as _, WindowsState, WorkspacesState};
use niri_ipc::{Event, Window, Workspace};
use tokio::sync::broadcast;
use tokio::time::Instant;

/// What the event stream's task tells waiters.
#[derive(Debug, Clone)]
pub(crate) enum Update {
    /// An event, numbered across connections, after the replica applied it.
    Event(u64, Box<Event>),
    /// The connection ended, and with it the replica.
    Reset,
}

/// The windows and workspaces as one waiter sees them.
#[derive(Debug, Default)]
pub(crate) struct View {
    windows: WindowsState,
    workspaces: WorkspacesState,
}

impl View {
    pub(crate) fn of(state: &EventStreamState) -> Self {
        Self {
            windows: WindowsState {
                windows: state.windows.windows.clone(),
            },
            workspaces: WorkspacesState {
                workspaces: state.workspaces.workspaces.clone(),
            },
        }
    }

    fn apply(&mut self, event: Event) {
        if let Some(event) = self.workspaces.apply(event) {
            self.windows.apply(event);
        }
    }

    pub(crate) const fn windows(&self) -> &HashMap<u64, Window> {
        &self.windows.windows
    }

    pub(crate) const fn workspaces(&self) -> &HashMap<u64, Workspace> {
        &self.workspaces.workspaces
    }

    /// The window with keyboard focus, if focus is on a window.
    pub(crate) fn focused_window(&self) -> Option<u64> {
        self.windows()
            .values()
            .find(|window| window.is_focused)
            .map(|window| window.id)
    }
}

/// How a wait ended.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Waited<T> {
    /// The check returned this.
    Done(T),
    Timeout,
    /// The stream can't say any more: why.
    Lost(String),
}

#[derive(Debug)]
pub(crate) struct Waiter {
    view: View,
    /// The number of the last event in `view`.
    seq: u64,
    updates: broadcast::Receiver<Update>,
}

impl Waiter {
    pub(super) const fn new(view: View, seq: u64, updates: broadcast::Receiver<Update>) -> Self {
        Self { view, seq, updates }
    }

    /// The state as of the last event applied.
    pub(crate) const fn view(&self) -> &View {
        &self.view
    }

    /// Calls `check` on the current view, then after every event, until it returns a value,
    /// `limit` passes, or the stream is lost.
    pub(crate) async fn until<T>(
        &mut self,
        limit: Duration,
        mut check: impl FnMut(&View) -> Option<T>,
    ) -> Waited<T> {
        let deadline = Instant::now() + limit;
        loop {
            if let Some(done) = check(&self.view) {
                return Waited::Done(done);
            }
            if let Err(reason) = self.next(deadline).await {
                return reason;
            }
        }
    }

    /// Applies the next event the view doesn't hold yet.
    async fn next<T>(&mut self, deadline: Instant) -> Result<(), Waited<T>> {
        loop {
            let update = match tokio::time::timeout_at(deadline, self.updates.recv()).await {
                Err(_) => return Err(Waited::Timeout),
                Ok(Ok(update)) => update,
                Ok(Err(broadcast::error::RecvError::Lagged(missed))) => {
                    return Err(Waited::Lost(format!(
                        "missed {missed} of niri's events while waiting"
                    )));
                }
                Ok(Err(broadcast::error::RecvError::Closed)) => {
                    return Err(Waited::Lost("niri's event stream stopped".to_owned()));
                }
            };
            match update {
                Update::Reset => {
                    return Err(Waited::Lost(
                        "niri's event stream disconnected while waiting".to_owned(),
                    ));
                }
                // Already in the view it started from.
                Update::Event(seq, _) if seq <= self.seq => {}
                Update::Event(seq, event) => {
                    self.seq = seq;
                    self.view.apply(*event);
                    return Ok(());
                }
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use serde_json::json;

    use super::*;

    /// A window on workspace `workspace`, focused or not, as niri describes it.
    pub(crate) fn window(id: u64, app_id: Option<&str>, workspace: u64, focused: bool) -> Window {
        serde_json::from_value(json!({
            "id": id, "title": "t", "app_id": app_id, "pid": 1, "workspace_id": workspace,
            "is_focused": focused, "is_floating": false, "is_urgent": false,
            "layout": {
                "pos_in_scrolling_layout": null, "tile_size": [1.0, 1.0],
                "window_size": [1, 1], "tile_pos_in_workspace_view": null,
                "window_offset_in_tile": [0.0, 0.0]
            }
        }))
        .unwrap()
    }

    pub(crate) fn workspace(id: u64, focused: bool) -> Workspace {
        serde_json::from_value(json!({
            "id": id, "idx": id, "name": null, "output": "DP-1", "is_urgent": false,
            "is_active": focused, "is_focused": focused, "active_window_id": null
        }))
        .unwrap()
    }

    /// A view of `windows` on workspaces 1 (focused) and 2.
    pub(crate) fn view(windows: Vec<Window>) -> View {
        let mut view = View::default();
        view.apply(Event::WorkspacesChanged {
            workspaces: vec![workspace(1, true), workspace(2, false)],
        });
        view.apply(Event::WindowsChanged { windows });
        view
    }

    /// A waiter on `view` after event `seq`, and the task's end of its channel.
    pub(crate) fn waiter(view: View, seq: u64) -> (Waiter, broadcast::Sender<Update>) {
        let (sender, updates) = broadcast::channel(16);
        (Waiter::new(view, seq, updates), sender)
    }

    fn focus(id: u64) -> Event {
        Event::WindowFocusChanged { id: Some(id) }
    }

    const LIMIT: Duration = Duration::from_secs(5);

    #[tokio::test(start_paused = true)]
    async fn checks_the_current_view_first_and_then_every_event_in_order() {
        let (mut waiter, sender) = waiter(view(vec![window(1, None, 1, true)]), 3);
        assert_eq!(
            waiter.until(LIMIT, View::focused_window).await,
            Waited::Done(1)
        );
        let mut seen = Vec::new();
        // An event the view already holds, then focus passing through 2 on its way to 3.
        for (seq, event) in [(3, focus(9)), (4, focus(2)), (5, focus(3))] {
            sender.send(Update::Event(seq, Box::new(event))).unwrap();
        }
        sender
            .send(Update::Event(
                6,
                Box::new(Event::WindowOpenedOrChanged {
                    window: window(3, None, 1, true),
                }),
            ))
            .unwrap();
        let ended = waiter
            .until(LIMIT, |view| {
                seen.push(view.focused_window());
                view.windows().contains_key(&3).then_some(())
            })
            .await;
        assert_eq!(ended, Waited::Done(()));
        // 2 and 3 aren't windows yet when focus moves to them, so nothing is focused.
        assert_eq!(seen, [Some(1), None, None, Some(3)]);
    }

    #[tokio::test(start_paused = true)]
    async fn ends_at_the_deadline_or_when_the_stream_is_lost() {
        let (mut waiter, sender) = waiter(view(Vec::new()), 0);
        assert_eq!(waiter.until(LIMIT, |_| None::<()>).await, Waited::Timeout);
        sender.send(Update::Reset).unwrap();
        assert!(matches!(
            waiter.until(LIMIT, |_| None::<()>).await,
            Waited::Lost(reason) if reason.contains("disconnected")
        ));
        for seq in 1..=20 {
            sender.send(Update::Event(seq, Box::new(focus(1)))).unwrap();
        }
        assert!(matches!(
            waiter.until(LIMIT, |_| None::<()>).await,
            Waited::Lost(reason) if reason.contains("missed")
        ));
        drop(sender);
        assert!(matches!(
            waiter.until(LIMIT, |_| None::<()>).await,
            Waited::Lost(reason) if reason.contains("stopped")
        ));
    }
}
