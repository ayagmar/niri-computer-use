//! The change report's watch on niri's event stream: a waiter registered as an action's
//! work starts, which follows niri's events while the work and its evidence run and
//! afterwards says what changed (`changes::desktop`). It reads only the event stream, never
//! asks niri anything.

use std::collections::BTreeMap;
use std::time::Duration;

use niri_ipc::Window;
use tokio::time::Instant;

use super::{Observed, Outcome};
use crate::changes::{self, Changes, Scene};
use crate::error::ToolError;
use crate::niri::waiter::{Waited, Waiter};
use crate::niri::{self, Events};

/// The report ends once niri has sent no event that changes what it shows for this long,
/// counted from the start of the action, so an effect that comes a moment after the input,
/// such as a dialog opening after a click, is in it. The action's own wait counts towards
/// it.
pub(crate) const QUIET: Duration = Duration::from_millis(250);
/// At most this long after the action's own wait and its evidence, however busy niri is.
const EXTRA: Duration = Duration::from_millis(500);
/// After a wait that returned on niri's event, the rest of niri's batch has come once niri
/// sent no event of any kind for this long.
const GAP: Duration = Duration::from_millis(10);
/// At most this long for the rest of niri's batch.
const BATCH: Duration = Duration::from_millis(50);

/// A watch on niri's events from the start of an action.
#[derive(Debug)]
pub(crate) struct Recorder {
    waiter: Waiter,
    before: Scene,
    /// The scene as of the last event applied.
    now: Scene,
    /// Every window seen since that wasn't open before, as last seen.
    passing: BTreeMap<u64, Window>,
    /// When the last event that changed what the report shows came, or the watch started.
    last: Instant,
    /// Why the event stream ended, once it did.
    lost: Option<String>,
}

/// How long the report waits once the work ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Wait {
    /// Until niri has been quiet for `QUIET`: nothing else waited for the screen.
    Quiet,
    /// Until the rest of niri's batch: no event for `GAP`, at most `BATCH`. niri writes a
    /// refresh's events one at a time, and some come after the one a wait looks for, such
    /// as the urgency it clears on a window that took focus.
    Batch,
    /// Only for what niri already sent: after a settled screenshot waited for the screen,
    /// or a wait that ended on a timeout, long after niri's batch.
    Received,
}

/// Whether an action's report waits for quiet once the work ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Quiet {
    /// Always: input's effect is the app's to show, a moment after niri handled it.
    Always,
    /// `focus`: unless its own wait returned on niri's event, `focused` or `closed`. Focus
    /// is niri's own, and that wait watched it; the rest of niri's batch follows the event.
    UnlessSeen,
    /// `niri_action`: unless its own wait ended with `changed` or `closed`, a fixed 200 ms
    /// after the first change, or with `unchanged`, after a second without one. Its actions
    /// are niri's own, and niri's batch was in long before.
    UnlessSettled,
    /// Never: `niri_action`'s `Spawn` and `SpawnSh`, whose window comes later than this
    /// wait would cover; `wait_for` is for that.
    Never,
}

impl Quiet {
    /// How the report ends after an action that observed `observed`, with a settled
    /// screenshot when `shot`.
    pub(crate) const fn wait(self, shot: bool, observed: Observed) -> Wait {
        if shot {
            return Wait::Received;
        }
        match (self, observed) {
            (Self::UnlessSeen, Observed::Focused | Observed::Closed) => Wait::Batch,
            (Self::UnlessSettled, Observed::Changed | Observed::Unchanged | Observed::Closed)
            | (Self::Never, _) => Wait::Received,
            _ => Wait::Quiet,
        }
    }
}

impl Recorder {
    /// Starts watching, or says why niri's event stream can't be watched.
    pub(crate) async fn start(events: Events<'_>) -> Result<Self, ToolError> {
        let waiter = niri::waiter(events).await?;
        let before = Scene::of(waiter.view());
        Ok(Self {
            waiter,
            now: before.clone(),
            before,
            passing: BTreeMap::new(),
            last: Instant::now(),
            lost: None,
        })
    }

