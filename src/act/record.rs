//! The change report's watch on niri's event stream: a waiter registered as an action's
//! work starts, which afterwards applies every event niri sent since and says what changed
//! (`changes::desktop`). It reads only the event stream, never asks niri anything.

use std::collections::BTreeMap;
use std::time::Duration;

use niri_ipc::Window;
use tokio::time::Instant;

use crate::changes::{self, Changes, Scene};
use crate::niri::waiter::{View, Waited, Waiter};
use crate::niri::{self, Events};

/// The report ends once this long passes without an event from niri, so an effect that
/// comes a moment after the action, such as a dialog opening after a click, is in it.
pub(crate) const QUIET: Duration = Duration::from_millis(250);
/// At most this long after the work ended, however busy niri is.
const QUIET_LIMIT: Duration = Duration::from_secs(1);

/// A watch on niri's events from the start of an action.
#[derive(Debug)]
pub(crate) struct Recorder {
    waiter: Waiter,
    before: Scene,
    /// Every window seen since that wasn't open before, as last seen.
    passing: BTreeMap<u64, Window>,
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
        })
    }

    /// Applies niri's events as `wait` says, then what changed since the start; with
    /// `lost` saying why when the stream ended first.
    pub(crate) async fn finish(self, wait: Wait) -> Changes {
        let Self {
            mut waiter,
            before,
            mut passing,
        } = self;
        let mut record = |view: &View| {
            let new = view.windows().iter().filter(|(id, _)| !before.has(**id));
            passing.extend(new.map(|(id, window)| (*id, window.clone())));
        };
        let lost = match wait {
            Wait::Settled => drain(&mut waiter, &mut record).await,
            Wait::Quiet => quiet(&mut waiter, &mut record).await,
        };
        let mut changes = changes::desktop(&before, &Scene::of(waiter.view()), &passing);
        changes.lost = lost;
        changes
    }
}

/// Applies the events already received, passing each view to `record`.
async fn drain(waiter: &mut Waiter, record: &mut impl FnMut(&View)) -> Option<String> {
    let ended = waiter
        .until(Duration::ZERO, |view| {
            record(view);
            None::<()>
        })
        .await;
    match ended {
        Waited::Lost(reason) => Some(reason),
        Waited::Done(()) | Waited::Timeout => None,
    }
}

/// Applies events until `QUIET` passes without one, or `QUIET_LIMIT` after the start,
/// passing each view to `record`.
async fn quiet(waiter: &mut Waiter, record: &mut impl FnMut(&View)) -> Option<String> {
    let deadline = Instant::now() + QUIET_LIMIT;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return drain(waiter, record).await;
        }
        // The first check sees the view as it is; any later one follows a new event.
        let mut checks = 0;
        let ended = waiter
            .until(QUIET.min(left), |view| {
                record(view);
                checks += 1;
                (checks > 1).then_some(())
            })
            .await;
        match ended {
            Waited::Done(()) => {}
            Waited::Timeout => return None,
            Waited::Lost(reason) => return Some(reason),
        }
    }
}

#[cfg(test)]
mod tests {
    use niri_ipc::Event;
    use serde_json::json;

    use super::*;
    use crate::niri::waiter::Update;
    use crate::niri::waiter::tests::{view, waiter, window};

    fn watching(windows: Vec<Window>) -> (Recorder, tokio::sync::broadcast::Sender<Update>) {
        let (waiter, sender) = waiter(view(windows), 0);
        let before = Scene::of(waiter.view());
        let recorder = Recorder {
            waiter,
            before,
            passing: BTreeMap::new(),
        };
        (recorder, sender)
    }

    fn event(sender: &tokio::sync::broadcast::Sender<Update>, seq: u64, event: Event) {
        sender.send(Update::Event(seq, Box::new(event))).unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn a_dialog_that_opens_a_moment_after_the_action_is_in_the_report() {
        let (recorder, sender) = watching(vec![window(1, Some("app"), 1, true)]);
        let opens = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            let dialog = window(5, Some("app"), 1, true);
            event(&sender, 1, Event::WindowOpenedOrChanged { window: dialog });
            // A window that comes and goes again within the report.
            tokio::time::sleep(Duration::from_millis(200)).await;
            let toast = window(6, Some("toast"), 1, false);
            event(&sender, 2, Event::WindowOpenedOrChanged { window: toast });
            event(&sender, 3, Event::WindowClosed { id: 6 });
            sender
        });
        let started = Instant::now();
        let changes = recorder.finish(Wait::Quiet).await;
        assert_eq!(started.elapsed(), Duration::from_millis(650));
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
        drop(opens.await.unwrap());
    }

    /// Marks window 1 urgent every 100 ms, forty times.
    async fn busy(sender: tokio::sync::broadcast::Sender<Update>) {
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
        let (recorder, sender) = watching(vec![window(1, Some("app"), 1, true)]);
        let busy = tokio::spawn(busy(sender));
        let started = Instant::now();
        let changes = recorder.finish(Wait::Quiet).await;
        assert_eq!(started.elapsed(), QUIET_LIMIT);
        assert_eq!(
            changes.windows.first().and_then(|window| window.urgent),
            Some(true)
        );
        busy.abort();
    }

    #[tokio::test(start_paused = true)]
    async fn after_a_settled_screenshot_the_report_waits_for_nothing() {
        let (recorder, sender) = watching(vec![window(1, Some("app"), 1, true)]);
        event(&sender, 1, Event::WindowFocusChanged { id: None });
        let started = Instant::now();
        let changes = recorder.finish(Wait::Settled).await;
        assert_eq!(started.elapsed(), Duration::ZERO);
        assert_eq!(
            serde_json::to_value(&changes).unwrap(),
            json!({"focus": {"from": 1, "to": null}})
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_stream_lost_during_the_report_keeps_what_was_seen_and_says_why() {
        let (recorder, sender) = watching(vec![window(1, Some("app"), 1, true)]);
        event(&sender, 1, Event::WindowClosed { id: 1 });
        sender.send(Update::Reset(2)).unwrap();
        let changes = recorder.finish(Wait::Quiet).await;
        assert_eq!(changes.closed.len(), 1);
        assert!(changes.lost.unwrap().contains("disconnected"));
    }
}
