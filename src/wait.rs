//! `wait_for`'s work: waiting on niri's event stream until a window appears, closes or
//! changes its title, walking a window's accessible tree until an element appears or goes
//! away, or waiting until the screen stops changing, so an agent doesn't poll with
//! screenshots. Waiting changes nothing, so it needs no lease.

use std::time::Duration;

use serde::Serialize;
use tokio::time::Instant;

use crate::a11y::Capped;
use crate::a11y::model::{Filter, Unbound, Unmappable};
use crate::elements::{self, Listing};
use crate::error::{CallError, ErrorName, ToolError};
use crate::niri::waiter::{View, Waited, Waiter};
use crate::observe::{Metadata, Screenshot};
use crate::{niri, settle};

/// The shortest pause between two walks of an element condition. `elements` on the nested
/// suite's fixture windows takes 5 to 8 ms at the median and at most 90 ms, and an agent's
/// next turn takes seconds, so four walks a second cost the app little and the agent no
/// noticeable time.
const PAUSE: Duration = Duration::from_millis(250);

/// What to wait for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Until {
    /// A window with this `app_id` and a title containing this text, either one optional.
    Window {
        app_id: Option<String>,
        title: Option<String>,
    },
    /// The window with this id is gone.
    Closed(u64),
    /// The window with this id has a title containing this text.
    Title { window_id: u64, contains: String },
    /// An element of a window's accessible tree.
    Element(Element),
    /// Two captures of the focused output in a row are the same.
    ScreenStable,
}

/// An element condition: one that `filter` lists in window `window_id` is there, or with
/// `absent`, none is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Element {
    pub(crate) window_id: u64,
    pub(crate) filter: Filter,
    pub(crate) absent: bool,
}

/// How the wait ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Ended {
    Met,
    Timeout,
    /// niri's event stream was lost, so whether it happened is unknown.
    Uncertain,
    /// The window of an element condition closed.
    WindowGone,
    /// The window of an element condition isn't on the accessibility bus.
    NotAccessible,
}

#[derive(Debug, Serialize)]
pub(crate) struct Report {
    pub(crate) observed: Ended,
    /// The windows that met the condition.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) windows: Vec<u64>,
    /// The element that appeared, for an element condition.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) element: Option<Appeared>,
    /// The window with keyboard focus when the wait ended; absent for `screen_stable`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) focused_window: Option<u64>,
    pub(crate) waited_ms: u128,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) detail: Option<String>,
    /// The metadata of the screenshot that comes with the result.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) screenshot: Option<Metadata>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) screenshot_error: Option<ToolError>,
}

impl Until {
    /// Refuses a condition that can never be met or doesn't say what to wait for.
    pub(crate) fn check(&self) -> Result<(), String> {
        match self {
            Self::Window {
                app_id: None,
                title: None,
            } => Err("`window` needs an `app_id`, a `title`, or both".to_owned()),
            Self::Title { contains, .. } if contains.is_empty() => {
                Err("`contains` must not be empty".to_owned())
            }
            Self::Window { .. }
            | Self::Closed(_)
            | Self::Title { .. }
            | Self::Element(_)
            | Self::ScreenStable => Ok(()),
        }
    }
}

/// The windows that meet `until` in `view`, or None while it isn't met.
fn met(until: &Until, view: &View) -> Option<Vec<u64>> {
    let mut ids: Vec<u64> = match until {
        Until::Window { app_id, title } => view
            .windows()
            .values()
            .filter(|window| {
                app_id
                    .as_ref()
                    .is_none_or(|app_id| window.app_id.as_ref() == Some(app_id))
                    && title.as_ref().is_none_or(|title| {
                        window.title.as_ref().is_some_and(|it| it.contains(title))
                    })
            })
            .map(|window| window.id)
            .collect(),
        Until::Closed(id) => return (!view.windows().contains_key(id)).then(|| vec![*id]),
        Until::Title {
            window_id,
            contains,
        } => {
            let window = view.windows().get(window_id)?;
            let titled = window
                .title
                .as_ref()
                .is_some_and(|it| it.contains(contains));
            return titled.then(|| vec![*window_id]);
        }
        Until::Element(_) | Until::ScreenStable => return None,
    };
    ids.sort_unstable();
    (!ids.is_empty()).then_some(ids)
}

