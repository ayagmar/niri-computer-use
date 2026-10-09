//! `shell_open` and `shell_close`: change which Noctalia panel is open, then poll
//! Noctalia's `status` until `activePanelId` shows the change (plan §6.1). Noctalia sends
//! no events, so polling is the only way to observe it. Nothing is retried.

use std::time::Duration;

use serde::Serialize;

use super::{Niri, Observed, Outcome};
use crate::Env;
use crate::error::{CallError, ToolError, Unanswered};
use crate::niri;
use crate::noctalia::{self, Command};
use crate::policy::Panel;

/// How often Noctalia's status is read while waiting.
const POLL: Duration = Duration::from_millis(100);
/// How long a panel change has to show in `activePanelId`.
const WAIT: Duration = Duration::from_secs(2);

/// What Noctalia reported when the observation ended.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Shell {
    /// The open panel's id, or null when none is open.
    pub(crate) active_panel: Option<String>,
}

/// Opens `panel`; `opened` once Noctalia reports it as the active panel.
pub(crate) async fn open(env: &Env, niri: Niri<'_>, panel: Panel) -> Result<Outcome, CallError> {
    let is_open = |active: Option<&str>| active == Some(panel.id());
    change(
        env,
        niri,
        Command::PanelOpen(panel),
        is_open,
        Observed::Opened,
    )
    .await
}

/// Closes `panel`; `closed` once Noctalia no longer reports it as the active panel.
/// Another panel being open counts: `panel` is closed.
pub(crate) async fn close(env: &Env, niri: Niri<'_>, panel: Panel) -> Result<Outcome, CallError> {
    let is_closed = |active: Option<&str>| active != Some(panel.id());
    change(
        env,
        niri,
        Command::PanelClose(panel),
        is_closed,
        Observed::Closed,
    )
    .await
}

/// Sends `command` unless `done` already holds, then waits until it does.
async fn change(
    env: &Env,
    niri: Niri<'_>,
    command: Command,
    done: impl Fn(Option<&str>) -> bool + Send + Sync,
    observed: Observed,
) -> Result<Outcome, CallError> {
    // Also tells a Noctalia that isn't running from one that refused the command.
    let before = active_panel(env).await?;
    if done(before.as_deref()) {
        let shell = Shell {
            active_panel: before,
        };
        return Ok(outcome(Some(false), observed, Some(shell), niri).await);
    }
    match noctalia::change_panel(env, command).await {
        Ok(()) => {}
        Err(Unanswered::Refused(error)) => return Err(error.into()),
        Err(Unanswered::Lost(error)) => {
            let mut lost = outcome(None, Observed::Uncertain, None, niri).await;
            lost.detail = Some(error.detail);
            return Ok(lost);
        }
    }
    let watched = watch(|| active_panel(env), done, observed).await;
    let mut seen = outcome(Some(true), watched.observed, watched.shell, niri).await;
    seen.detail = watched.detail;
    Ok(seen)
}

async fn active_panel(env: &Env) -> Result<Option<String>, ToolError> {
    noctalia::active_panel(&noctalia::status(env).await?)
}

/// How a wait for a panel change ended.
#[derive(Debug, PartialEq, Eq)]
struct Watched {
    observed: Observed,
    /// The last `activePanelId` read, unless the last read failed.
    shell: Option<Shell>,
    /// Why the outcome is uncertain.
    detail: Option<String>,
}

/// Reads the active panel every 100 ms until `done` holds (`observed`), a read fails
/// (`uncertain`), or two seconds pass (`timeout`).
async fn watch<F>(
    mut read: impl FnMut() -> F + Send,
    done: impl Fn(Option<&str>) -> bool + Send + Sync,
    observed: Observed,
) -> Watched
where
    F: Future<Output = Result<Option<String>, ToolError>> + Send,
{
    let mut last = None;
    let polled = tokio::time::timeout(WAIT, async {
        let mut poll = tokio::time::interval(POLL);
        loop {
            poll.tick().await;
            match read().await {
                Ok(active) if done(active.as_deref()) => return Ok(active),
                Ok(active) => last = active,
                Err(error) => return Err(error),
            }
        }
    })
    .await;
    match polled {
        Ok(Ok(active)) => Watched {
            observed,
            shell: Some(Shell {
                active_panel: active,
            }),
            detail: None,
        },
        Ok(Err(error)) => Watched {
            observed: Observed::Uncertain,
            shell: None,
            detail: Some(error.detail),
        },
        Err(_) => Watched {
            observed: Observed::Timeout,
            shell: Some(Shell { active_panel: last }),
            detail: None,
        },
    }
}

/// An outcome with the window niri has focused now: none while a panel holds keyboard
/// focus. A lost event stream only loses that field.
async fn outcome(
    accepted: Option<bool>,
    observed: Observed,
    shell: Option<Shell>,
    niri: Niri<'_>,
) -> Outcome {
    let focused_window = niri::waiter(niri.events)
        .await
        .ok()
        .and_then(|waiter| waiter.view().focused_window());
    Outcome {
        accepted,
        observed,
        focused_window,
        windows: Vec::new(),
        focus: None,
        shell,
        detail: None,
        screenshot: None,
        screenshot_error: None,
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::VecDeque;

    use super::*;
    use crate::error::ErrorName;

    /// Reads the replies in order, repeating the last one.
    fn replies(
        replies: Vec<Result<Option<&str>, ToolError>>,
    ) -> impl FnMut() -> std::future::Ready<Result<Option<String>, ToolError>> {
        let replies = RefCell::new(VecDeque::from(replies));
        move || {
            let mut replies = replies.borrow_mut();
            let reply = if replies.len() > 1 {
                replies.pop_front()
            } else {
                replies.front().cloned()
            };
            std::future::ready(reply.unwrap().map(|active| active.map(str::to_owned)))
        }
    }

    fn opened(active: Option<&str>) -> bool {
        active == Some("wallpaper")
    }

    #[tokio::test(start_paused = true)]
    async fn the_change_is_seen_once_noctalia_reports_it() {
        let started = tokio::time::Instant::now();
        let seen = watch(
            replies(vec![Ok(None), Ok(None), Ok(Some("wallpaper"))]),
            opened,
            Observed::Opened,
        )
        .await;
        assert_eq!(
            seen,
            Watched {
                observed: Observed::Opened,
                shell: Some(Shell {
                    active_panel: Some("wallpaper".to_owned())
                }),
                detail: None
            }
        );
        // Read at once, then every 100 ms.
        assert_eq!(started.elapsed(), Duration::from_millis(200));
    }

    #[tokio::test(start_paused = true)]
    async fn after_two_seconds_it_times_out_with_the_last_panel_seen() {
        let started = tokio::time::Instant::now();
        let seen = watch(
            replies(vec![Ok(Some("control-center"))]),
            opened,
            Observed::Opened,
        )
        .await;
        assert_eq!(
            seen,
            Watched {
                observed: Observed::Timeout,
                shell: Some(Shell {
                    active_panel: Some("control-center".to_owned())
                }),
                detail: None
            }
        );
        assert_eq!(started.elapsed(), WAIT);
    }

    #[tokio::test(start_paused = true)]
    async fn losing_noctalia_while_waiting_is_uncertain() {
        let gone = ToolError::new(ErrorName::NoctaliaUnavailable, "connect: refused");
        let seen = watch(replies(vec![Ok(None), Err(gone)]), opened, Observed::Opened).await;
        assert_eq!(
            seen,
            Watched {
                observed: Observed::Uncertain,
                shell: None,
                detail: Some("connect: refused".to_owned())
            }
        );
    }
}
