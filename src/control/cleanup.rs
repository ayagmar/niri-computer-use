//! Input cleanups that outlive the call that started them: a wtype still typing after its
//! call was dropped, and the releases a dropped gesture or native key press sent, until
//! niri has handled them and the input-dirty marker is off. The server waits for them
//! before it exits, so a client's end can't cut one short and leave the marker behind.

use std::sync::LazyLock;
use std::time::Duration;

use tokio::sync::watch;

/// How long the server waits for them at its end: the longest any of them takes. That is a
/// native paste's: two seconds each for niri to handle the release and to send the keymap
/// back, five for the keeper's report, and half a second for the marker's lock. A wtype
/// paste takes at most three seconds for wtype, which include the keeper's admission, five
/// for the report and the lock's half second; other input, at most four and a half.
const LIMIT: Duration = Duration::from_secs(10);

static PENDING: LazyLock<watch::Sender<usize>> = LazyLock::new(|| watch::Sender::new(0));

/// One cleanup, counted while it is held.
#[derive(Debug)]
pub(crate) struct Pending(());

impl Pending {
    pub(crate) fn start() -> Self {
        PENDING.send_modify(|count| *count = count.saturating_add(1));
        Self(())
    }
}

impl Drop for Pending {
    fn drop(&mut self) {
        PENDING.send_modify(|count| *count = count.saturating_sub(1));
    }
}

/// How many cleanups are pending now.
pub(crate) fn pending() -> usize {
    *PENDING.borrow()
}

/// Waits until no cleanup is pending, or `LIMIT` has passed.
pub(crate) async fn settled() {
    let mut pending = PENDING.subscribe();
    let none = pending.wait_for(|count| *count == 0);
    // Past the limit, a cleanup left undone keeps the marker, for `recover`.
    if let Ok(done) = tokio::time::timeout(LIMIT, none).await {
        drop(done);
    }
}
