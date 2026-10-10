//! `niri-computer-use paste-keeper`, which `paste` starts for one call (see `paste` for
//! the protocol). It saves the selection whole or refuses, takes it with the text, admits
//! the key on the server's `k` with `armed` while the text still holds the selection,
//! counts the reads that start after that, and restores the saved selection once the
//! target has read and gone quiet after `p`, or at once on `n`, when the server's stdin
//! ends before `k`, or when `k` doesn't come in time; a `k` after that gets no answer, so
//! no key goes out. After `k`, only `p` or `n` proves the key can't still arrive: if the
//! server ends or runs out of time without either, the keeper keeps the text, reports
//! `kept`, and serves it until another client takes the selection, rather than risk a late
//! key pasting the restored clipboard. It then serves the restored selection, without a
//! deadline, until another client takes it or niri goes away. Each step until then, and
//! each transfer, has a deadline; the keeper ends only once every transfer it accepted has
//! finished or reached its deadline.

use std::io::Write as _;
use std::os::fd::AsFd as _;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::AsyncReadExt as _;
use tokio::net::unix::pipe;
use tokio::task::JoinSet;
use tokio::time::Instant;

use super::paste::{Clipboard, MAX_TEXT, Report};
use crate::niri::selection::{self, Contents, Event, Selection, SourceId};
use crate::{Env, niri};

/// The types the text is offered as: those `wl-copy` offers for text.
const TEXT_TYPES: [&str; 5] = [
    "text/plain;charset=utf-8",
    "text/plain",
    "UTF8_STRING",
    "STRING",
    "TEXT",
];
/// The hint KDE's and other clipboard managers honour: don't keep this in the history.
const HINT: &str = "x-kde-passwordManagerHint";
const SECRET: &[u8] = b"secret";
/// The saved selection, all types together.
const MAX_SAVED: usize = 16 * 1024 * 1024;
/// From the start to the text, and from the text to `p` or `n`: the key's whole call.
const COMMAND_WAIT: Duration = Duration::from_secs(10);
/// After `p`, for the target's first read.
const READ_WAIT: Duration = Duration::from_secs(2);
/// After the latest read began, for another of the same paste.
const QUIET: Duration = Duration::from_millis(100);
/// Each transfer to a reader.
const TRANSFER: Duration = Duration::from_secs(2);
/// The longest `take` runs: niri's PID, then binding, saving, the check after the save and
/// taking the selection, each within its own deadline.
pub(crate) const TAKE: Duration = niri::PID_LIMIT.saturating_add(selection::STEP.saturating_mul(4));

pub(crate) async fn run(env: &Env) -> Result<(), String> {
    let mut commands = Commands::stdin()?;
    let text: Arc<[u8]> = tokio::time::timeout(COMMAND_WAIT, commands.text())
        .await
        .map_err(|_| format!("no text within {COMMAND_WAIT:?}"))??
        .into();
    let (mut selection, saved, paste) = match take(env).await {
        Ok(taken) => taken,
        Err(detail) => {
            say(&Report::Refused { detail });
            return Ok(());
        }
    };
    say(&Report::Ready);
    let mut transfers = Transfers::default();
    let watched = watch(&mut selection, commands, paste, &text, &mut transfers).await;
    let watched = match After::of(watched) {
        After::Keep(watched) => {
            say(&Report::Done {
                read: watched.reads > 0,
                clipboard: Clipboard::Kept,
                detail: Some(KEPT.to_owned()),
            });
            keep(&mut selection, watched, paste, &text, &mut transfers)
                .await
                .ok();
            transfers.finish().await;
            return Ok(());
        }
        After::Confirm(watched) => {
            confirm(&mut selection, watched, paste, &text, &mut transfers).await
        }
        After::Settled(watched) => watched,
    };
    let (read, clipboard, detail) = match watched {
        Err(detail) => (false, Clipboard::Failed, Some(detail)),
        Ok(watched) if watched.replaced => (watched.reads > 0, Clipboard::Replaced, None),
        Ok(watched) => {
            let read = watched.reads > 0;
            match restore(&mut selection, saved.as_ref()).await {
                Ok(None) => (read, Clipboard::Cleared, None),
                Ok(Some(restored)) => {
                    say(&Report::Done {
                        read,
                        clipboard: Clipboard::Restored,
                        detail: None,
                    });
                    // Nobody waits for this; it ends with the selection or with niri.
                    let saved = saved.unwrap_or_default();
                    serve(&mut selection, restored, &saved, &mut transfers)
                        .await
                        .ok();
                    transfers.finish().await;
                    return Ok(());
                }
                Err(detail) => (read, Clipboard::Failed, Some(detail)),
            }
        }
    };
    say(&Report::Done {
        read,
        clipboard,
        detail,
    });
    transfers.finish().await;
    Ok(())
}

