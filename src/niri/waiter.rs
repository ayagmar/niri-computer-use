//! Waiters: how an action tool watches niri's event stream for the effect it expects.
//!
//! A waiter is registered before the action is dispatched (plan §7). It starts from the
//! windows and workspaces of the replica at that moment and then applies every later event
//! itself, in order, so a check sees each intermediate state, such as focus passing
//! through another window, which a snapshot of the replica could skip. A disconnect ends
//! every wait, because the state it was built on is gone.

use std::collections::HashMap;
use std::time::Duration;

use niri_ipc::state::{
    EventStreamState, EventStreamStatePart as _, KeyboardLayoutsState, WindowsState,
    WorkspacesState,
};
use niri_ipc::{Event, Window, Workspace};
use tokio::sync::broadcast;
use tokio::time::Instant;

/// What the event stream's task tells waiters.
#[derive(Debug, Clone)]
pub(crate) enum Update {
    /// An event, numbered across connections, after the replica applied it.
    Event(u64, Box<Event>),
    /// The connection with this number ended, and with it its replica.
    Reset(u64),
}

/// Where the event stream is: the last event's number, counted across connections, and
/// the connection's, counted from 1.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Numbers {
    pub(crate) seq: u64,
    pub(crate) connection: u64,
}

/// The windows and workspaces as one waiter sees them.
#[derive(Debug, Default)]
pub(crate) struct View {
    windows: WindowsState,
    workspaces: WorkspacesState,
    keyboard_layouts: KeyboardLayoutsState,
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
            keyboard_layouts: KeyboardLayoutsState {
                keyboard_layouts: state.keyboard_layouts.keyboard_layouts.clone(),
            },
        }
    }

    fn apply(&mut self, event: Event) {
        if let Some(event) = self.workspaces.apply(event)
            && let Some(event) = self.windows.apply(event)
        {
            self.keyboard_layouts.apply(event);
        }
    }

    pub(crate) fn keyboard_group(&self) -> Option<u32> {
        self.keyboard_layouts
            .keyboard_layouts
            .as_ref()
            .map(|layouts| u32::from(layouts.current_idx))
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
    /// The last event in `view` and the connection it came from.
    numbers: Numbers,
    updates: broadcast::Receiver<Update>,
}

impl Waiter {
    pub(super) const fn new(
        view: View,
        numbers: Numbers,
        updates: broadcast::Receiver<Update>,
    ) -> Self {
        Self {
            view,
            numbers,
            updates,
        }
    }

    /// The state as of the last event applied.
    pub(crate) const fn view(&self) -> &View {
        &self.view
    }

    /// How many updates have arrived that the view hasn't taken yet.
    pub(crate) fn queued(&self) -> usize {
        self.updates.len()
    }