    /// Runs `work`, applying niri's events as they come meanwhile, so that the time it
    /// spends with niri quiet counts towards `QUIET`.
    pub(crate) async fn follow<T>(&mut self, work: impl Future<Output = T>) -> T {
        let mut work = std::pin::pin!(work);
        while self.lost.is_none() {
            tokio::select! {
                biased;
                done = &mut work => return done,
                _ = self.next(QUIET) => {}
            }
        }
        work.await
    }

    /// Applies niri's events as `wait` says, then brings `outcome`'s focus up to the view
    /// the report ends at, so a result never names two focused windows, and says what
    /// changed since the start; with `lost` saying why when the stream ended first, and
    /// then `focused_window` unknown, as focus may have moved since.
    pub(crate) async fn finish(mut self, wait: Wait, outcome: &mut Outcome) -> Changes {
        match wait {
            Wait::Quiet => self.quiet().await,
            Wait::Batch => self.batch().await,
            Wait::Received => {}
        }
        self.drain().await;
        let view = self.waiter.view();
        outcome.caught_up(view);
        if self.lost.is_some() {
            outcome.focused_window = None;
        }
        let mut changes = changes::desktop(&self.before, &Scene::of(view), &self.passing);
        changes.lost = self.lost;
        changes
    }

    /// Applies events until `QUIET` passed since the last one that changed what the report
    /// shows, or `EXTRA` from now. Those already received come first, so one that arrived
    /// as the work ended, after `QUIET` had passed, still restarts it.
    async fn quiet(&mut self) {
        let limit = Instant::now() + EXTRA;
        self.drain().await;
        while self.lost.is_none() {
            let left = (self.last + QUIET)
                .min(limit)
                .saturating_duration_since(Instant::now());
            if left.is_zero() {
                return;
            }
            self.next(left).await;
        }
    }

    /// Applies events until none came for `GAP`, or `BATCH` from now.
    async fn batch(&mut self) {
        let limit = Instant::now() + BATCH;
        while self.lost.is_none() {
            let left = GAP.min(limit.saturating_duration_since(Instant::now()));
            if left.is_zero() || !self.next(left).await {
                return;
            }
        }
    }

    /// Applies the events already received when it starts, and no more, so a niri that
    /// keeps sending can't hold it.
    async fn drain(&mut self) {
        for _ in 0..self.waiter.queued() {
            if self.lost.is_some() || !self.next(Duration::ZERO).await {
                return;
            }
        }
    }

