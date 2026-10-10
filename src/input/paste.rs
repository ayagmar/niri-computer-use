//! `paste`'s work: text pasted through the clipboard, which is then put back. A keeper
//! process, our own binary's `paste-keeper` started through the runner, saves every MIME
//! type of the current selection, takes the selection with the text and reports `ready`.
//! The server then presses the paste combination through `keyboard::type_input_then`,
//! with all of its gates, and tells the keeper whether it went out. The keeper waits for
//! the target's read, puts the saved selection back and keeps serving it, as `wl-copy`
//! would, until another client takes the selection. A key that never went out restores
//! at once.
//!
//! Whoever commits the key, right before it can go out, takes the keeper's end as an
//! `Aftercare`: wtype's task, or the native device. It says `k` and waits for the keeper to
//! admit the key; without that, the key doesn't go out. Then it sends the key, says `p`
//! even if the key failed partway, and only then takes its input-dirty marker off, so a
//! stop, a cancelled call or the session's end can't drop the keeper's end while a key may
//! still arrive, and no new input, from this server or another, starts until the keeper
//! has reported. A call dropped before the commit drops the keeper, which restores at once.
//!
//! The protocol, server to keeper: the text's length as 8 little-endian bytes and the
//! text; `k` just before the key; then `p` once the key went out, or `n` if it didn't.
//! End of file before `k` means no key. After `k`, the key may still come without `p` or
//! `n`, so the keeper keeps the text on the clipboard rather than restore it. Keeper to
//! server: one JSON `Report` per line, `ready` or `refused`, then `armed` for a `k` that
//! came while the keeper still waited for it with the text on the clipboard, and `done`.
//! A keeper that stopped waiting has restored the clipboard and said `done`, and never
//! answers a later `k` with `armed`.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader, Lines};
use tokio::process::{ChildStdin, ChildStdout};
use tokio::sync::oneshot;

use crate::act::Outcome;
use crate::error::{CallError, ErrorName, ToolError};
use crate::niri;
use crate::policy;
use crate::runner;

use super::Input;
use super::keeper;
use super::keyboard::{self, Expect, Typing};

/// `paste`'s text, in bytes.
pub(crate) const MAX_TEXT: usize = 1024 * 1024;
/// The keeper's start and its read of the text, which the server writes at once.
const START: Duration = Duration::from_secs(1);
/// The keeper's start, then its `take`, which ends with `ready` or `refused` within
/// `keeper::TAKE` whenever niri answers slowly: 11 s.
const READY: Duration = START.saturating_add(keeper::TAKE);
/// The keeper's wait for the read, its quiet time and the restore's round trip.
const DONE: Duration = Duration::from_secs(5);
/// From `k` to `armed`: the keeper answers after one round trip to niri, which takes
/// milliseconds. Well inside wtype's three seconds, which run while wtype waits at its
/// gate for this, and short enough that a keeper that can't answer fails the call quickly.
const ADMIT: Duration = Duration::from_millis(500);

/// What became of the clipboard the call found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Clipboard {
    /// Every MIME type it offered is offered again.
    Restored,
    /// Nothing was selected, and nothing is again; or an earlier paste's text was, which
    /// a `kept` outcome left, and it is dropped, as `detail` says.
    Cleared,
    /// Another client took the selection before the restore, so theirs stays.
    Replaced,
    /// Restoring failed; `detail` says why.
    Failed,
    /// The key may have gone out, but the server couldn't say, so the pasted text stays
    /// and the clipboard isn't restored; `detail` says why.
    Kept,
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
    /// The key may go out: the text holds the selection, and reads from now on count.
    Armed,
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
    input.display.checked(input.niri.socket).await?;
    let keeper = Keeper::start(text.as_bytes()).await?;
    let (report, done) = oneshot::channel();
    let mut aftercare = Some(Aftercare { keeper, report });
    let keys = Typing::Keys(vec![combo.to_owned()]);
    let typed = keyboard::type_input_then(input, keys, expect, &mut aftercare).await;
    let done = match aftercare {
        // Nothing committed the key, so it never went out.
        Some(aftercare) => aftercare.unsent().await,
        None => done.await.unwrap_or_else(|_| Err(lost(DROPPED))),
    };
    combined(typed, done)
}

