//! Waiters: how an action tool watches niri's event stream for the effect it expects.
//!
//! A waiter is registered before the action is dispatched (plan §7). It starts from the
//! windows and workspaces of the replica at that moment and then applies every later event
//! itself, in order, so a check sees each intermediate state, such as focus passing
//! through another window, which a snapshot of the replica could skip. A disconnect ends
//! every wait, because the state it was built on is gone.

use std::collections::{HashMap, HashSet};
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

/// How much an event can change what is shown where, least first.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Shift {
    /// Nothing shown moves: a title, an urgency, niri's bookkeeping.
    #[default]
    None,
    /// Windows: one opened, closed or focused, its layout, workspace or floating changed,
    /// or a workspace's active window changed.
    Windows,
    /// The whole view: a workspace activated or changed, or the overview opened or closed.
    View,
}

/// What niri's events since the last look changed (`Waiter::moved`).
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Moved {
    /// How much any of them can change what is shown where, the most of them.
    pub(crate) shift: Shift,
    /// The windows they moved, resized, floated, moved to another workspace or opened.
    pub(crate) relaid: HashSet<u64>,
    /// Those of `relaid` that opened.
    pub(crate) opened: HashSet<u64>,
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

    /// Applies what niri has sent by now (`catch_up`), and says what it changed. `Err`
    /// with the reason when the stream can't say.
    pub(crate) async fn moved(&mut self) -> Result<Moved, String> {
        let mut moved = Moved::default();
        let caught_up = self
            .catch_up(
                |view, event| {
                    moved.shift = moved.shift.max(shift(view, event));
                    let relaid = relaid_by(view.windows(), event);
                    let known = view.windows();
                    moved
                        .opened
                        .extend(relaid.iter().filter(|id| !known.contains_key(id)));
                    moved.relaid.extend(relaid);
                },
                |_| None::<()>,
            )
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
fn relaid(old: &Window, new: &Window) -> bool {
    old.layout != new.layout
        || old.workspace_id != new.workspace_id
        || old.is_floating != new.is_floating
}

/// The windows `event`, on top of `known`, moves, resizes, floats, moves to another
/// workspace or opens.
pub(super) fn relaid_by(known: &HashMap<u64, Window>, event: &Event) -> Vec<u64> {
    let changed = |window: &Window| known.get(&window.id).is_none_or(|old| relaid(old, window));
    match event {
        Event::WindowsChanged { windows } => windows
            .iter()
            .filter(|window| changed(window))
            .map(|window| window.id)
            .collect(),
        Event::WindowOpenedOrChanged { window } => {
            changed(window).then_some(window.id).into_iter().collect()
        }
        Event::WindowLayoutsChanged { changes } => changes
            .iter()
            .filter(|(id, layout)| known.get(id).is_none_or(|old| old.layout != *layout))
            .map(|(id, _)| *id)
            .collect(),
        Event::WorkspacesChanged { .. }
        | Event::WorkspaceUrgencyChanged { .. }
        | Event::WorkspaceActivated { .. }
        | Event::WorkspaceActiveWindowChanged { .. }
        | Event::WindowClosed { .. }
        | Event::WindowFocusChanged { .. }
        | Event::WindowFocusTimestampChanged { .. }
        | Event::WindowUrgencyChanged { .. }
        | Event::KeyboardLayoutsChanged { .. }
        | Event::KeyboardLayoutSwitched { .. }
        | Event::OverviewOpenedOrClosed { .. }
        | Event::ConfigLoaded { .. }
        | Event::ScreenshotCaptured { .. }
        // `events::IGNORED`: the stream drops these unread, so no waiter gets them. Reading
        // them here or in `shift` means taking them out of that list.
        | Event::CastsChanged { .. }
        | Event::CastStartedOrChanged { .. }
        | Event::CastStopped { .. } => Vec::new(),
    }
}

/// How much `event`, on top of `view`, can change what is shown where.
fn shift(view: &View, event: &Event) -> Shift {
    match event {
        Event::WindowOpenedOrChanged { window } => {
            let moved = view
                .windows()
                .get(&window.id)
                .is_none_or(|old| relaid(old, window) || old.is_focused != window.is_focused);
            if moved { Shift::Windows } else { Shift::None }
        }
        Event::WorkspacesChanged { .. }
        | Event::WorkspaceActivated { .. }
        | Event::OverviewOpenedOrClosed { .. } => Shift::View,
        Event::WorkspaceActiveWindowChanged { .. }
        | Event::WindowsChanged { .. }
        | Event::WindowClosed { .. }
        | Event::WindowFocusChanged { .. }
        | Event::WindowLayoutsChanged { .. } => Shift::Windows,
        Event::WorkspaceUrgencyChanged { .. }
        | Event::WindowFocusTimestampChanged { .. }
        | Event::WindowUrgencyChanged { .. }
        | Event::KeyboardLayoutsChanged { .. }
        | Event::KeyboardLayoutSwitched { .. }
        | Event::ConfigLoaded { .. }
        | Event::ScreenshotCaptured { .. }
        | Event::CastsChanged { .. }
        | Event::CastStartedOrChanged { .. }
        | Event::CastStopped { .. } => Shift::None,
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

    /// The view `events` make, in order.
    pub(crate) fn view_of(events: Vec<Event>) -> View {
        let mut view = View::default();
        for event in events {
            view.apply(event);
        }
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
    // refuses it, and while a gesture holds a button, one that changes the whole view stops
    // it; a title or urgency changing does neither.
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
            (moved_by((40.0, 30.0)), Shift::Windows),
            (
                Event::WindowOpenedOrChanged {
                    window: window(2, None, 1, false),
                },
                Shift::Windows,
            ),
            (
                Event::WorkspaceActivated {
                    id: 2,
                    focused: true,
                },
                Shift::View,
            ),
            (
                Event::WorkspaceActiveWindowChanged {
                    workspace_id: 1,
                    active_window_id: Some(2),
                },
                Shift::Windows,
            ),
            (Event::OverviewOpenedOrClosed { is_open: true }, Shift::View),
            (Event::WindowClosed { id: 1 }, Shift::Windows),
        ];
        let (mut watched, sender) = waiter(view(vec![floating()]), 0);
        for (seq, event) in (1..).zip(quiet) {
            sender.send(Update::Event(seq, Box::new(event))).unwrap();
        }
        assert_eq!(
            watched.moved().await.map(|moved| moved.shift),
            Ok(Shift::None)
        );
        for (event, shifted) in moving {
            let (mut fresh, updates) = waiter(view(vec![floating()]), 0);
            let what = format!("{event:?}");
            updates.send(Update::Event(1, Box::new(event))).unwrap();
            assert_eq!(
                fresh.moved().await.map(|moved| moved.shift),
                Ok(shifted),
                "{what}"
            );
            // Each event counts once: nothing new has come since.
            assert_eq!(fresh.moved().await, Ok(Moved::default()), "{what}");
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
            let moved = waiter.moved().await.map(|moved| moved.shift);
            let past = waiter.until(Duration::ZERO, |_| None::<()>).await;
            (moved, past)
        })
        .await;
        sending.abort();
        assert_eq!(looked, Ok((Ok(Shift::None), Waited::Timeout)));
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
