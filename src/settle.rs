//! Waiting for the screen to stop changing. An input tool's `sent` only means niri handled
//! the input; the app redraws after that, at its own pace. A screenshot taken right away
//! can show the screen from before, so a capture counts as the result only once the next
//! one, taken at least `GAP` later, is the same image.

use std::time::Duration;

use tokio::time::Instant;

use crate::error::{CallError, ErrorName, ToolError};
use crate::observe::Screenshot;

/// A first redraw after input usually lands within a few frames.
const FIRST: Duration = Duration::from_millis(50);
/// Several frames at 60 Hz between capture starts. Equal samples do not prove semantic
/// readiness or exclude an animation that changes between samples.
const GAP: Duration = Duration::from_millis(100);
/// How long a screenshot asked for with an action waits for the screen to stop changing.
pub(crate) const LIMIT: Duration = Duration::from_millis(1500);

/// The last capture, and whether it matched the one before it.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Settled<T> {
    pub(crate) last: T,
    pub(crate) stable: bool,
}

/// Captures until two consecutive samples match, within a wall-clock budget including
/// the initial delay and capture work. On timeout, returns the last completed sample
/// unsettled, or a deadline error if none completed. Dropping a capture cancels its runner.
pub(crate) async fn settle<T, E, F>(
    mut capture: impl FnMut() -> F,
    same: impl Fn(&T, &T) -> bool,
    limit: Duration,
) -> Result<Settled<T>, E>
where
    F: Future<Output = Result<T, E>>,
    E: From<ToolError>,
{
    let deadline = Instant::now() + limit;
    tokio::time::sleep_until((Instant::now() + FIRST).min(deadline)).await;
    if Instant::now() >= deadline {
        return Err(no_sample().into());
    }
    let mut started = Instant::now();
    let mut last = tokio::time::timeout_at(deadline, Box::pin(capture()))
        .await
        .map_err(|_| E::from(no_sample()))??;
    if Instant::now() >= deadline {
        return Err(no_sample().into());
    }
    loop {
        tokio::time::sleep_until((started + GAP).min(deadline)).await;
        if Instant::now() >= deadline {
            break;
        }
        started = Instant::now();
        let next = tokio::time::timeout_at(deadline, Box::pin(capture())).await;
        let Ok(next) = next else { break };
        if Instant::now() >= deadline {
            break;
        }
        let next = next?;
        if same(&last, &next) {
            return Ok(Settled {
                last: next,
                stable: true,
            });
        }
        last = next;
    }
    Ok(Settled {
        last,
        stable: false,
    })
}

fn no_sample() -> ToolError {
    ToolError::new(
        ErrorName::DeadlineExceeded,
        "screenshot settle deadline exceeded before any capture completed",
    )
}

/// The last of the screenshots `settle` took, its metadata saying whether the screen had
/// stopped changing.
pub(crate) async fn screenshot<F>(
    capture: impl FnMut() -> F,
    limit: Duration,
) -> Result<Screenshot, CallError>
where
    F: Future<Output = Result<Screenshot, CallError>>,
{
    let same = |a: &Screenshot, b: &Screenshot| a.image == b.image;
    let settled = settle(capture, same, limit).await?;
    let mut shot = settled.last;
    shot.metadata.settled = Some(settled.stable);
    Ok(shot)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    async fn frame_after(delay: Duration) -> Result<u8, ToolError> {
        tokio::time::sleep(delay).await;
        Ok(1)
    }

    /// Captures that return `frames` in order, and the times they were taken, from the
    /// start of the test.
    async fn run(frames: &[u8], limit: Duration) -> (Settled<u8>, Vec<u128>) {
        let start = Instant::now();
        let taken = Mutex::new(Vec::new());
        let mut frames = frames.iter().copied();
        let settled = settle(
            || {
                taken.lock().unwrap().push(start.elapsed().as_millis());
                std::future::ready(Ok::<_, ToolError>(frames.next().unwrap()))
            },
            |a, b| a == b,
            limit,
        )
        .await
        .unwrap();
        (settled, taken.into_inner().unwrap())
    }

    #[tokio::test(start_paused = true)]
    async fn the_first_repeated_image_is_the_result() {
        let (settled, taken) = run(&[1, 2, 3, 3, 9], Duration::from_secs(2)).await;
        assert_eq!(
            settled,
            Settled {
                last: 3,
                stable: true
            }
        );
        // A first look after 50 ms, then one capture every 100 ms.
        assert_eq!(taken, [50, 150, 250, 350]);
    }

    #[tokio::test(start_paused = true)]
    async fn a_screen_that_keeps_changing_ends_at_the_limit_unsettled() {
        let frames: Vec<u8> = (0..100).collect();
        let (settled, taken) = run(&frames, Duration::from_millis(450)).await;
        assert!(!settled.stable);
        assert_eq!(taken, [50, 150, 250, 350]);
        assert_eq!(settled.last, 3);
    }

    #[tokio::test(start_paused = true)]
    async fn a_slow_later_capture_returns_the_last_complete_frame_at_the_deadline() {
        let start = Instant::now();
        let mut delays = [Duration::ZERO, Duration::from_secs(5)].into_iter();
        let result = settle(
            || frame_after(delays.next().unwrap()),
            |a, b| a == b,
            Duration::from_millis(450),
        )
        .await
        .unwrap();
        assert_eq!(start.elapsed(), Duration::from_millis(450));
        assert_eq!(
            result,
            Settled {
                last: 1,
                stable: false
            }
        );
        assert_eq!(delays.len(), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn a_slow_first_capture_fails_without_claiming_a_frame_was_observed() {
        let start = Instant::now();
        let result = settle(
            || async {
                tokio::time::sleep(Duration::from_secs(5)).await;
                Ok::<_, ToolError>(1_u8)
            },
            |a, b| a == b,
            Duration::from_millis(450),
        )
        .await
        .unwrap_err();
        assert_eq!(result.name, ErrorName::DeadlineExceeded);
        assert_eq!(start.elapsed(), Duration::from_millis(450));
    }

    #[tokio::test(start_paused = true)]
    async fn a_capture_finishing_exactly_at_the_deadline_is_not_a_stable_sample() {
        let mut delays = [Duration::ZERO, Duration::from_millis(300)].into_iter();
        let result = settle(
            || frame_after(delays.next().unwrap()),
            |a, b| a == b,
            Duration::from_millis(450),
        )
        .await
        .unwrap();
        assert!(!result.stable);
    }

    #[tokio::test(start_paused = true)]
    async fn a_failed_capture_ends_the_wait_with_its_error() {
        let error = ToolError::new(ErrorName::UpstreamError, "grim died");
        let mut captures = [Ok(1), Err(error.clone())].into_iter();
        let failed = settle(
            || std::future::ready(captures.next().unwrap()),
            |a: &u8, b: &u8| a == b,
            Duration::from_secs(1),
        )
        .await;
        assert_eq!(failed, Err(error));
    }
}