/// Waits on the event stream for a window condition, up to `limit`.
pub(crate) async fn window(
    events: niri::Events<'_>,
    until: &Until,
    limit: Duration,
) -> Result<Report, CallError> {
    let started = Instant::now();
    let mut waiter = niri::waiter(events).await?;
    if let Until::Title { window_id, .. } = until
        && !waiter.view().windows().contains_key(window_id)
    {
        return Err(CallError::InvalidArguments(format!(
            "no window with id {window_id}; desktop_state lists them"
        )));
    }
    let (observed, windows, detail) = match waiter.until(limit, |view| met(until, view)).await {
        Waited::Done(windows) => (Ended::Met, windows, None),
        Waited::Timeout => (Ended::Timeout, Vec::new(), None),
        Waited::Lost(reason) => (Ended::Uncertain, Vec::new(), Some(reason)),
    };
    Ok(Report {
        observed,
        windows,
        focused_window: waiter.view().focused_window(),
        waited_ms: started.elapsed().as_millis(),
        detail,
        element: None,
        screenshot: None,
        screenshot_error: None,
    })
}

/// The element that met an appearing condition: its ref in the form `elements` issues,
/// while there is a lease, its role and its states, and as in `elements`, why it has no
/// box and why every tool refuses its ref, when it does. Never its name, the app's text.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Appeared {
    pub(crate) element_ref: Option<String>,
    pub(crate) role: &'static str,
    pub(crate) states: Vec<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) unmappable: Option<Unmappable>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) unbound: Option<Unbound>,
}

/// Why an element that appeared has no ref, though the lease is held.
const WITHHELD: &str = "no element_ref: the walk was cut short before it could tell whether \
    another element has this one's role and name, as a list's rows put on the bus without \
    containers do; aim at its pixels on a fresh screenshot";

/// How one walk left an element condition.
#[derive(Debug, PartialEq, Eq)]
enum Checked {
    /// The wait ends.
    Ends(Ended, Option<Appeared>, Option<String>),
    /// Not met yet, and why the walk might have missed what would meet it.
    Unmet(Option<Capped>),
}

/// How `polled`, one walk's listing or none when the window is gone, leaves `element`. A
/// walk cut short can't show that none matches.
fn check(
    element: &Element,
    polled: Result<Option<Listing>, ToolError>,
) -> Result<Checked, ToolError> {
    let listing = match polled {
        Ok(Some(listing)) => listing,
        Ok(None) => return Ok(Checked::Ends(Ended::WindowGone, None, None)),
        Err(error) if error.name == ErrorName::NotAccessible => {
            return Ok(Checked::Ends(
                Ended::NotAccessible,
                None,
                Some(error.detail),
            ));
        }
        Err(error) => return Err(error),
    };
    let withheld = listing.withheld.then(|| WITHHELD.to_owned());
    let first = listing.elements.into_iter().next();
    Ok(match (first, element.absent) {
        (Some(found), false) => Checked::Ends(
            Ended::Met,
            Some(Appeared {
                element_ref: found.element_ref,
                role: found.role,
                states: found.states,
                unmappable: found.unmappable,
                unbound: found.unbound,
            }),
            withheld,
        ),
        // An `absent` check lists nothing: `truncated` says the walk read a match.
        (Some(_), true) => Checked::Unmet(None),
        (None, true) if listing.truncated => Checked::Unmet(None),
        (None, true) if listing.capped_reason.is_none() => Checked::Ends(Ended::Met, None, None),
        (None, _) => Checked::Unmet(listing.capped_reason),
    })
}

/// Walks the tree of `element`'s window through `poll` until the condition holds, the
/// window closes, or `limit` passes (`watch`).
pub(crate) async fn element<F>(
    events: niri::Events<'_>,
    element: &Element,
    limit: Duration,
    poll: impl FnMut() -> F,
) -> Result<Report, CallError>
where
    F: Future<Output = Result<Option<Listing>, ToolError>>,
{
    let started = Instant::now();
    let waiter = niri::waiter(events).await?;
    let id = element.window_id;
    if !waiter.view().windows().contains_key(&id) {
        return Err(CallError::InvalidArguments(format!(
            "no window with id {id}; desktop_state lists them"
        )));
    }
    Ok(watch(waiter, element, started + limit, poll).await?)
}