/// Why the text stays.
const KEPT: &str = "the server neither confirmed the key nor said it didn't go out, so a late key could still paste; the pasted text stays on the clipboard instead of what was there before";

/// Binds, saves the selection, and takes it with a source offering the text, if
/// `previous` allows: within `TAKE`.
async fn take(env: &Env) -> Result<(Selection, Option<Contents>, SourceId), String> {
    let display = env.display.path().map_err(|error| error.detail.clone())?;
    let niri = niri::pid(&env.niri_socket)
        .await
        .map_err(|error| error.detail)?;
    let mut selection = Selection::bind(display, niri)
        .await
        .map_err(|error| error.detail)?;
    let saved = selection
        .save(MAX_SAVED)
        .await
        .map_err(|error| error.detail)?;
    let current = selection
        .unchanged_since(&saved)
        .await
        .map_err(|error| error.detail)?;
    let saved = previous(saved.contents, current)?;
    let mut types = TEXT_TYPES.to_vec();
    types.push(HINT);
    let paste = selection
        .offer(&types)
        .await
        .map_err(|error| error.detail)?;
    Ok((selection, saved, paste))
}

/// Decides from the selection saved, and whether it was `current`, still the selection
/// once the save ended, what the paste puts back afterwards, or why it refuses. A copy
/// made while the save ran refuses: the restore would put the older one back over it, and
/// the newer one may be a secret the check below never saw. A copy niri handles between
/// that check and the take is the race data-control can't exclude, since it has no
/// request that sets the selection only if it is still the one seen. A secret refuses.
fn previous(saved: Option<Contents>, current: bool) -> Result<Option<Contents>, String> {
    if !current {
        return Err(CHANGED.to_owned());
    }
    if saved.as_ref().is_some_and(secret) {
        return Err(format!(
            "the clipboard holds what its owner marked as a secret ({HINT}); restoring it would keep it past its owner's own clearing"
        ));
    }
    Ok(saved)
}

/// Why a copy made while saving refuses.
const CHANGED: &str = "something else was copied while the clipboard was being saved; nothing changed, so that copy stays";

fn secret(saved: &Contents) -> bool {
    saved
        .iter()
        .any(|(mime, bytes)| mime == HINT && bytes.as_ref() == SECRET)
}

/// What happened while the text held the selection.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Watched {
    /// Reads of the text that began after `k`.
    reads: usize,
    armed: bool,
    /// When `p` came.
    pasted: Option<Instant>,
    /// When the latest counted read began.
    latest: Option<Instant>,
    /// Another client took the selection.
    replaced: bool,
    /// The server said the key didn't go out.
    unsent: bool,
}

impl Watched {
    /// Serves and counts a read of the text, or notes that another client took the
    /// selection.
    fn handle(
        &mut self,
        event: Event,
        paste: SourceId,
        text: &Arc<[u8]>,
        transfers: &mut Transfers,
    ) {
        match event {
            Event::Send { source, mime, fd } if source == paste => {
                if self.armed {
                    self.reads += 1;
                    self.latest = Some(Instant::now());
                }
                let bytes = if mime == HINT {
                    SECRET.into()
                } else {
                    Arc::clone(text)
                };
                transfers.spawn(fd, bytes);
            }
            Event::Cancelled(source) if source == paste => self.replaced = true,
            // A source of ours from before, which is gone.
            Event::Send { .. } | Event::Cancelled(_) => {}
        }
    }

    /// Whether a `k` now admits the key: the text still holds the selection and the key
    /// wasn't admitted already.
    const fn admits(&self) -> bool {
        !self.armed && !self.replaced
    }

    /// The server's stdin ended. After `p` the wait for the read goes on; otherwise it ends
    /// now. Returns whether it ends now.
    const fn closed(&self) -> bool {
        self.pasted.is_none()
    }

