//! `paste`'s work: text pasted through the clipboard, which is then put back. A keeper
//! process, our own binary's `paste-keeper` started through the runner, saves every MIME
//! type of the current selection, takes the selection with the text and reports `ready`.
//! The server then presses the paste combination through `keyboard::type_input`, with all
//! of its gates, and tells the keeper whether it went out. The keeper waits for the
//! target's read, puts the saved selection back and keeps serving it, as `wl-copy` would,
//! until another client takes the selection. A stop, a cancelled call, a failed key or the
//! server's end closes the keeper's stdin, and it restores at once.
//!
//! The protocol, server to keeper: the text's length as 8 little-endian bytes and the
//! text; `k` just before the key; then `p` once the key went out, or end of file. Keeper
//! to server: one JSON `Report` per line, `ready` or `refused`, then `done`.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader, Lines};
use tokio::process::{ChildStdin, ChildStdout};

use crate::act::Outcome;
use crate::error::{CallError, ErrorName, ToolError};
use crate::niri;
use crate::policy;
use crate::runner;

use super::Input;
use super::keyboard::{self, Expect, Typing};

/// `paste`'s text, in bytes.
pub(crate) const MAX_TEXT: usize = 1024 * 1024;
/// Binding and saving the selection, each within niri's 2 s deadline, then taking it.
const READY: Duration = Duration::from_secs(5);
/// The keeper's wait for the read, its quiet time and the restore's round trip.
const DONE: Duration = Duration::from_secs(5);

/// What became of the clipboard the call found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Clipboard {
    /// Every MIME type it offered is offered again.
    Restored,
    /// Nothing was selected, and nothing is again.
    Cleared,
    /// Another client took the selection before the restore, so theirs stays.
    Replaced,
    /// Restoring failed; `detail` says why.
    Failed,
    /// The keeper didn't report; `detail` says why.
    Unknown,
}

/// `paste`'s part of the outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub(crate) struct Pasted {
    /// Whether a client read the pasted text after the key; null when the keeper didn't
    /// report.
    pub(crate) read: Option<bool>,
    pub(crate) clipboard: Clipboard,
}