/// Walks through `poll` until `element` holds, the window closes or `deadline` passes. A
/// walk keeps its own per-call deadline and budget and is cut off at `deadline`. Walks
/// never overlap, and the pause after one is at least as long as it took, so a slow app
/// spends at most half the wait answering. niri's event stream ends the wait the moment
/// the window closes, mid-walk too; without the stream, the next walk finds it gone. An
/// app that leaves the bus after a walk read it gets one more walk before the wait ends
/// `not_accessible`, so its window closing can end it `window_gone`.
async fn watch<F>(
    mut waiter: Waiter,
    element: &Element,
    deadline: Instant,
    mut poll: impl FnMut() -> F,
) -> Result<Report, ToolError>
where
    F: Future<Output = Result<Option<Listing>, ToolError>>,
{
    let started = Instant::now();
    let id = element.window_id;
    let (mut cut, mut read) = (None, false);
    let walking = async {
        loop {
            let walk = Instant::now();
            match check(element, poll().await)? {
                // An app that quits leaves the bus as niri closes its window, and may leave
                // first: after a walk that read it, look once more.
                Checked::Ends(Ended::NotAccessible, ..) if read => read = false,
                Checked::Ends(ended, appeared, detail) => return Ok((ended, appeared, detail)),
                Checked::Unmet(capped) => (cut, read) = (capped, true),
            }
            tokio::time::sleep(walk.elapsed().max(PAUSE)).await;
        }
    };
    let closed = async {
        let gone = |view: &View| (!view.windows().contains_key(&id)).then_some(());
        let left = deadline.saturating_duration_since(Instant::now());
        match waiter.until(left, gone).await {
            Waited::Done(()) => (Ended::WindowGone, None, None),
            Waited::Timeout | Waited::Lost(_) => std::future::pending().await,
        }
    };
    let ended: Result<_, ToolError> = tokio::select! {
        biased;
        gone = closed => Ok(gone),
        walked = tokio::time::timeout_at(deadline, walking) => walked.unwrap_or_else(|_| {
            let why = |capped| format!("the last walk was cut short: {}", elements::word(capped));
            Ok((Ended::Timeout, None, cut.map(why)))
        }),
    };
    let (observed, appeared, detail) = ended?;
    Ok(Report {
        observed,
        windows: if observed == Ended::Met {
            vec![id]
        } else {
            Vec::new()
        },
        focused_window: waiter.view().focused_window(),
        waited_ms: started.elapsed().as_millis(),
        detail,
        element: appeared,
        screenshot: None,
        screenshot_error: None,
    })
}

