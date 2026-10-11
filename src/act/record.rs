//! The change report's watch on niri's event stream: a waiter registered as an action's
//! work starts, which follows niri's events while the work and its evidence run and
//! afterwards says what changed (`changes::desktop`). It reads only the event stream, never
//! asks niri anything.

use std::collections::BTreeMap;
use std::time::Duration;

use niri_ipc::Window;
use tokio::time::Instant;

use super::Outcome;
use crate::changes::{self, Changes, Scene};
use crate::niri::waiter::{Waited, Waiter};
use crate::niri::{self, Events};

/// The report ends once niri has sent no event for this long, counted from the start of
/// the action, so an effect that comes a moment after the input, such as a dialog
/// opening after a click, is in it. The action's own wait counts towards it.
pub(crate) const QUIET: Duration = Duration::from_millis(250);
/// At most this long after the action's own wait and its evidence, however busy niri is.
const EXTRA: Duration = Duration::from_millis(500);

/// A watch on niri's events from the start of an action.
#[derive(Debug)]
pub(crate) struct Recorder {
    waiter: Waiter,
    before: Scene,
    /// Every window seen since that wasn't open before, as last seen.
    passing: BTreeMap<u64, Window>,
    /// When the last event came, or the watch started.
    last: Instant,
    /// Why the event stream ended, once it did.
    lost: Option<String>,
}

/// How long the report waits once the work ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Wait {
    /// Until niri has been quiet for `QUIET`: nothing else waited for the screen.
    Quiet,
    /// Only for what niri already sent, after a settled screenshot waited for the screen.
    Settled,
}

impl Recorder {
    /// Starts watching, or none without an event stream: then there is no report.
    pub(crate) async fn start(events: Events<'_>) -> Option<Self> {
        let waiter = niri::waiter(events).await.ok()?;
        let before = Scene::of(waiter.view());
        Some(Self {
            waiter,
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
    /// then focus unknown.
    pub(crate) async fn finish(mut self, wait: Wait, outcome: &mut Outcome) -> Changes {
        if wait == Wait::Quiet {
            self.quiet().await;
        }
        self.drain().await;
        let view = self.waiter.view();
        if self.lost.is_some() {
            outcome.focused_window = None;
        } else {
            outcome.caught_up(view);
        }
        let mut changes = changes::desktop(&self.before, &Scene::of(view), &self.passing);
        changes.lost = self.lost;
        changes
    }

    /// Applies events until `QUIET` passed since the last one, or `EXTRA` from now.
    async fn quiet(&mut self) {
        let limit = Instant::now() + EXTRA;
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

    /// Applies the events already received.
    async fn drain(&mut self) {
        while self.lost.is_none() && self.next(Duration::ZERO).await {}
    }

    /// Applies the next event, waiting at most `limit` for it: whether one came. The
    /// stream ending sets `lost`.
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
                self.last = Instant::now();
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
    use niri_ipc::Event;
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

    /// Marks window 1 urgent every 100 ms, forty times.
    async fn busy(sender: Sender) {
        for seq in 1..=40 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            let urgent = Event::WindowUrgencyChanged {
                id: 1,
                urgent: true,
            };
            event(&sender, seq, urgent);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_busy_niri_ends_the_report_at_its_limit() {
        let (recorder, mut outcome, sender) = watching(vec![app()]);
        let busy = tokio::spawn(busy(sender));
        let started = Instant::now();
        let changes = recorder.finish(Wait::Quiet, &mut outcome).await;
        assert_eq!(started.elapsed(), EXTRA);
        assert_eq!(
            changes.windows.first().and_then(|window| window.urgent),
            Some(true)
        );
        busy.abort();
    }

    #[tokio::test(start_paused = true)]
    async fn after_a_settled_screenshot_the_report_waits_for_nothing() {
        let (recorder, mut outcome, sender) = watching(vec![app()]);
        event(&sender, 1, Event::WindowFocusChanged { id: None });
        let started = Instant::now();
        let changes = recorder.finish(Wait::Settled, &mut outcome).await;
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
        let opened = window(2, Some("app"), 1, true);
        event(&sender, 1, Event::WindowOpenedOrChanged { window: opened });
        sender.send(Update::Reset(2)).unwrap();
        let changes = recorder.finish(Wait::Quiet, &mut outcome).await;
        assert_eq!(changes.opened.len(), 1);
        assert!(changes.lost.unwrap().contains("disconnected"));
        // Focus may have moved on since: unknown, not window 2.
        assert_eq!(outcome.focused_window, None);
    }
}
