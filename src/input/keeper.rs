//! `niri-computer-use paste-keeper`, which `paste` starts for one call (see `paste` for
//! the protocol). It saves the selection whole or refuses, takes it with the text, counts
//! the reads that start after the server's `k`, and restores the saved selection once the
//! target has read and gone quiet, or at once when the server's stdin ends. It then serves
//! the restored selection, without a deadline, until another client takes it or niri goes
//! away. Each step until then, and each transfer, has a deadline; the keeper ends only
//! once every transfer it accepted has finished or reached its deadline.

use std::io::Write as _;
use std::os::fd::AsFd as _;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::AsyncReadExt as _;
use tokio::net::unix::pipe;
use tokio::task::JoinSet;
use tokio::time::Instant;

use super::paste::{Clipboard, MAX_TEXT, Report};
use crate::Env;
use crate::niri::selection::{self, Contents, Event, Selection, SourceId};

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
/// From the start to the text, and from the text to `p` or the end of stdin: the key's
/// whole call.
const COMMAND_WAIT: Duration = Duration::from_secs(10);
/// After `p`, for the target's first read.
const READ_WAIT: Duration = Duration::from_secs(2);
/// After the latest read began, for another of the same paste.
const QUIET: Duration = Duration::from_millis(100);
/// Each transfer to a reader.
const TRANSFER: Duration = Duration::from_secs(2);

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
    let watched = match watch(&mut selection, &mut commands, paste, &text, &mut transfers).await {
        Ok(watched) if !watched.replaced => {
            confirm(&mut selection, watched, paste, &text, &mut transfers).await
        }
        other => other,
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

/// Binds, saves the selection, and takes it with a source offering the text.
async fn take(env: &Env) -> Result<(Selection, Option<Contents>, SourceId), String> {
    let display = env
        .wayland_socket()
        .ok_or("WAYLAND_DISPLAY or XDG_RUNTIME_DIR is not set")?;
    let niri = crate::niri::pid(&env.niri_socket)
        .await
        .map_err(|error| error.detail)?;
    let mut selection = Selection::bind(&display, niri)
        .await
        .map_err(|error| error.detail)?;
    let saved = selection
        .save(MAX_SAVED)
        .await
        .map_err(|error| error.detail)?;
    if saved.as_ref().is_some_and(secret) {
        return Err(format!(
            "the clipboard holds what its owner marked as a secret ({HINT}); restoring it would keep it past its owner's own clearing"
        ));
    }
    let mut types = TEXT_TYPES.to_vec();
    types.push(HINT);
    let paste = selection
        .offer(&types)
        .await
        .map_err(|error| error.detail)?;
    Ok((selection, saved, paste))
}

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

/// Serves the text's reads until the wait ends, the server's stdin ends, or another
/// client takes the selection.
async fn watch(
    selection: &mut Selection,
    commands: &mut Commands,
    paste: SourceId,
    text: &Arc<[u8]>,
    transfers: &mut Transfers,
) -> Result<Watched, String> {
    let started = Instant::now();
    let mut watched = Watched::default();
    loop {
        tokio::select! {
            command = commands.next() => match command {
                Command::Arm => watched.armed = true,
                Command::Pasted => watched.pasted = Some(Instant::now()),
                Command::End => return Ok(watched),
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

    /// The next command; anything but `k` or `p` ends the call, as end of file does.
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
    fn only_k_and_p_keep_the_call_going() {
        assert_eq!(command(b'k'), Command::Arm);
        assert_eq!(command(b'p'), Command::Pasted);
        assert_eq!(command(b'x'), Command::End);
    }

    #[test]
    fn a_secret_marked_by_its_owner_is_not_kept() {
        let saved = |hint: &[u8]| -> Contents {
            vec![
                ("text/plain".to_owned(), Arc::from(&b"hunter2"[..])),
                (HINT.to_owned(), Arc::from(hint)),
            ]
        };
        assert!(secret(&saved(b"secret")));
        assert!(!secret(&saved(b"other")));
    }
}