    /// Whether a key may still arrive: `k` came, then neither `p` nor `n`, because the
    /// server ended or ran out of time.
    const fn abandoned(&self) -> bool {
        self.armed && self.pasted.is_none() && !self.unsent
    }

    /// When the wait ends unless something happens first: the server's whole call before
    /// `p`, the first read's wait after it, and the quiet time after a read.
    fn ends(&self, started: Instant) -> Instant {
        match (self.pasted, self.latest) {
            (None, _) => started + COMMAND_WAIT,
            (Some(pasted), None) => pasted + READ_WAIT,
            (Some(pasted), Some(latest)) => pasted.max(latest) + QUIET,
        }
    }
}

/// What follows the wait for the key, by how it ended.
#[derive(Debug, PartialEq, Eq)]
enum After {
    /// The key may still arrive: the text stays, and the saved selection isn't restored.
    Keep(Watched),
    /// The text may still hold the selection: check that it does, then restore.
    Confirm(Watched),
    /// Another client took the selection, or the wait failed: nothing to restore.
    Settled(Result<Watched, String>),
}

impl After {
    fn of(watched: Result<Watched, String>) -> Self {
        match watched {
            Ok(watched) if watched.replaced => Self::Settled(Ok(watched)),
            Ok(watched) if watched.abandoned() => Self::Keep(watched),
            Ok(watched) => Self::Confirm(watched),
            Err(detail) => Self::Settled(Err(detail)),
        }
    }
}

/// Serves the text's reads until the wait ends, the server says the key didn't go out or
/// ends before `p`, or another client takes the selection. It takes the server's commands
/// and closes them as it ends, so no `k` is admitted after it.
async fn watch(
    selection: &mut Selection,
    mut commands: Commands,
    paste: SourceId,
    text: &Arc<[u8]>,
    transfers: &mut Transfers,
) -> Result<Watched, String> {
    let started = Instant::now();
    let mut watched = Watched::default();
    let mut open = true;
    loop {
        tokio::select! {
            command = commands.next(), if open => match command {
                Command::Arm => {
                    watched = admit(selection, watched, paste, text, transfers).await?;
                    if watched.replaced {
                        return Ok(watched);
                    }
                }
                Command::Pasted => watched.pasted = Some(Instant::now()),
                Command::Unsent => {
                    watched.unsent = true;
                    return Ok(watched);
                }
                Command::End => {
                    open = false;
                    if watched.closed() {
                        return Ok(watched);
                    }
                }
            },
            event = selection.next() => {
                watched.handle(event.map_err(|error| error.detail)?, paste, text, transfers);
                if watched.replaced {
                    return Ok(watched);
                }
            }
            () = tokio::time::sleep_until(watched.ends(started)) => return Ok(watched),
        }
    }
}

/// Admits the key: once the reads niri sent before now are handled, and only while the
/// text still holds the selection and the key wasn't admitted already, arms the count and
/// says `armed`. The server sends the key only on that report, so a `k` that comes after
/// `watch` ended, which nothing reads, or one that finds the text replaced, sends nothing.
async fn admit(
    selection: &mut Selection,
    watched: Watched,
    paste: SourceId,
    text: &Arc<[u8]>,
    transfers: &mut Transfers,
) -> Result<Watched, String> {
    if !watched.admits() {
        return Ok(watched);
    }
    let mut watched = confirm(selection, watched, paste, text, transfers).await?;
    if watched.admits() {
        watched.armed = true;
        say(&Report::Armed);
    }
    Ok(watched)
}

/// Handles what niri sent before now, so a replacement already on its way counts: the end
/// of stdin or the quiet time doesn't prove the text still holds the selection. A
/// replacement niri handles after this is the race that data-control can't exclude, since
/// it has no request that sets the selection only if it is still ours.
async fn confirm(
    selection: &mut Selection,
    mut watched: Watched,
    paste: SourceId,
    text: &Arc<[u8]>,
    transfers: &mut Transfers,
) -> Result<Watched, String> {
    for event in selection.pending().await.map_err(|error| error.detail)? {
        watched.handle(event, paste, text, transfers);
    }
    Ok(watched)
}