/// One line from the keeper.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "report")]
pub(crate) enum Report {
    /// The selection is saved and holds the text.
    Ready,
    /// The selection couldn't be saved whole; nothing changed.
    Refused { detail: String },
    Done {
        read: bool,
        clipboard: Clipboard,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
}

/// Pastes `text` with the combination `combo` into the focused app, which `expect` must
/// name.
pub(crate) async fn paste(
    input: Input<'_>,
    text: &str,
    combo: &str,
    expect: Expect,
) -> Result<Outcome, CallError> {
    check_text(text)?;
    // The clipboard isn't touched for a paste the key's own checks would refuse.
    let waiter = niri::waiter(input.niri.events).await?;
    keyboard::check_expect(&expect, waiter.view())?;
    if let Some(refused) = policy::refuse_input(input.policy, super::focused_app_id(waiter.view()))
    {
        return Err(refused.into());
    }
    drop(waiter);
    let mut keeper = Keeper::start(text.as_bytes()).await?;
    keeper.arm().await?;
    let typed = keyboard::type_input(input, Typing::Keys(vec![combo.to_owned()]), expect).await;
    let went_out = matches!(&typed, Ok(outcome) if outcome.pressed.is_none());
    let done = keeper.finish(went_out).await;
    combined(typed, done)
}

fn check_text(text: &str) -> Result<(), CallError> {
    if text.is_empty() {
        return Err(CallError::InvalidArguments(
            "`text` must not be empty".to_owned(),
        ));
    }
    if text.len() > MAX_TEXT {
        return Err(ToolError::new(
            ErrorName::TextTooLong,
            format!(
                "{} bytes; at most {MAX_TEXT} per call; nothing was pasted",
                text.len()
            ),
        )
        .into());
    }
    Ok(())
}

/// The key's outcome with what the keeper reported, or the key's error with it.
fn combined(
    typed: Result<Outcome, CallError>,
    done: Result<Report, ToolError>,
) -> Result<Outcome, CallError> {
    let (pasted, detail) = match done {
        Ok(Report::Done {
            read,
            clipboard,
            detail,
        }) => (
            Pasted {
                read: Some(read),
                clipboard,
            },
            detail,
        ),
        Ok(other) => (unknown(), Some(format!("the keeper reported {other:?}"))),
        Err(error) => (unknown(), Some(error.detail)),
    };
    match typed {
        Ok(outcome) => Ok(Outcome {
            paste: Some(pasted),
            detail: joined(outcome.detail, detail),
            ..outcome
        }),
        Err(CallError::Tool(error)) => Err(ToolError::new(
            error.name,
            format!("{}; {}", error.detail, said(pasted, detail.as_deref())),
        )
        .into()),
        Err(mistake @ CallError::InvalidArguments(_)) => Err(mistake),
    }
}

const fn unknown() -> Pasted {
    Pasted {
        read: None,
        clipboard: Clipboard::Unknown,
    }
}

fn joined(first: Option<String>, second: Option<String>) -> Option<String> {
    match (first, second) {
        (Some(first), Some(second)) => Some(format!("{first}; {second}")),
        (first, second) => first.or(second),
    }
}

/// What became of the clipboard, in words, for an error's detail.
fn said(pasted: Pasted, detail: Option<&str>) -> String {
    let detail = detail.unwrap_or("no detail");
    match pasted.clipboard {
        Clipboard::Restored => "the clipboard was restored".to_owned(),
        Clipboard::Cleared => "the clipboard is empty again, as it was".to_owned(),
        Clipboard::Replaced => {
            "another client took the clipboard meanwhile, so it wasn't restored".to_owned()
        }
        Clipboard::Failed => format!("restoring the clipboard failed: {detail}"),
        Clipboard::Unknown => format!("the clipboard keeper didn't report: {detail}"),
    }
}

/// The server's end of a keeper.
struct Keeper {
    stdin: ChildStdin,
    lines: Lines<BufReader<ChildStdout>>,
}

impl Keeper {
    /// Starts a keeper with `text`, and waits until it holds the selection.
    async fn start(text: &[u8]) -> Result<Self, ToolError> {
        let started = runner::companion("/proc/self/exe", &["paste-keeper".to_owned()])?;
        let mut keeper = Self {
            stdin: started.stdin,
            lines: BufReader::new(started.stdout).lines(),
        };
        let length = u64::try_from(text.len()).unwrap_or(u64::MAX).to_le_bytes();
        let ready = async {
            keeper.stdin.write_all(&length).await.map_err(lost)?;
            keeper.stdin.write_all(text).await.map_err(lost)?;
            keeper.report().await
        };
        match within(READY, ready).await? {
            Report::Ready => Ok(keeper),
            Report::Refused { detail } => Err(ToolError::new(
                ErrorName::ClipboardUnsaved,
                format!("{detail}; the clipboard is unchanged and nothing was pasted"),
            )),
            other @ Report::Done { .. } => Err(unexpected(&other)),
        }
    }

    /// Tells the keeper the key is about to go out, so reads from now on count.
    async fn arm(&mut self) -> Result<(), ToolError> {
        self.stdin.write_all(b"k").await.map_err(|error| {
            ToolError::new(
                ErrorName::UpstreamError,
                format!(
                    "the clipboard keeper ended before the key ({error}), so nothing was pasted; the clipboard may be empty"
                ),
            )
        })
    }

    /// Says whether the key went out, or ends the keeper's stdin when it didn't, and waits
    /// for its last report.
    async fn finish(mut self, went_out: bool) -> Result<Report, ToolError> {
        if went_out {
            self.stdin.write_all(b"p").await.map_err(lost)?;
        } else {
            self.stdin.shutdown().await.map_err(lost)?;
        }
        within(DONE, self.report()).await
    }

