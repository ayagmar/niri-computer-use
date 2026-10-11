//! The clipboard as one lease sees it: a data-control connection bound when the lease is
//! taken, which notes every selection niri announces from then on, so that `clipboard_read`
//! returns only what was copied during the lease. `acquire_desktop` starts it and waits
//! until it is bound, so whatever the agent copies after that call counts. It ends within
//! `TICK` once the lease is gone, however it went: `release_desktop`, a stop, a moved lease
//! file or the session's end. Reads go through the same connection,
//! so what the ledger says and the offer read are the same selection.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

use super::{Clipboard, read};
use crate::error::{ErrorName, ToolError};
use crate::input::keeper::PASTE;
use crate::niri::selection::{self, Selection};
use crate::niri::{self, Display, Socket};

/// How often the watcher asks whether its lease still holds.
const TICK: Duration = Duration::from_millis(250);
/// The most reads waiting for the watcher: one session's calls.
const QUEUE: usize = crate::session::MAX_IN_FLIGHT;
/// The longest a read waits for its answer: the round trip that catches up with niri,
/// then the hint and the text, each within a step's deadline, and a little for the queue.
const ANSWER: Duration = selection::STEP
    .saturating_mul(2)
    .saturating_add(Duration::from_millis(500));

type Reply = oneshot::Sender<Result<Clipboard, ToolError>>;

/// A watcher for one lease. Dropping it ends the watcher and closes its connection.
#[derive(Debug)]
pub(crate) struct Watch {
    lease: u64,
    reader: Reader,
    task: JoinHandle<()>,
}

/// Asks a watcher for the clipboard.
#[derive(Debug, Clone)]
pub(crate) struct Reader {
    requests: mpsc::Sender<Reply>,
    /// Why the watcher ended, once it failed.
    failure: Arc<Mutex<Option<ToolError>>>,
}

impl Watch {
    /// Starts watching for `lease` while `holds` says it still holds: binds to niri's
    /// display, within niri's PID's and the bind's deadlines, and returns once bound, then
    /// notes every selection. A bind that fails leaves a watch that answers every read with
    /// the bind's error.
    pub(crate) async fn start(
        socket: Socket,
        display: Display,
        lease: u64,
        holds: impl Fn() -> bool + Send + 'static,
    ) -> Self {
        let (requests, received) = mpsc::channel(QUEUE);
        let reader = Reader {
            requests,
            failure: Arc::new(Mutex::new(None)),
        };
        let failure = Arc::clone(&reader.failure);
        let (bound, binding) = oneshot::channel();
        let task = tokio::spawn(async move {
            let watched = match bind(&socket, &display).await {
                Ok(selection) => {
                    bound.send(()).ok();
                    run(selection, received, holds).await
                }
                Err(error) => Err(error),
            };
            keep(&failure, watched);
        });
        // A failed bind drops `bound` once its error is kept.
        binding.await.ok();
        Self {
            lease,
            reader,
            task,
        }
    }

    /// The lease this watches for.
    pub(crate) const fn lease(&self) -> u64 {
        self.lease
    }