/// Why a committed key's keeper never reported.
const DROPPED: &str = "the key's cleanup ended without its report: the key may have gone out, so the keeper keeps the pasted text on the clipboard";

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
    let given = detail.unwrap_or("no detail");
    match pasted.clipboard {
        Clipboard::Restored => "the clipboard was restored".to_owned(),
        Clipboard::Cleared => detail.map_or_else(
            || "the clipboard is empty again, as it was".to_owned(),
            |detail| format!("the clipboard is empty: {detail}"),
        ),
        Clipboard::Replaced => {
            "another client took the clipboard meanwhile, so it wasn't restored".to_owned()
        }
        Clipboard::Failed => format!("restoring the clipboard failed: {given}"),
        Clipboard::Kept => format!("the pasted text stays on the clipboard: {given}"),
        Clipboard::Unknown => format!("the clipboard keeper didn't report: {given}"),
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
        Self::ready(started, text).await
    }

    /// Gives the started keeper `text`, and waits until it holds the selection.
    async fn ready(started: runner::Companion, text: &[u8]) -> Result<Self, ToolError> {
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
            other @ (Report::Armed | Report::Done { .. }) => Err(unexpected(&other)),
        }
    }

    /// Says `command`, `p` or `n`, and waits for the keeper's last report. An `armed` that
    /// came too late for the key is passed over.
    async fn finish(mut self, command: &[u8]) -> Result<Report, ToolError> {
        let last = async {
            let mut report = self.tell(command).await?;
            while report == Report::Armed {
                report = self.report().await?;
            }
            Ok(report)
        };
        within(DONE, last).await
    }

    /// Says `command` and reads the next report. A keeper that has ended can't read it,
    /// but may have reported before it did.
    async fn tell(&mut self, command: &[u8]) -> Result<Report, ToolError> {
        match self.stdin.write_all(command).await {
            Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => {}
            written => written.map_err(lost)?,
        }
        self.report().await
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

/// A paste's keeper, taken by whoever commits its key, right before the key can go out.
/// Dropped after `arm`, the keeper keeps the pasted text, since the key may still arrive.
pub(crate) struct Aftercare {
    keeper: Keeper,
    report: oneshot::Sender<Result<Report, ToolError>>,
}

impl Aftercare {
    /// Tells the keeper the key is about to go out, and returns once it has admitted the
    /// key, within `ADMIT`. Otherwise the key must not go out: the keeper hears `n` unless
    /// it already reported, its last report goes to the call, and the error says nothing
    /// was pasted.
    pub(crate) async fn arm(mut self) -> Result<Self, ToolError> {
        let refused = match within(ADMIT, self.keeper.tell(b"k")).await {
            Ok(Report::Armed) => return Ok(self),
            Ok(done @ Report::Done { .. }) => {
                self.report.send(Ok(done)).ok();
                return Err(unadmitted("it had stopped waiting for the key"));
            }
            Ok(other) => format!("it reported {other:?}"),
            Err(error) => error.detail,
        };
        let done = self.keeper.finish(b"n").await;
        self.report.send(done).ok();
        Err(unadmitted(&refused))
    }

    /// Says the key went out, or may have, and hands the keeper's last report, once it has
    /// read and restored, to the call.
    pub(crate) async fn sent(self) {
        let done = self.keeper.finish(b"p").await;
        self.report.send(done).ok();
    }

    /// Says the key never went out, so the keeper restores at once.
    async fn unsent(self) -> Result<Report, ToolError> {
        self.keeper.finish(b"n").await
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

/// The key wasn't sent, because the keeper didn't admit it.
fn unadmitted(why: &str) -> ToolError {
    ToolError::new(
        ErrorName::UpstreamError,
        format!(
            "the clipboard keeper didn't admit the key ({why}), so the key wasn't sent and nothing was pasted"
        ),
    )
}

fn unexpected(report: &Report) -> ToolError {
    ToolError::new(
        ErrorName::UpstreamError,
        format!("the clipboard keeper reported {report:?} out of turn"),
    )
}

/// A paste's aftercare whose keeper is `sh -c script`, which reads the 9 bytes of the
/// text `x` and answers `ready` first, and the receiver of its last report.
#[cfg(test)]
pub(super) async fn fake_aftercare(
    script: &str,
) -> (Aftercare, oneshot::Receiver<Result<Report, ToolError>>) {
    let script = format!(
        "dd bs=1 count=9 status=none >/dev/null; echo '{{\"report\":\"ready\"}}'; {script}"
    );
    let started = runner::companion("sh", &["-c".to_owned(), script]).unwrap();
    let keeper = Keeper::ready(started, b"x").await.unwrap();
    let (report, done) = oneshot::channel();
    (Aftercare { keeper, report }, done)
}

/// A keeper's script that, like one whose wait for the key ended, has restored the
/// clipboard and reported, and answers nothing more.
#[cfg(test)]
pub(super) const STOPPED_WAITING: &str =
    r#"echo '{"report":"done","read":false,"clipboard":"restored"}'; exec cat >/dev/null"#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::act::Observed;

    #[tokio::test]
    async fn a_keeper_that_stopped_waiting_for_the_key_admits_none() {
        let (aftercare, done) = fake_aftercare(STOPPED_WAITING).await;
        let Err(error) = aftercare.arm().await else {
            panic!("a keeper that restored the clipboard admitted the key");
        };
        assert!(
            error.detail.contains("nothing was pasted"),
            "{}",
            error.detail
        );
        let Err(CallError::Tool(error)) = combined(Err(error.into()), done.await.unwrap()) else {
            panic!("the refusal is lost");
        };
        assert!(
            error.detail.ends_with("; the clipboard was restored"),
            "{}",
            error.detail
        );
    }

    #[tokio::test]
    async fn a_key_the_keeper_admits_too_late_isnt_sent_and_the_keeper_hears_n() {
        // `armed` comes past `ADMIT`; the keeper then restores on `n`.
        let script = r#"dd bs=1 count=1 status=none >/dev/null; sleep 0.8; echo '{"report":"armed"}'; c=$(dd bs=1 count=1 status=none); echo "{\"report\":\"done\",\"read\":false,\"clipboard\":\"restored\",\"detail\":\"after $c\"}""#;
        let (aftercare, done) = fake_aftercare(script).await;
        let Err(error) = aftercare.arm().await else {
            panic!("a key admitted too late went out");
        };
        assert!(
            error.detail.contains("nothing was pasted"),
            "{}",
            error.detail
        );
        assert_eq!(
            done.await.unwrap().unwrap(),
            Report::Done {
                read: false,
                clipboard: Clipboard::Restored,
                detail: Some("after n".to_owned())
            }
        );
    }

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

    /// A clipboard cleared of an earlier paste's text wasn't empty before: the error says
    /// what was dropped rather than that the clipboard is as it was.
    #[test]
    fn a_failed_key_after_an_earlier_paste_says_its_text_was_dropped() {
        let refused = ToolError::new(ErrorName::FocusMismatch, "expected app_id \"a\"");
        let done = Report::Done {
            read: false,
            clipboard: Clipboard::Cleared,
            detail: Some("an earlier paste's text was dropped".to_owned()),
        };
        let Err(CallError::Tool(error)) = combined(Err(refused.into()), Ok(done)) else {
            panic!("the key's error is lost");
        };
        assert_eq!(
            error.detail,
            "expected app_id \"a\"; the clipboard is empty: an earlier paste's text was dropped"
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