    /// Applies the next event, waiting at most `limit` for it: whether one came. Only an
    /// event that changes what the report shows restarts the quiet, so niri's bookkeeping,
    /// such as a window's focus timestamp a moment after it took focus, can't extend it.
    /// The stream ending sets `lost`.
    async fn next(&mut self, limit: Duration) -> bool {
        let Self {
            waiter,
            before,
            passing,
            ..
        } = self;
        // The first check sees the view as it is; the second follows a new event.
        let mut checks = 0;
        let ended = waiter
            .until(limit, |view| {
                let new = view.windows().iter().filter(|(id, _)| !before.has(**id));
                passing.extend(new.map(|(id, window)| (*id, window.clone())));
                checks += 1;
                (checks > 1).then_some(())
            })
            .await;
        match ended {
            Waited::Done(()) => {
                let now = Scene::of(self.waiter.view());
                let shown = changes::desktop(&self.now, &now, &BTreeMap::new());
                if !shown.is_empty() {
                    self.last = Instant::now();
                }
                self.now = now;
                true
            }
            Waited::Timeout => false,
            Waited::Lost(reason) => {
                self.lost = Some(reason);
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use niri_ipc::{Event, Timestamp};
    use serde_json::json;

    use super::*;
    use crate::act::Observed;
    use crate::niri::waiter::Update;
    use crate::niri::waiter::tests::{view, waiter, window};

    type Sender = tokio::sync::broadcast::Sender<Update>;

    /// A watch started now on `windows`, and the outcome of an action that observed them.
    fn watching(windows: Vec<Window>) -> (Recorder, Outcome, Sender) {
        let (waiter, sender) = waiter(view(windows), 0);
        let outcome = Outcome::seen(Observed::Sent, waiter.view(), Vec::new());
        let before = Scene::of(waiter.view());
        let recorder = Recorder {
            waiter,
            now: before.clone(),
            before,
            passing: BTreeMap::new(),
            last: Instant::now(),
            lost: None,
        };
        (recorder, outcome, sender)
    }

    fn event(sender: &Sender, seq: u64, event: Event) {
        sender.send(Update::Event(seq, Box::new(event))).unwrap();
    }

    /// Sends `sent` after `delay`, as niri's first event since the watch started.
    async fn later(sender: Sender, delay: Duration, sent: Event) -> Sender {
        tokio::time::sleep(delay).await;
        event(&sender, 1, sent);
        sender
    }

    fn app() -> Window {
        window(1, Some("app"), 1, true)
    }

    #[tokio::test(start_paused = true)]
    async fn a_dialog_that_opens_a_moment_after_the_action_is_in_the_report() {
        let (recorder, mut outcome, sender) = watching(vec![app()]);
        outcome.window = Some((&app()).into());
        let opens = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            let dialog = window(5, Some("app"), 1, true);
            event(&sender, 1, Event::WindowOpenedOrChanged { window: dialog });
            // A window that comes and goes again within the report.
            tokio::time::sleep(Duration::from_millis(100)).await;
            let toast = window(6, Some("toast"), 1, false);
            event(&sender, 2, Event::WindowOpenedOrChanged { window: toast });
            event(&sender, 3, Event::WindowClosed { id: 6 });
            sender
        });
        let started = Instant::now();
        let changes = recorder.finish(Wait::Quiet, &mut outcome).await;
        assert_eq!(started.elapsed(), Duration::from_millis(200) + QUIET);
        assert_eq!(
            serde_json::to_value(&changes).unwrap(),
            json!({
                "opened": [
                    {"id": 5, "app_id": "app", "workspace_id": 1, "title": "t"},
                    {"id": 6, "app_id": "toast", "workspace_id": 1, "title": "t"}
                ],
                "closed": [{"id": 6, "app_id": "toast"}],
                "focus": {"from": 1, "to": 5},
            })
        );
        // The outcome names the same focused window as the report, not the one it saw.
        assert_eq!(outcome.focused_window, Some(5));
        assert_eq!(outcome.window.map(|window| window.is_focused), Some(false));
        drop(opens.await.unwrap());
    }

    #[tokio::test(start_paused = true)]
    async fn events_that_change_nothing_the_report_shows_dont_extend_it() {
        let (recorder, mut outcome, sender) = watching(vec![app()]);
        let sends = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            // niri sets a window's focus timestamp a moment after it took focus.
            let focus_timestamp = Some(Timestamp { secs: 1, nanos: 0 });
            let stamped = Event::WindowFocusTimestampChanged {
                id: 1,
                focus_timestamp,
            };
            event(&sender, 1, stamped);
            // Positions are left out of the report, so a move alone changes nothing in it.
            let mut layout = app().layout;
            layout.tile_pos_in_workspace_view = Some((10.0, 10.0));
            let moved = Event::WindowLayoutsChanged {
                changes: vec![(1, layout)],
            };
            event(&sender, 2, moved);
            sender
        });
        let started = Instant::now();
        let changes = recorder.finish(Wait::Quiet, &mut outcome).await;
        assert_eq!(started.elapsed(), QUIET);
        assert!(changes.is_empty());
        drop(sends.await.unwrap());
    }

    #[test]
    fn only_a_wait_that_saw_niri_or_a_screenshot_ends_the_report_early() {
        // `focus` returned on the event, so the rest of niri's batch may still be coming.
        for observed in [Observed::Focused, Observed::Closed] {
            assert_eq!(Quiet::UnlessSeen.wait(false, observed), Wait::Batch);
        }
        // `niri_action`'s wait ended on a timeout, long after niri's batch.
        for observed in [Observed::Changed, Observed::Unchanged, Observed::Closed] {
            assert_eq!(Quiet::UnlessSettled.wait(false, observed), Wait::Received);
            assert_eq!(Quiet::Always.wait(false, observed), Wait::Quiet);
        }
        // A focus that timed out or was interrupted, or an action only sent, saw nothing.
        for observed in [Observed::Timeout, Observed::Interrupted, Observed::Sent] {
            assert_eq!(Quiet::UnlessSeen.wait(false, observed), Wait::Quiet);
        }
        assert_eq!(
            Quiet::UnlessSettled.wait(false, Observed::Sent),
            Wait::Quiet
        );
        assert_eq!(Quiet::Never.wait(false, Observed::Sent), Wait::Received);
        assert_eq!(Quiet::Always.wait(true, Observed::Sent), Wait::Received);
    }

    #[tokio::test(start_paused = true)]
    async fn a_focus_reports_the_urgency_niri_clears_in_the_same_refresh() {
        let urgent = Window {
            is_urgent: true,
            ..window(2, Some("chat"), 1, false)
        };
        let (recorder, mut outcome, sender) = watching(vec![app(), urgent]);
        // The focus wait returned on this event; niri writes the next one a moment later.
        event(&sender, 1, Event::WindowFocusChanged { id: Some(2) });
        let clears = tokio::spawn(async move {
            tokio::task::yield_now().await;
            let cleared = Event::WindowUrgencyChanged {
                id: 2,
                urgent: false,
            };
            event(&sender, 2, cleared);
            sender
        });
        let wait = Quiet::UnlessSeen.wait(false, Observed::Focused);
        let changes = recorder.finish(wait, &mut outcome).await;
        assert_eq!(
            serde_json::to_value(&changes).unwrap(),
            json!({
                "focus": {"from": 1, "to": 2},
                "windows": [{"id": 2, "urgent": false}],
            })
        );
        drop(clears.await.unwrap());
    }

    #[tokio::test(start_paused = true)]
    async fn a_dialog_that_came_as_a_long_wait_ended_restarts_the_quiet() {
        let (recorder, mut outcome, sender) = watching(vec![app()]);
        // Ready as the action's own 300 ms wait ended, and not applied yet.
        tokio::time::advance(Duration::from_millis(300)).await;
        let dialog = window(5, Some("app"), 1, true);
        event(&sender, 1, Event::WindowOpenedOrChanged { window: dialog });
        let titles = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            let mut titled = window(5, Some("app"), 1, true);
            titled.title = Some("Save changes?".to_owned());
            event(&sender, 2, Event::WindowOpenedOrChanged { window: titled });
            sender
        });
        let started = Instant::now();
        let changes = recorder.finish(Wait::Quiet, &mut outcome).await;
        assert_eq!(started.elapsed(), Duration::from_millis(100) + QUIET);
        let opened = changes
            .opened
            .first()
            .and_then(|window| window.title.clone());
        assert_eq!(opened.as_deref(), Some("Save changes?"));
        drop(titles.await.unwrap());
    }

    #[tokio::test(start_paused = true)]
    async fn each_change_the_report_shows_restarts_the_quiet_and_is_in_it() {
        let mut resized = app().layout;
        resized.window_size = (800, 600);
        // A resize comes with a move, which alone would change nothing shown.
        resized.tile_pos_in_workspace_view = Some((10.0, 10.0));
        let cases = [
            (
                Event::WindowLayoutsChanged {
                    changes: vec![(1, resized)],
                },
                json!({"windows": [{"id": 1, "size": [800, 600]}]}),
            ),
            (
                Event::WindowUrgencyChanged {
                    id: 1,
                    urgent: true,
                },
                json!({"windows": [{"id": 1, "urgent": true}]}),
            ),
            (
                Event::WorkspaceActivated {
                    id: 2,
                    focused: true,
                },
                json!({"workspace": {"from": 1, "to": 2}}),
            ),
            (
                Event::WindowFocusChanged { id: None },
                json!({"focus": {"from": 1, "to": null}}),
            ),
        ];
        for (sent, reported) in cases {
            let (recorder, mut outcome, sender) = watching(vec![app()]);
            let sends = tokio::spawn(later(sender, Duration::from_millis(200), sent));
            let started = Instant::now();
            let changes = recorder.finish(Wait::Quiet, &mut outcome).await;
            assert_eq!(started.elapsed(), Duration::from_millis(200) + QUIET);
            assert_eq!(serde_json::to_value(&changes).unwrap(), reported);
            drop(sends.await.unwrap());
        }
    }

    #[tokio::test(start_paused = true)]
    async fn quiet_during_the_actions_own_wait_counts_and_only_the_rest_is_added() {
        let (mut recorder, mut outcome, _sender) = watching(vec![app()]);
        recorder.follow(tokio::time::sleep(QUIET)).await;
        let started = Instant::now();
        recorder.finish(Wait::Quiet, &mut outcome).await;
        assert_eq!(started.elapsed(), Duration::ZERO);

        let (mut later, mut later_outcome, sender) = watching(vec![app()]);
        let work = async {
            tokio::time::sleep(Duration::from_millis(200)).await;
            event(
                &sender,
                1,
                Event::WindowUrgencyChanged {
                    id: 1,
                    urgent: true,
                },
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        };
        later.follow(work).await;
        let ended = Instant::now();
        let changes = later.finish(Wait::Quiet, &mut later_outcome).await;
        assert_eq!(ended.elapsed(), Duration::from_millis(150));
        assert_eq!(
            changes.windows.first().and_then(|window| window.urgent),
            Some(true)
        );
    }

    /// Retitles window 1 every 100 ms, forty times, as a page that keeps loading does.
    async fn busy(sender: Sender) {
        for seq in 1..=40 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            let mut retitled = app();
            retitled.title = Some(format!("page {seq}"));
            event(
                &sender,
                seq,
                Event::WindowOpenedOrChanged { window: retitled },
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_busy_niri_ends_the_report_at_its_limit() {
        let (recorder, mut outcome, sender) = watching(vec![app()]);
        let busy = tokio::spawn(busy(sender));
        let started = Instant::now();
        let changes = recorder.finish(Wait::Quiet, &mut outcome).await;
        assert_eq!(started.elapsed(), EXTRA);
        let title = changes
            .windows
            .first()
            .and_then(|window| window.title.clone());
        assert!(title.is_some_and(|title| title.starts_with("page ")));
        busy.abort();
    }

    #[tokio::test(start_paused = true)]
    async fn after_a_settled_screenshot_the_report_waits_for_nothing() {
        let (recorder, mut outcome, sender) = watching(vec![app()]);
        event(&sender, 1, Event::WindowFocusChanged { id: None });
        let started = Instant::now();
        let changes = recorder.finish(Wait::Received, &mut outcome).await;
        assert_eq!(started.elapsed(), Duration::ZERO);
        assert_eq!(
            serde_json::to_value(&changes).unwrap(),
            json!({"focus": {"from": 1, "to": null}})
        );
        assert_eq!(outcome.focused_window, None);
    }

    #[tokio::test(start_paused = true)]
    async fn a_stream_lost_during_the_report_keeps_what_was_seen_and_says_why() {
        let (recorder, mut outcome, sender) = watching(vec![app()]);
        outcome.window = Some((&app()).into());
        let opened = window(2, Some("app"), 1, true);
        event(&sender, 1, Event::WindowOpenedOrChanged { window: opened });
        sender.send(Update::Reset(2)).unwrap();
        let changes = recorder.finish(Wait::Quiet, &mut outcome).await;
        assert_eq!(changes.opened.len(), 1);
        assert_eq!(changes.focus.map(|focus| focus.to), Some(Some(2)));
        assert!(changes.lost.unwrap().contains("disconnected"));
        // Focus may have moved on since: unknown, not window 2.
        assert_eq!(outcome.focused_window, None);
        // The window the action is about agrees with the report: it lost focus.
        assert_eq!(outcome.window.map(|window| window.is_focused), Some(false));
    }

    /// Marks window 1 urgent on odd sequence numbers and not on even ones.
    const fn urgent(seq: u64) -> Event {
        Event::WindowUrgencyChanged {
            id: 1,
            urgent: seq % 2 == 1,
        }
    }

    /// Sends one more event each time it gets to run, until the thousandth.
    async fn keeps_sending(sender: Sender) {
        for seq in 2..1000 {
            event(&sender, seq, urgent(seq));
            tokio::task::yield_now().await;
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_niri_that_keeps_sending_cant_hold_the_report_after_a_screenshot() {
        let (recorder, mut outcome, sender) = watching(vec![app()]);
        event(&sender, 1, urgent(1));
        let sending = tokio::spawn(keeps_sending(sender));
        let changes = recorder.finish(Wait::Received, &mut outcome).await;
        assert!(!sending.is_finished());
        sending.abort();
        // The event received before the report ended is in it.
        assert_eq!(
            changes.windows.first().and_then(|window| window.urgent),
            Some(true)
        );
    }
}