    pub(crate) fn reader(&self) -> Reader {
        self.reader.clone()
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Reader {
    /// The clipboard's text, if it was copied during the lease, within `ANSWER`.
    pub(crate) async fn read(&self) -> Result<Clipboard, ToolError> {
        let (reply, answer) = oneshot::channel();
        if self.requests.try_send(reply).is_err() {
            return Err(self.ended());
        }
        match tokio::time::timeout(ANSWER, answer).await {
            Ok(Ok(read)) => read,
            Ok(Err(_)) => Err(self.ended()),
            Err(_) => Err(ToolError::new(
                ErrorName::DeadlineExceeded,
                format!("the clipboard didn't answer within {ANSWER:?}"),
            )),
        }
    }

    /// Why the watcher can't answer.
    fn ended(&self) -> ToolError {
        self.failure
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            .unwrap_or_else(|| {
                ToolError::new(
                    ErrorName::UpstreamError,
                    "the clipboard watcher is busy or has ended; read it again",
                )
            })
    }
}

/// Keeps why the watcher ended, if it failed.
fn keep(failure: &Mutex<Option<ToolError>>, ended: Result<(), ToolError>) {
    if let Err(error) = ended {
        *failure.lock().unwrap_or_else(PoisonError::into_inner) = Some(error);
    }
}

async fn bind(socket: &Socket, display: &Display) -> Result<Selection, ToolError> {
    let display = display.path().map_err(Clone::clone)?;
    Selection::bind(display, niri::pid(socket).await?).await
}

/// What woke the watcher.
enum Woken {
    Tick,
    Read(Option<Reply>),
    Changed(Result<(), ToolError>),
}

/// Notes each selection niri announces and answers each read, until the lease is gone or
/// the connection fails.
async fn run(
    mut selection: Selection,
    mut requests: mpsc::Receiver<Reply>,
    holds: impl Fn() -> bool,
) -> Result<(), ToolError> {
    selection.record();
    let mut ledger = Ledger::default();
    let mut tick = tokio::time::interval(TICK);
    loop {
        let woken = tokio::select! {
            _ = tick.tick() => Woken::Tick,
            request = requests.recv() => Woken::Read(request),
            changed = selection.changed() => Woken::Changed(changed),
        };
        match woken {
            Woken::Tick if !holds() => return Ok(()),
            Woken::Tick => {}
            Woken::Read(None) => return Ok(()),
            Woken::Read(Some(reply)) => answer(&mut selection, &mut ledger, reply).await?,
            Woken::Changed(changed) => {
                changed?;
                ledger.note_all(selection.announced());
            }
        }
    }
}

/// Catches up with every selection niri announced before now, then reads the current one
/// for `reply`, unless the caller has gone. A failed read is the caller's answer; a lost
/// connection ends the watcher too.
async fn answer(
    selection: &mut Selection,
    ledger: &mut Ledger,
    mut reply: Reply,
) -> Result<(), ToolError> {
    if let Err(error) = selection.sync().await {
        reply.send(Err(error.clone())).ok();
        return Err(error);
    }
    ledger.note_all(selection.announced());
    let during = ledger.during;
    let read = tokio::select! {
        read = async {
            if during {
                read(selection).await
            } else {
                Ok(Clipboard::withheld("copied_before_lease"))
            }
        } => Some(read),
        () = reply.closed() => None,
    };
    if let Some(read) = read {
        reply.send(read).ok();
    }
    Ok(())
}

/// Whether the current selection was copied during the lease, from the selections niri
/// announced since the lease began, which was then current.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Ledger {
    during: bool,
    /// While the current selection is a paste's text: whether the selection it replaced
    /// was copied during the lease. The selection after the text, the keeper's restore or
    /// another copy, counts as that one did, so a restore never turns the user's earlier
    /// clipboard into a copy of the lease's.
    pasted_over: Option<bool>,
}

impl Ledger {
    fn note_all(&mut self, announced: Vec<Option<Vec<String>>>) {
        for types in announced {
            self.note(types.as_deref());
        }
    }

    /// Notes one selection announced, with its types, or `None` for nothing selected.
    fn note(&mut self, types: Option<&[String]>) {
        if types.is_some_and(|types| types.iter().any(|mime| mime == PASTE)) {
            self.pasted_over = Some(self.pasted_over.unwrap_or(self.during));
            self.during = true;
        } else {
            self.during = self.pasted_over.take().unwrap_or(true);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(ledger: &mut Ledger, types: &[&str]) {
        let types: Vec<String> = types.iter().map(|mime| (*mime).to_owned()).collect();
        ledger.note(Some(&types));
    }

    const COPY: [&str; 1] = ["text/plain"];
    const PASTED: [&str; 2] = ["text/plain", PASTE];

    #[test]
    fn only_a_copy_made_during_the_lease_counts() {
        let mut ledger = Ledger::default();
        assert!(!ledger.during, "the clipboard the lease began with");
        note(&mut ledger, &COPY);
        assert!(ledger.during);
        ledger.note(None);
        assert!(ledger.during, "cleared during the lease");
    }

    /// `paste` puts the user's earlier clipboard back after its text: that restore is a
    /// selection announced during the lease, but still the user's.
    #[test]
    fn a_paste_restore_counts_as_what_the_paste_replaced() {
        let mut ledger = Ledger::default();
        note(&mut ledger, &PASTED);
        note(&mut ledger, &COPY);
        assert!(!ledger.during, "the user's earlier copy, restored");

        note(&mut ledger, &COPY);
        assert!(ledger.during, "a copy made after the restore");
        note(&mut ledger, &PASTED);
        note(&mut ledger, &COPY);
        assert!(ledger.during, "the lease's own copy, restored");
    }

    /// A later paste replaces a kept paste's text; what follows still counts as what the
    /// first paste replaced.
    #[test]
    fn pastes_in_a_row_keep_the_first_ones_origin() {
        let mut ledger = Ledger::default();
        note(&mut ledger, &PASTED);
        note(&mut ledger, &PASTED);
        assert!(ledger.during, "a paste's text is the agent's own");
        ledger.note(None);
        assert!(
            !ledger.during,
            "cleared after the user's clipboard was dropped"
        );
    }
}
