//! `wait_for`'s work: waiting on niri's event stream until a window appears, closes or
//! changes its title, or until the screen stops changing, so an agent doesn't poll with
//! screenshots. Waiting changes nothing, so it needs no lease.

use std::time::Duration;

use serde::Serialize;
use tokio::time::Instant;

use crate::error::{CallError, ToolError};
use crate::niri::events::EventStream;
use crate::niri::waiter::{View, Waited};
use crate::observe::{Metadata, Screenshot};
use crate::{niri, settle};

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
    /// Two captures of the focused output in a row are the same.
    ScreenStable,
}

/// How the wait ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Ended {
    Met,
    Timeout,
    /// niri's event stream was lost, so whether it happened is unknown.
    Uncertain,
}

#[derive(Debug, Serialize)]
pub(crate) struct Report {
    pub(crate) observed: Ended,
    /// The windows that met the condition.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) windows: Vec<u64>,
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
            Self::Window { .. } | Self::Closed(_) | Self::Title { .. } | Self::ScreenStable => {
                Ok(())
            }
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
        Until::ScreenStable => return None,
    };
    ids.sort_unstable();
    (!ids.is_empty()).then_some(ids)
}

/// Waits on the event stream for a window condition, up to `limit`.
pub(crate) async fn window(
    events: Option<&EventStream>,
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
        screenshot: None,
        screenshot_error: None,
    };
    Ok((report, shot))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::niri::waiter::tests::{view, window};

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
}
