//! Input cleanups that outlive the call that started them: a wtype still typing after its
//! call was dropped, and the releases a dropped gesture or native key press sent, until
//! niri has handled them and the input-dirty marker is off. The server waits for them
//! before it exits, so a client's end can't cut one short and leave the marker behind.
//!
//! Each runs in a task of its own through `spawn`, and ends by its deadline, `LIMIT` after
//! it started, or after the cleanup it takes over started. So the server's wait, as long
//! as `LIMIT`, outlasts every one of them, and a cleanup past its deadline is dropped with
//! whatever it held: the marker it would have cleared stays, for `recover`.

use std::sync::LazyLock;
use std::time::Duration;

use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::error::{ErrorName, ToolError};

/// How long a cleanup may run, and how long the server waits for them at its end. Every
/// step of a cleanup has a deadline of its own, and steps that all came close to theirs
/// would add up to more: a native paste's are a `KeyboardLayouts` request and up to four
/// Wayland round trips of two seconds each, the keeper's report in six and a half and the marker's
/// lock in half a second. Each takes milliseconds when niri and the keeper answer.
pub(crate) const LIMIT: Duration = Duration::from_secs(10);

static PENDING: LazyLock<watch::Sender<usize>> = LazyLock::new(|| watch::Sender::new(0));

/// One cleanup, counted while it is held.
#[derive(Debug)]
struct Pending(());

impl Pending {
    fn start() -> Self {
        PENDING.send_modify(|count| *count = count.saturating_add(1));
        Self(())
    }
}

impl Drop for Pending {
    fn drop(&mut self) {
        PENDING.send_modify(|count| *count = count.saturating_sub(1));
    }
}

/// The deadline of a cleanup that starts now.
pub(crate) fn deadline() -> Instant {
    Instant::now() + LIMIT
}

/// Runs `work` in a task of its own, counted as pending until it ends, and drops it at
/// `ends`, which fails it with `deadline_exceeded`.
pub(crate) fn spawn<T: Send + 'static>(
    ends: Instant,
    work: impl Future<Output = Result<T, ToolError>> + Send + 'static,
) -> JoinHandle<Result<T, ToolError>> {
    let pending = Pending::start();
    tokio::spawn(async move {
        let done = tokio::time::timeout_at(ends, work).await;
        drop(pending);
        done.unwrap_or_else(|_| Err(over()))
    })
}

fn over() -> ToolError {
    ToolError::new(
        ErrorName::DeadlineExceeded,
        format!(
            "the input cleanup didn't finish within {LIMIT:?} of its start, so the input-dirty marker stays until the user runs `niri-computer-use recover`"
        ),
    )
}

/// How many cleanups are pending now.
pub(crate) fn pending() -> usize {
    *PENDING.borrow()
}

/// Waits until no cleanup is pending, or `LIMIT` has passed: by then each has ended.
pub(crate) async fn settled() {
    let mut pending = PENDING.subscribe();
    let none = pending.wait_for(|count| *count == 0);
    if let Ok(done) = tokio::time::timeout(LIMIT, none).await {
        drop(done);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    /// A cleanup that never ends on its own, such as one whose niri stopped answering just
    /// inside each step's deadline, ends at its deadline, before the server's wait does,
    /// and drops what it held.
    #[tokio::test(start_paused = true)]
    async fn a_cleanup_past_its_deadline_is_dropped_with_what_it_held() {
        let held = Arc::new(());
        let marker = Arc::clone(&held);
        let started = Instant::now();
        let cleanup = spawn(deadline(), async move {
            std::future::pending::<()>().await;
            drop(marker);
            Ok(())
        });
        let error = cleanup.await.unwrap().unwrap_err();
        assert_eq!(started.elapsed(), LIMIT);
        assert_eq!(error.name, ErrorName::DeadlineExceeded);
        assert!(error.detail.contains("recover"), "{}", error.detail);
        assert_eq!(Arc::strong_count(&held), 1, "the cleanup still holds it");
    }
}