    async fn report(&mut self) -> Result<Report, ToolError> {
        let line = self
            .lines
            .next_line()
            .await
            .map_err(lost)?
            .ok_or_else(|| lost("it ended without a report"))?;
        serde_json::from_str(&line).map_err(|error| {
            ToolError::new(
                ErrorName::UpstreamError,
                format!("the clipboard keeper's report {line:?}: {error}"),
            )
        })
    }
}

async fn within<T>(
    deadline: Duration,
    work: impl Future<Output = Result<T, ToolError>>,
) -> Result<T, ToolError> {
    tokio::time::timeout(deadline, work).await.map_err(|_| {
        ToolError::new(
            ErrorName::DeadlineExceeded,
            format!("the clipboard keeper didn't report within {deadline:?}"),
        )
    })?
}

fn lost(error: impl std::fmt::Display) -> ToolError {
    ToolError::new(
        ErrorName::UpstreamError,
        format!("the clipboard keeper: {error}"),
    )
}

fn unexpected(report: &Report) -> ToolError {
    ToolError::new(
        ErrorName::UpstreamError,
        format!("the clipboard keeper reported {report:?} out of turn"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::act::Observed;

    fn sent() -> Outcome {
        Outcome {
            observed: Observed::Sent,
            detail: None,
            ..Outcome::uncertain(Some(true), None, String::new())
        }
    }

    #[test]
    fn reports_read_one_json_line_each() {
        let done = Report::Done {
            read: true,
            clipboard: Clipboard::Restored,
            detail: None,
        };
        let line = serde_json::to_string(&done).unwrap();
        assert_eq!(
            line,
            r#"{"report":"done","read":true,"clipboard":"restored"}"#
        );
        assert_eq!(serde_json::from_str::<Report>(&line).unwrap(), done);
        assert_eq!(
            serde_json::from_str::<Report>(r#"{"report":"ready"}"#).unwrap(),
            Report::Ready
        );
    }

    #[test]
    fn the_outcome_says_what_became_of_the_clipboard() {
        let done = Report::Done {
            read: false,
            clipboard: Clipboard::Failed,
            detail: Some("niri went away".to_owned()),
        };
        let pasted = combined(Ok(sent()), Ok(done)).unwrap();
        assert_eq!(
            pasted.paste,
            Some(Pasted {
                read: Some(false),
                clipboard: Clipboard::Failed
            })
        );
        assert_eq!(pasted.detail.as_deref(), Some("niri went away"));
    }

    #[test]
    fn a_failed_key_keeps_its_error_and_says_the_clipboard_was_restored() {
        let refused = ToolError::new(ErrorName::FocusMismatch, "expected app_id \"a\"");
        let done = Report::Done {
            read: false,
            clipboard: Clipboard::Restored,
            detail: None,
        };
        let Err(CallError::Tool(error)) = combined(Err(refused.into()), Ok(done)) else {
            panic!("the key's error is lost");
        };
        assert_eq!(error.name, ErrorName::FocusMismatch);
        assert_eq!(
            error.detail,
            "expected app_id \"a\"; the clipboard was restored"
        );
    }

    #[test]
    fn a_keeper_that_never_reported_leaves_the_clipboard_unknown() {
        let lost = ToolError::new(ErrorName::DeadlineExceeded, "no report");
        let pasted = combined(Ok(sent()), Err(lost)).unwrap();
        assert_eq!(pasted.paste, Some(unknown()));
        assert_eq!(pasted.detail.as_deref(), Some("no report"));
    }

    #[test]
    fn text_is_capped_in_bytes_and_must_not_be_empty() {
        assert!(check_text(&"é".repeat(MAX_TEXT / 2)).is_ok());
        let Err(CallError::Tool(error)) = check_text(&"a".repeat(MAX_TEXT + 1)) else {
            panic!("over the cap");
        };
        assert_eq!(error.name, ErrorName::TextTooLong);
        assert!(matches!(
            check_text(""),
            Err(CallError::InvalidArguments(_))
        ));
    }
}