    /// Calls `check` on the current view, then after every event, until it returns a value,
    /// `limit` passes, or the stream is lost. Past `limit`, only what niri sent by then
    /// counts (`catch_up`).
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
            match self.next(deadline).await {
                Ok(true) => {}
                Ok(false) => break,
                Err(reason) => return Waited::Lost(reason),
            }
        }
        self.catch_up(|_, _| {}, check).await
    }

    /// Applies what niri has sent by now (`catch_up`), and says whether any of it can
    /// change what is shown where: a workspace activated or changed, the overview opened or
    /// closed, a window opened, closed, focused or made its workspace's active one, or a
    /// window's layout, workspace or floating changed. A title or urgency changing doesn't
    /// count. `Err` with the reason when the stream can't say.
    pub(crate) async fn moved(&mut self) -> Result<bool, String> {
        let mut moved = false;
        let caught_up = self
            .catch_up(|view, event| moved |= moves(view, event), |_| None::<()>)
            .await;
        match caught_up {
            Waited::Lost(reason) => Err(reason),
            Waited::Done(()) | Waited::Timeout => Ok(moved),
        }
    }

    /// Applies the next event the view doesn't hold yet, waiting until `deadline` for it:
    /// whether one came, or `Err` with why the stream can't say.
    async fn next(&mut self, deadline: Instant) -> Result<bool, String> {
        while Instant::now() < deadline {
            let Ok(received) = tokio::time::timeout_at(deadline, self.updates.recv()).await else {
                return Ok(false);
            };
            if self.take(received.map_err(|error| lost(&error))?, &mut |_, _| {})? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Applies what niri has sent by now. The stream's task gets one turn to pass on what
    /// it has already read; then the updates queued at that moment are applied in order,
    /// `seen` shown each new event with the view before it and `check` the view after it,
    /// until it returns a value (`Timeout` otherwise). Updates that come meanwhile are left
    /// for the next look, so a niri that keeps sending can't hold this one, and no timer
    /// runs: one would round up to the next millisecond tick.
    async fn catch_up<T>(
        &mut self,
        mut seen: impl FnMut(&View, &Event),
        mut check: impl FnMut(&View) -> Option<T>,
    ) -> Waited<T> {
        tokio::task::yield_now().await;
        for _ in 0..self.queued() {
            let received = match self.updates.try_recv() {
                Ok(update) => Ok(update),
                Err(broadcast::error::TryRecvError::Empty) => break,
                Err(broadcast::error::TryRecvError::Lagged(missed)) => {
                    Err(broadcast::error::RecvError::Lagged(missed))
                }
                Err(broadcast::error::TryRecvError::Closed) => {
                    Err(broadcast::error::RecvError::Closed)
                }
            };
            let applied = match received
                .map_err(|error| lost(&error))
                .and_then(|update| self.take(update, &mut seen))
            {
                Ok(applied) => applied,
                Err(reason) => return Waited::Lost(reason),
            };
            if let Some(done) = applied.then(|| check(&self.view)).flatten() {
                return Waited::Done(done);
            }
        }
        Waited::Timeout
    }

    /// Applies `update` when it is an event the view doesn't hold yet, showing it to `seen`
    /// with the view before it: whether it was one. `Err` with the reason once the view's
    /// connection has ended.
    fn take(
        &mut self,
        update: Update,
        seen: &mut impl FnMut(&View, &Event),
    ) -> Result<bool, String> {
        match update {
            Update::Reset(connection) if connection == self.numbers.connection => {
                Err("niri's event stream disconnected while waiting".to_owned())
            }
            // An earlier connection's end, or an event the view already holds.
            Update::Reset(_) => Ok(false),
            Update::Event(seq, _) if seq <= self.numbers.seq => Ok(false),
            Update::Event(seq, event) => {
                self.numbers.seq = seq;
                seen(&self.view, &event);
                self.view.apply(*event);
                Ok(true)
            }
        }
    }
}

/// Why the stream can't say, after `error` receiving from it.
fn lost(error: &broadcast::error::RecvError) -> String {
    match error {
        broadcast::error::RecvError::Lagged(missed) => {
            format!("missed {missed} of niri's events while waiting")
        }
        broadcast::error::RecvError::Closed => "niri's event stream stopped".to_owned(),
    }
}

/// Whether niri moved, resized or floated a window between `old` and `new`, or moved it to
/// another workspace.
pub(super) fn relaid(old: &Window, new: &Window) -> bool {
    old.layout != new.layout
        || old.workspace_id != new.workspace_id
        || old.is_floating != new.is_floating
}

/// Whether `event`, on top of `view`, can change what is shown where (see `Waiter::moved`).
fn moves(view: &View, event: &Event) -> bool {
    match event {
        Event::WindowOpenedOrChanged { window } => view
            .windows()
            .get(&window.id)
            .is_none_or(|old| relaid(old, window) || old.is_focused != window.is_focused),
        Event::WorkspacesChanged { .. }
        | Event::WorkspaceActivated { .. }
        | Event::WorkspaceActiveWindowChanged { .. }
        | Event::WindowsChanged { .. }
        | Event::WindowClosed { .. }
        | Event::WindowFocusChanged { .. }
        | Event::WindowLayoutsChanged { .. }
        | Event::OverviewOpenedOrClosed { .. } => true,
        Event::WorkspaceUrgencyChanged { .. }
        | Event::WindowFocusTimestampChanged { .. }
        | Event::WindowUrgencyChanged { .. }
        | Event::KeyboardLayoutsChanged { .. }
        | Event::KeyboardLayoutSwitched { .. }
        | Event::ConfigLoaded { .. }
        | Event::ScreenshotCaptured { .. }
        | Event::CastsChanged { .. }
        | Event::CastStartedOrChanged { .. }
        | Event::CastStopped { .. } => false,
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
        view_on(vec![workspace(1, true), workspace(2, false)], windows)
    }

    pub(crate) fn view_on(workspaces: Vec<Workspace>, windows: Vec<Window>) -> View {
        let mut view = View::default();
        view.apply(Event::WorkspacesChanged { workspaces });
        view.apply(Event::WindowsChanged { windows });
        view
    }

    /// A waiter on `view` after event `seq` of connection 2, and the task's end of its
    /// channel.
    pub(crate) fn waiter(view: View, seq: u64) -> (Waiter, broadcast::Sender<Update>) {
        let (sender, updates) = broadcast::channel(16);
        let numbers = Numbers { seq, connection: 2 };
        (Waiter::new(view, numbers, updates), sender)
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

    // Before an element aim goes out, any event that can change what is shown where
    // refuses it; a title or urgency changing doesn't.
    #[tokio::test]
    async fn a_change_to_what_is_shown_where_counts_as_moved_and_a_title_does_not() {
        let floating = || {
            let mut window = window(1, None, 1, true);
            window.layout.tile_pos_in_workspace_view = Some((20.0, 30.0));
            window
        };
        let retitled = Event::WindowOpenedOrChanged {
            window: Window {
                title: Some("another".to_owned()),
                ..floating()
            },
        };
        let moved_by = |pos| {
            let mut window = floating();
            window.layout.tile_pos_in_workspace_view = Some(pos);
            Event::WindowOpenedOrChanged { window }
        };
        let quiet = [
            retitled,
            Event::WindowUrgencyChanged {
                id: 1,
                urgent: true,
            },
        ];
        let moving = [
            moved_by((40.0, 30.0)),
            Event::WindowOpenedOrChanged {
                window: window(2, None, 1, false),
            },
            Event::WorkspaceActivated {
                id: 2,
                focused: true,
            },
            Event::WorkspaceActiveWindowChanged {
                workspace_id: 1,
                active_window_id: Some(2),
            },
            Event::OverviewOpenedOrClosed { is_open: true },
            Event::WindowClosed { id: 1 },
        ];
        let (mut watched, sender) = waiter(view(vec![floating()]), 0);
        for (seq, event) in (1..).zip(quiet) {
            sender.send(Update::Event(seq, Box::new(event))).unwrap();
        }
        assert_eq!(watched.moved().await, Ok(false));
        for event in moving {
            let (mut fresh, updates) = waiter(view(vec![floating()]), 0);
            let what = format!("{event:?}");
            updates.send(Update::Event(1, Box::new(event))).unwrap();
            assert_eq!(fresh.moved().await, Ok(true), "{what}");
            // Each event counts once: nothing new has come since.
            assert_eq!(fresh.moved().await, Ok(false), "{what}");
        }
        sender.send(Update::Reset(2)).unwrap();
        assert!(watched.moved().await.is_err());
    }

    #[tokio::test(start_paused = true)]
    async fn ends_at_the_deadline_or_when_the_stream_is_lost() {
        let (mut waiter, sender) = waiter(view(Vec::new()), 0);
        assert_eq!(waiter.until(LIMIT, |_| None::<()>).await, Waited::Timeout);
        // The end of the connection before the one the view came from changes nothing.
        sender.send(Update::Reset(1)).unwrap();
        assert_eq!(waiter.until(LIMIT, |_| None::<()>).await, Waited::Timeout);
        sender.send(Update::Reset(2)).unwrap();
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

    /// Native typing checks focus before every key pair; a timer tick per check cost about
    /// a millisecond each, most of a 100-key call. How long the calls take is checked only
    /// with `NCU_PERF=1`: a loaded machine can take 20 ms without any tick, and the paused
    /// clock fires a timer that is already due without moving, so it can't tell either.
    #[tokio::test]
    async fn a_zero_wait_answers_without_a_timer_tick() {
        let (mut waiter, sender) = waiter(view(vec![window(1, None, 1, true)]), 0);
        let started = std::time::Instant::now();
        for seq in 1..=100 {
            assert_eq!(
                waiter.until(Duration::ZERO, |_| None::<()>).await,
                Waited::Timeout
            );
            sender.send(Update::Event(seq, Box::new(focus(2)))).unwrap();
            assert_eq!(
                waiter.until(Duration::ZERO, |_| None::<()>).await,
                Waited::Timeout
            );
            assert_eq!(waiter.view().focused_window(), None);
        }
        if std::env::var_os("NCU_PERF").is_some_and(|perf| perf == "1") {
            let took = started.elapsed();
            assert!(took < Duration::from_millis(20), "{took:?}");
        }
    }

    /// Sends an urgency change for window 1 each time it gets to run, a million times.
    async fn keeps_sending(sender: broadcast::Sender<Update>) {
        for seq in 1..=1_000_000 {
            let urgent = Event::WindowUrgencyChanged {
                id: 1,
                urgent: seq % 2 == 1,
            };
            sender.send(Update::Event(seq, Box::new(urgent))).ok();
            tokio::task::yield_now().await;
        }
    }

    // The last look before input goes out, and a wait whose time is up, take what niri
    // sent by then and no more: a niri that sends one more event every time its task runs
    // can't hold them.
    #[tokio::test]
    async fn a_niri_that_keeps_sending_cant_hold_a_look_at_what_it_sent() {
        let (mut waiter, sender) = waiter(view(vec![window(1, None, 1, true)]), 0);
        let sending = tokio::spawn(keeps_sending(sender));
        let looked = tokio::time::timeout(LIMIT, async {
            let moved = waiter.moved().await;
            let past = waiter.until(Duration::ZERO, |_| None::<()>).await;
            (moved, past)
        })
        .await;
        sending.abort();
        assert_eq!(looked, Ok((Ok(false), Waited::Timeout)));
        assert_ne!(waiter.numbers.seq, 0);
    }

    /// The stream's task must get a turn first: an event it has already read but not yet
    /// passed on would otherwise reach the check only after the next key.
    #[tokio::test]
    async fn a_zero_wait_lets_the_stream_pass_on_what_it_has() {
        let (mut waiter, sender) = waiter(view(vec![window(1, None, 1, true)]), 0);
        tokio::spawn(async move {
            sender.send(Update::Event(1, Box::new(focus(2)))).unwrap();
        });
        assert_eq!(
            waiter
                .until(Duration::ZERO, |view| (view.focused_window() != Some(1))
                    .then_some(()))
                .await,
            Waited::Done(())
        );
    }
}