/// Captures the focused output until two captures in a row are the same, up to `limit`;
/// the report and the last capture.
pub(crate) async fn screen<F>(
    capture: impl FnMut() -> F,
    limit: Duration,
) -> Result<(Report, Screenshot), CallError>
where
    F: Future<Output = Result<Screenshot, CallError>>,
{
    let started = Instant::now();
    let shot = settle::screenshot(capture, limit).await?;
    let report = Report {
        observed: if shot.metadata.settled == Some(true) {
            Ended::Met
        } else {
            Ended::Timeout
        },
        windows: Vec::new(),
        focused_window: None,
        waited_ms: started.elapsed().as_millis(),
        detail: None,
        element: None,
        screenshot: None,
        screenshot_error: None,
    };
    Ok((report, shot))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::elements::Listed;
    use crate::error::ErrorName;
    use crate::niri::waiter::Update;
    use crate::niri::waiter::tests::{view, waiter, window};

    fn titled(id: u64, app_id: &str, title: &str) -> niri_ipc::Window {
        niri_ipc::Window {
            title: Some(title.to_owned()),
            ..window(id, Some(app_id), 1, false)
        }
    }

    #[test]
    fn a_window_condition_matches_app_id_and_title_text() {
        let current = view(vec![
            titled(1, "firefox", "Inbox — Mail"),
            titled(2, "foot", "build: ok"),
            titled(3, "firefox", "Save As"),
        ]);
        let window = |app_id: Option<&str>, title: Option<&str>| Until::Window {
            app_id: app_id.map(str::to_owned),
            title: title.map(str::to_owned),
        };
        assert_eq!(
            met(&window(Some("firefox"), None), &current),
            Some(vec![1, 3])
        );
        assert_eq!(
            met(&window(Some("firefox"), Some("Save")), &current),
            Some(vec![3])
        );
        assert_eq!(met(&window(None, Some("build")), &current), Some(vec![2]));
        assert_eq!(met(&window(Some("foot"), Some("Save")), &current), None);
        assert!(window(None, None).check().is_err());
    }

    #[test]
    fn closed_and_title_conditions_follow_one_window() {
        let current = view(vec![titled(2, "foot", "make: running")]);
        assert_eq!(met(&Until::Closed(7), &current), Some(vec![7]));
        assert_eq!(met(&Until::Closed(2), &current), None);
        let title = |contains: &str| Until::Title {
            window_id: 2,
            contains: contains.to_owned(),
        };
        assert_eq!(met(&title("running"), &current), Some(vec![2]));
        assert_eq!(met(&title("done"), &current), None);
        assert!(title("").check().is_err());
    }

    /// A walk's listing of window 1: buttons with the refs `refs`, and why it stopped early.
    fn listing(refs: &[&str], capped: Option<Capped>) -> Listing {
        let elements = refs
            .iter()
            .map(|id| Listed {
                element_ref: Some((*id).to_owned()),
                role: "button",
                name: "Save".to_owned(),
                states: vec!["enabled", "sensitive", "showing"],
                actions: vec!["click".to_owned()],
                layout_box: None,
                unmappable: None,
                unbound: None,
            })
            .collect();
        Listing {
            window_id: 1,
            elements,
            truncated: false,
            walked: 10,
            capped: capped.is_some(),
            capped_reason: capped,
            withheld: false,
        }
    }

    fn save_button(absent: bool) -> Element {
        Element {
            window_id: 1,
            filter: Filter::new(Some("button".to_owned()), Some("save"), false)
                .with_state("enabled")
                .unwrap(),
            absent,
        }
    }

    /// Watches `element` in window 1 for up to `limit`, walking as `walk` says for the walk
    /// numbered from 0: how long it takes and what it lists. The report, the number of
    /// walks, and the most that ran at once.
    async fn watched(
        element: &Element,
        limit: Duration,
        walk: impl Fn(usize) -> (Duration, Result<Option<Listing>, ToolError>) + Sync,
    ) -> (Report, usize, usize) {
        let (waiter, _sender) = waiter(view(vec![window(1, Some("a"), 1, true)]), 1);
        let (walks, running, most) = (
            AtomicUsize::new(0),
            AtomicUsize::new(0),
            AtomicUsize::new(0),
        );
        let running = &running;
        let poll = || {
            let (took, listed) = walk(walks.fetch_add(1, Ordering::Relaxed));
            let now = running.fetch_add(1, Ordering::Relaxed) + 1;
            most.fetch_max(now, Ordering::Relaxed);
            async move {
                tokio::time::sleep(took).await;
                running.fetch_sub(1, Ordering::Relaxed);
                listed
            }
        };
        let report = watch(waiter, element, Instant::now() + limit, poll)
            .await
            .unwrap();
        (report, walks.into_inner(), most.into_inner())
    }

    const WALK: Duration = Duration::from_millis(40);

    /// `listed`, `took` from now, as a walk that takes that long.
    async fn after(
        took: Duration,
        listed: Result<Option<Listing>, ToolError>,
    ) -> Result<Option<Listing>, ToolError> {
        tokio::time::sleep(took).await;
        listed
    }

    // The bug: a wait that reads once, or loses the ref, makes the agent list again.
    #[tokio::test(start_paused = true)]
    async fn an_element_that_appears_later_ends_the_wait_met_with_its_ref() {
        let appears = |walk| match walk {
            0 | 1 => (WALK, Ok(Some(listing(&[], None)))),
            _ => (WALK, Ok(Some(listing(&["elem-ab-3"], None)))),
        };
        let (report, walks, _) =
            watched(&save_button(false), Duration::from_secs(5), appears).await;
        assert_eq!(report.observed, Ended::Met);
        assert_eq!(report.windows, [1]);
        assert_eq!(
            report.element,
            Some(Appeared {
                element_ref: Some("elem-ab-3".to_owned()),
                role: "button",
                states: vec!["enabled", "sensitive", "showing"],
                unmappable: None,
                unbound: None,
            })
        );
        assert_eq!(walks, 3);
        assert_eq!(
            report.waited_ms,
            2 * PAUSE.as_millis() + 3 * WALK.as_millis()
        );
    }

    // The bug: a walk cut short by its budget, which saw no match, taken as proof that
    // none is left.
    #[tokio::test(start_paused = true)]
    async fn absent_is_met_by_a_whole_walk_and_never_by_a_cut_one() {
        let cut = |walk| match walk {
            0 => (WALK, Ok(Some(listing(&["elem-ab-1"], None)))),
            _ => (WALK, Ok(Some(listing(&[], Some(Capped::BudgetExhausted))))),
        };
        let limit = Duration::from_secs(2);
        let (report, _, _) = watched(&save_button(true), limit, cut).await;
        assert_eq!(report.observed, Ended::Timeout);
        assert_eq!(report.waited_ms, limit.as_millis());
        assert_eq!(
            report.detail.as_deref(),
            Some("the last walk was cut short: budget_exhausted")
        );
        let gone = |walk| match walk {
            0 => (WALK, Ok(Some(listing(&["elem-ab-1"], None)))),
            _ => (WALK, Ok(Some(listing(&[], None)))),
        };
        let (met, walks, _) = watched(&save_button(true), limit, gone).await;
        assert_eq!((met.observed, met.element, walks), (Ended::Met, None, 2));
        assert_eq!(met.windows, [1]);
    }

    // The bug: a closed window left to the timeout, or to the end of a walk of an app that
    // no longer answers.
    #[tokio::test(start_paused = true)]
    async fn a_window_that_closes_ends_the_wait_at_once_mid_walk() {
        let (waiter, sender) = waiter(view(vec![window(1, Some("a"), 1, true)]), 1);
        let close = async {
            tokio::time::sleep(Duration::from_millis(100)).await;
            let closed = niri_ipc::Event::WindowClosed { id: 1 };
            sender.send(Update::Event(2, Box::new(closed))).unwrap();
        };
        let hung = || std::future::pending::<Result<Option<Listing>, ToolError>>();
        let deadline = Instant::now() + Duration::from_secs(10);
        let element = save_button(false);
        let (report, ()) = tokio::join!(watch(waiter, &element, deadline, hung), close);
        let report = report.unwrap();
        assert_eq!(report.observed, Ended::WindowGone);
        assert_eq!(report.waited_ms, 100);
        assert_eq!(report.windows, [0; 0]);
    }

    // The bug: `timeout_ms` overrun by a walk that runs past it, three seconds for an app
    // that answers each call just under its deadline.
    #[tokio::test(start_paused = true)]
    async fn the_timeout_holds_when_one_walk_is_slow() {
        let slow = |_| (Duration::from_secs(3), Ok(Some(listing(&[], None))));
        let (report, walks, _) = watched(&save_button(false), Duration::from_secs(1), slow).await;
        assert_eq!(
            (report.observed, report.waited_ms, walks),
            (Ended::Timeout, 1000, 1)
        );
        assert_eq!(report.detail, None);
    }

    // The bug: walks that pile up on a slow app, or come back to back.
    #[tokio::test(start_paused = true)]
    async fn walks_never_overlap_and_pause_as_long_as_they_took() {
        let slow = |_| (Duration::from_millis(600), Ok(Some(listing(&[], None))));
        let (_, walks, most) = watched(&save_button(false), Duration::from_secs(3), slow).await;
        // Walks start at 0, 1.2 and 2.4 seconds.
        assert_eq!((walks, most), (3, 1));
    }

    #[test]
    fn a_window_off_the_accessibility_bus_ends_the_wait_and_other_failures_fail_it() {
        let element = save_button(false);
        let off = Err(ToolError::new(ErrorName::NotAccessible, "no app for pid 7"));
        assert_eq!(
            check(&element, off).unwrap(),
            Checked::Ends(
                Ended::NotAccessible,
                None,
                Some("no app for pid 7".to_owned())
            )
        );
        assert_eq!(
            check(&element, Ok(None)).unwrap(),
            Checked::Ends(Ended::WindowGone, None, None)
        );
        let refused = Err(ToolError::new(
            ErrorName::UpstreamError,
            "GetChildren failed",
        ));
        assert_eq!(
            check(&element, refused).unwrap_err().name,
            ErrorName::UpstreamError
        );
    }

    // The bug: a ref every tool refuses handed back as if it worked, where `elements`
    // would have said `unbound:repeated`, and a box-less element with no word of why.
    #[test]
    fn an_appeared_element_says_why_its_ref_is_refused_or_it_has_no_box() {
        let mut row = listing(&["elem-ab-1"], None);
        row.elements[0].unbound = Some(Unbound::Repeated);
        row.elements[0].unmappable = Some(Unmappable::FrameSizeMismatch);
        let Ok(Checked::Ends(Ended::Met, Some(appeared), None)) =
            check(&save_button(false), Ok(Some(row)))
        else {
            panic!("not met");
        };
        assert_eq!(appeared.unbound, Some(Unbound::Repeated));
        assert_eq!(appeared.unmappable, Some(Unmappable::FrameSizeMismatch));
        let json = serde_json::to_value(&appeared).unwrap();
        assert_eq!(json["unbound"], "repeated");
        assert_eq!(json["unmappable"], "frame_size_mismatch");
    }

    // The bug: an element with no ref and no word of why, which the agent takes for a
    // missing lease.
    #[test]
    fn an_appeared_element_whose_ref_was_withheld_says_why() {
        let mut unchecked = listing(&[], Some(Capped::BudgetExhausted));
        unchecked.elements = listing(&["x"], None).elements;
        unchecked.elements[0].element_ref = None;
        unchecked.withheld = true;
        let checked = check(&save_button(false), Ok(Some(unchecked))).unwrap();
        let Checked::Ends(Ended::Met, Some(appeared), Some(detail)) = checked else {
            panic!("{checked:?}");
        };
        assert_eq!(appeared.element_ref, None);
        assert!(detail.contains("role and name"), "{detail}");
    }

    // The bug: an app that quits leaves the accessibility bus a moment before niri reports
    // its window closed, and the wait ended `not_accessible` in between.
    #[tokio::test(start_paused = true)]
    async fn an_app_that_leaves_the_bus_with_its_window_ends_the_wait_window_gone() {
        let (waiter, sender) = waiter(view(vec![window(1, Some("a"), 1, true)]), 1);
        let walks = AtomicUsize::new(0);
        let poll = || {
            let listed = match walks.fetch_add(1, Ordering::Relaxed) {
                0 => Ok(Some(listing(&[], None))),
                _ => Err(ToolError::new(ErrorName::NotAccessible, "no app for pid 7")),
            };
            after(WALK, listed)
        };
        // The second walk ends at 330 ms, off the bus; niri reports the close at 400.
        let close = async {
            tokio::time::sleep(Duration::from_millis(400)).await;
            let closed = niri_ipc::Event::WindowClosed { id: 1 };
            sender.send(Update::Event(2, Box::new(closed))).unwrap();
        };
        let element = save_button(false);
        let deadline = Instant::now() + Duration::from_secs(10);
        let (report, ()) = tokio::join!(watch(waiter, &element, deadline, poll), close);
        assert_eq!(report.unwrap().observed, Ended::WindowGone);
    }

    // An app that stays off the bus still ends the wait, one walk later.
    #[tokio::test(start_paused = true)]
    async fn an_app_off_the_bus_twice_in_a_row_ends_the_wait_not_accessible() {
        let off = |walk| match walk {
            0 => (WALK, Ok(Some(listing(&[], None)))),
            _ => (
                WALK,
                Err(ToolError::new(ErrorName::NotAccessible, "no app")),
            ),
        };
        let (report, walks, _) = watched(&save_button(false), Duration::from_secs(5), off).await;
        assert_eq!((report.observed, walks), (Ended::NotAccessible, 3));
        let first = |_| {
            (
                WALK,
                Err(ToolError::new(ErrorName::NotAccessible, "no app")),
            )
        };
        let (at_once, first_walks, _) =
            watched(&save_button(false), Duration::from_secs(5), first).await;
        assert_eq!((at_once.observed, first_walks), (Ended::NotAccessible, 1));
    }
}