/// Serves the text until another client takes the selection.
async fn keep(
    selection: &mut Selection,
    mut watched: Watched,
    paste: SourceId,
    text: &Arc<[u8]>,
    transfers: &mut Transfers,
) -> Result<(), String> {
    while !watched.replaced {
        let event = selection.next().await.map_err(|error| error.detail)?;
        watched.handle(event, paste, text, transfers);
    }
    Ok(())
}

/// Offers the saved selection again, or clears the selection when nothing was saved.
async fn restore(
    selection: &mut Selection,
    saved: Option<&Contents>,
) -> Result<Option<SourceId>, String> {
    let Some(saved) = saved else {
        selection.clear().await.map_err(|error| error.detail)?;
        return Ok(None);
    };
    let types: Vec<&str> = saved.iter().map(|(mime, _)| mime.as_str()).collect();
    selection
        .offer(&types)
        .await
        .map(Some)
        .map_err(|error| error.detail)
}

/// Serves the restored selection's reads until another client takes it.
async fn serve(
    selection: &mut Selection,
    restored: SourceId,
    saved: &Contents,
    transfers: &mut Transfers,
) -> Result<(), String> {
    loop {
        match selection.next().await.map_err(|error| error.detail)? {
            Event::Send { source, mime, fd } if source == restored => {
                if let Some((_, bytes)) = saved.iter().find(|(offered, _)| *offered == mime) {
                    transfers.spawn(fd, Arc::clone(bytes));
                }
            }
            Event::Cancelled(source) if source == restored => return Ok(()),
            Event::Send { .. } | Event::Cancelled(_) => {}
        }
    }
}

/// The transfers this keeper accepted. Ending the process would cut them short, so it
/// waits for each to finish or reach its own deadline first.
#[derive(Debug, Default)]
struct Transfers(JoinSet<()>);

impl Transfers {
    /// Writes to a reader in a task of its own, so a slow reader holds nothing else up. A
    /// reader that goes away just ends its transfer.
    fn spawn(&mut self, fd: std::os::fd::OwnedFd, bytes: Arc<[u8]>) {
        while self.0.try_join_next().is_some() {}
        self.0.spawn(async move {
            selection::write(fd, &bytes, TRANSFER).await.ok();
        });
    }

    async fn finish(mut self) {
        while self.0.join_next().await.is_some() {}
    }
}

/// Writes one report line. A server that went away reads nothing more, and its stdin's
/// end has already told this keeper what to do.
#[expect(
    clippy::disallowed_methods,
    reason = "the keeper's stdout is its pipe to the server, not the MCP transport"
)]
fn say(report: &Report) {
    let Ok(line) = serde_json::to_string(report) else {
        return;
    };
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "{line}")
        .and_then(|()| stdout.flush())
        .ok();
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Command {
    Arm,
    Pasted,
    /// The key didn't go out.
    Unsent,
    /// End of file, or anything unknown.
    End,
}

/// The server's side of the protocol, on this process's stdin, a pipe.
struct Commands(pipe::Receiver);

impl Commands {
    fn stdin() -> Result<Self, String> {
        let stdin = std::io::stdin()
            .as_fd()
            .try_clone_to_owned()
            .map_err(|error| format!("use stdin: {error}"))?;
        pipe::Receiver::from_owned_fd(stdin)
            .map(Self)
            .map_err(|error| format!("stdin isn't a pipe: {error}"))
    }

    async fn text(&mut self) -> Result<Vec<u8>, String> {
        let mut length = [0; 8];
        self.0
            .read_exact(&mut length)
            .await
            .map_err(|error| format!("read the text's length: {error}"))?;
        let length = usize::try_from(u64::from_le_bytes(length)).unwrap_or(usize::MAX);
        if length > MAX_TEXT {
            return Err(format!("{length} bytes of text; at most {MAX_TEXT}"));
        }
        let mut text = vec![0; length];
        self.0
            .read_exact(&mut text)
            .await
            .map_err(|error| format!("read the text: {error}"))?;
        Ok(text)
    }

    /// The next command; anything but `k`, `p` or `n` counts as end of file.
    async fn next(&mut self) -> Command {
        let mut byte = [0];
        match self.0.read(&mut byte).await {
            Ok(1) => command(byte[0]),
            _ => Command::End,
        }
    }
}

