//! Waiting for the screen to stop changing. An input tool's `sent` only means niri handled
//! the input; the app redraws after that, at its own pace. A screenshot taken right away
//! can show the screen from before, so a capture counts as the result only once the next
//! one, taken at least `GAP` later, is the same image.

use std::time::Duration;

use tokio::time::Instant;

use crate::error::CallError;
use crate::observe::Screenshot;

/// A first redraw after input usually lands within a few frames.
const FIRST: Duration = Duration::from_millis(50);
/// Several frames at 60 Hz: a screen unchanged over this long has stopped redrawing for
/// the input, not just between two frames of it.
const GAP: Duration = Duration::from_millis(100);
/// How long a screenshot asked for with an action waits for the screen to stop changing.
pub(crate) const LIMIT: Duration = Duration::from_millis(1500);

/// The last capture, and whether it matched the one before it.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Settled<T> {
    pub(crate) last: T,
    pub(crate) stable: bool,
}

/// Captures until two captures in a row are the same, or `limit` has passed since the
/// first; then the last one is the result, `stable` or not. Something that keeps moving,
/// such as a spinner or a video, never settles, so the limit bounds the wait.
pub(crate) async fn settle<T, E, F>(
    mut capture: impl FnMut() -> F,
    same: impl Fn(&T, &T) -> bool,
    limit: Duration,
) -> Result<Settled<T>, E>
where
    F: Future<Output = Result<T, E>>,
{
    tokio::time::sleep(FIRST).await;
    let deadline = Instant::now() + limit;
    let mut started = Instant::now();
    let mut last = capture().await?;
    loop {
        tokio::time::sleep_until(started + GAP).await;
        started = Instant::now();
        let next = capture().await?;
        if same(&last, &next) {
            return Ok(Settled {
                last: next,
                stable: true,
            });
        }
        last = next;
        if Instant::now() >= deadline {
            return Ok(Settled {
                last,
                stable: false,
            });
        }
    }
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

    /// Captures that return `frames` in order, and the times they were taken, from the
    /// start of the test.
    async fn run(frames: &[u8], limit: Duration) -> (Settled<u8>, Vec<u128>) {
        let start = Instant::now();
        let taken = Mutex::new(Vec::new());
        let mut frames = frames.iter().copied();
        let settled = settle(
            || {
                taken.lock().unwrap().push(start.elapsed().as_millis());
                std::future::ready(Ok::<_, ()>(frames.next().unwrap()))
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
        assert_eq!(taken.last(), Some(&550));
        assert_eq!(settled.last, 5);
    }

    #[tokio::test(start_paused = true)]
    async fn a_failed_capture_ends_the_wait_with_its_error() {
        let mut captures = [Ok(1), Err("grim died")].into_iter();
        let failed = settle(
            || std::future::ready(captures.next().unwrap()),
            |a: &u8, b: &u8| a == b,
            Duration::from_secs(1),
        )
        .await;
        assert_eq!(failed, Err("grim died"));
    }
}