const fn command(byte: u8) -> Command {
    match byte {
        b'k' => Command::Arm,
        b'p' => Command::Pasted,
        b'n' => Command::Unsent,
        _ => Command::End,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waits_for_the_server_then_the_read_then_quiet() {
        let started = Instant::now();
        let mut watched = Watched::default();
        assert_eq!(watched.ends(started), started + COMMAND_WAIT);
        let pasted = started + Duration::from_millis(30);
        watched.pasted = Some(pasted);
        assert_eq!(watched.ends(started), pasted + READ_WAIT);
        let read = pasted + Duration::from_millis(5);
        watched.latest = Some(read);
        assert_eq!(watched.ends(started), read + QUIET);
    }

    #[test]
    fn a_read_before_the_servers_p_still_waits_out_the_quiet_time_after_p() {
        // The target may read before `p` arrives: the key went out before `p` was sent.
        let started = Instant::now();
        let read = started + Duration::from_millis(10);
        let pasted = started + Duration::from_millis(20);
        let watched = Watched {
            pasted: Some(pasted),
            latest: Some(read),
            ..Watched::default()
        };
        assert_eq!(watched.ends(started), pasted + QUIET);
    }

    #[test]
    fn after_k_only_p_or_n_lets_the_clipboard_be_restored() {
        // The end of stdin before `k` ends the wait, and the clipboard is restored.
        let before_k = Watched::default();
        assert!(before_k.closed());
        assert_eq!(After::of(Ok(before_k)), After::Confirm(before_k));
        // `k`, then the end of stdin: the server ended with the key perhaps on its way, so
        // the wait ends and the text stays.
        let armed = Watched {
            armed: true,
            ..Watched::default()
        };
        assert!(armed.closed());
        assert_eq!(After::of(Ok(armed)), After::Keep(armed));
        let unsent = Watched {
            unsent: true,
            ..armed
        };
        assert_eq!(After::of(Ok(unsent)), After::Confirm(unsent));
        // After `p` the key is done, and the end of stdin only stops the commands.
        let pasted = Watched {
            pasted: Some(Instant::now()),
            ..armed
        };
        assert!(!pasted.closed());
        assert_eq!(After::of(Ok(pasted)), After::Confirm(pasted));
        // A copy made meanwhile stays, whatever the server said.
        let replaced = Watched {
            replaced: true,
            ..armed
        };
        assert_eq!(After::of(Ok(replaced)), After::Settled(Ok(replaced)));
    }

    #[test]
    fn a_k_admits_the_key_once_and_only_while_the_text_holds_the_selection() {
        assert!(Watched::default().admits());
        let armed = Watched {
            armed: true,
            ..Watched::default()
        };
        assert!(!armed.admits());
        let replaced = Watched {
            replaced: true,
            ..Watched::default()
        };
        assert!(!replaced.admits());
    }

    #[test]
    fn only_n_ends_the_call_at_once_and_anything_unknown_counts_as_the_end() {
        assert_eq!(command(b'k'), Command::Arm);
        assert_eq!(command(b'p'), Command::Pasted);
        assert_eq!(command(b'n'), Command::Unsent);
        assert_eq!(command(b'x'), Command::End);
    }

    fn saved(types: &[(&str, &[u8])]) -> Contents {
        types
            .iter()
            .map(|(mime, bytes)| ((*mime).to_owned(), Arc::from(*bytes)))
            .collect()
    }

    #[test]
    fn the_saved_selection_is_put_back_and_nothing_saved_leaves_it_empty() {
        let copied = saved(&[("text/plain", b"copied")]);
        assert_eq!(previous(Some(copied.clone()), true), Ok(Some(copied)));
        assert_eq!(previous(None, true), Ok(None));
    }

    /// A copy made while the save ran refuses, whatever was saved: restoring would put the
    /// older selection back over it.
    #[test]
    fn a_copy_made_while_saving_refuses() {
        for saved in [None, Some(saved(&[("text/plain", b"older")]))] {
            assert_eq!(previous(saved, false), Err(CHANGED.to_owned()));
        }
    }

    #[test]
    fn a_secret_marked_by_its_owner_is_not_kept() {
        let refused = previous(
            Some(saved(&[("text/plain", b"hunter2"), (HINT, SECRET)])),
            true,
        );
        assert!(refused.is_err_and(|detail| detail.contains("marked as a secret")));
        let other = saved(&[("text/plain", b"hunter2"), (HINT, b"other")]);
        assert_eq!(previous(Some(other.clone()), true), Ok(Some(other)));
    }
}
