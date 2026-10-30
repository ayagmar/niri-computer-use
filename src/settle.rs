//! Waiting for the screen to stop changing. An input tool's `sent` only means niri handled
//! the input; the app redraws after that, at its own pace. A screenshot taken right away
//! can show the screen from before, so the server samples the focused output, each sample
//! at least `GAP` after the last, until it has stopped changing, then captures the image
//! the result returns.
//!
//! Samples are raw pixels (`ppm`), compared pixel by pixel. The screen counts as still once
//! two samples in a row are the same, or two comparisons in a row changed at most `QUIET`
//! of the output's pixels: a clock, a CPU graph, a spinner or a caret is that small, and a
//! popup, a page load or a dialog animating in is larger. Where grim gives no raw pixels,
//! the samples are the JPEGs the result returns, compared by their bytes, as before.

use std::time::Duration;

use tokio::time::Instant;

use crate::error::{CallError, ErrorName, ToolError};
use crate::observe::Screenshot;
use crate::ppm;

/// A first redraw after input usually lands within a few frames.
const FIRST: Duration = Duration::from_millis(50);
/// Several frames at 60 Hz between capture starts. Equal samples do not prove semantic
/// readiness or exclude an animation that changes between samples.
const GAP: Duration = Duration::from_millis(100);
/// How long a screenshot asked for with an action waits for the screen to stop changing.
pub(crate) const LIMIT: Duration = Duration::from_millis(1500);
/// The largest part of the output's pixels a comparison may change and still count as
/// still: 0.5 %, about 68x68 pixels of a 1280x720 sample.
pub(crate) const QUIET: f64 = 0.005;

/// The last sample; whether the samples stopped changing; what part of the pixels changed
/// between the last two, when that is known; and, once they stopped, when the sample that
/// began the still run started: the screen changed no more than `QUIET` from then on.
#[derive(Debug, PartialEq)]
pub(crate) struct Settled<T> {
    pub(crate) last: T,
    pub(crate) stable: bool,
    pub(crate) change: Option<f64>,
    pub(crate) since: Option<Instant>,
}

/// The comparisons in a row that changed at most `QUIET`, and when the sample that began
/// them started.
#[derive(Debug, Default)]
struct Run {
    quiet: u32,
    since: Option<Instant>,
}

impl Run {
    /// Counts the comparison of the sample started at `previous` with the next one, which
    /// changed `change` of the pixels, `None` when that isn't known, and says whether the
    /// samples have stopped changing: two the same, or two quiet comparisons in a row.
    fn add(&mut self, change: Option<f64>, previous: Instant) -> bool {
        match change {
            Some(change) if change <= QUIET => {
                if self.quiet == 0 {
                    self.since = Some(previous);
                }
                self.quiet += 1;
                change == 0.0 || self.quiet >= 2
            }
            Some(_) | None => {
                *self = Self::default();
                false
            }
        }
    }
}

/// Captures until the samples stop changing (`Run::add`), as `compare` measures two in a
/// row, within a wall-clock budget including the initial delay and capture work. On
/// timeout, returns the last completed sample unsettled, or a deadline error if none
/// completed. Dropping a capture cancels its runner.
pub(crate) async fn settle<T, E, F>(
    mut capture: impl FnMut() -> F,
    compare: impl Fn(&T, &T) -> Option<f64>,
    limit: Duration,
) -> Result<Settled<T>, E>
where
    F: Future<Output = Result<T, E>>,
    E: From<ToolError>,
{
    let deadline = Instant::now() + limit;
    tokio::time::sleep_until((Instant::now() + FIRST).min(deadline)).await;
    let mut started = Instant::now();
    let mut last = tokio::time::timeout_at(deadline, Box::pin(capture()))
        .await
        .map_err(|_| E::from(no_sample()))??;
    let (mut run, mut stable, mut change) = (Run::default(), false, None);
    loop {
        let expired = Instant::now() >= deadline;
        if stable || expired {
            let stable = stable && !expired;
            return Ok(Settled {
                last,
                stable,
                change,
                since: run.since.filter(|_| stable),
            });
        }
        tokio::select! {
            biased;
            () = tokio::time::sleep_until(deadline) => continue,
            () = tokio::time::sleep_until(started + GAP) => {},
        }
        let previous = std::mem::replace(&mut started, Instant::now());
        if let Ok(next) = tokio::time::timeout_at(deadline, Box::pin(capture())).await {
            let next = next?;
            change = compare(&last, &next);
            stable = run.add(change, previous);
            last = next;
        }
    }
}

fn no_sample() -> ToolError {
    ToolError::new(
        ErrorName::DeadlineExceeded,
        "screenshot settle deadline exceeded before any capture completed",
    )
}

/// One sample of a settle wait: the focused output's raw pixels, or the JPEG the result
/// would return, where grim gave no raw pixels.
#[derive(Debug)]
pub(crate) enum Sample {
    Pixels(Vec<u8>),
    Image(Box<Screenshot>),
}

/// What part of the pixels changed from `a` to `b`: counted for two samples of raw pixels
/// of one size, 0 for two JPEGs with the same bytes, and unknown otherwise.
pub(crate) fn compare(a: &Sample, b: &Sample) -> Option<f64> {
    match (a, b) {
        (Sample::Pixels(a), Sample::Pixels(b)) => {
            let (a, b) = (ppm::parse(a)?, ppm::parse(b)?);
            Some(fraction(ppm::changed(&a, &b)?, ppm::pixels(&a)))
        }
        (Sample::Image(a), Sample::Image(b)) => (a.image == b.image).then_some(0.0),
        (Sample::Pixels(_), Sample::Image(_)) | (Sample::Image(_), Sample::Pixels(_)) => None,
    }
}

#[expect(
    clippy::cast_precision_loss,
    reason = "pixel counts of an image fit in f64's 53 bits"
)]
fn fraction(changed: u64, of: u64) -> f64 {
    changed as f64 / of as f64
}

/// Samples until they stop changing, then the screenshot the result returns: the last
/// sample when it is a JPEG, or else the one `whole` captures, after the wait. Its
/// metadata says whether the samples stopped changing, what part of the pixels changed
/// between the last two, and, for refs, since when the screen was still.
pub(crate) async fn screenshot<F, G>(
    sample: impl FnMut() -> F,
    whole: impl FnOnce() -> G,
    limit: Duration,
) -> Result<Screenshot, CallError>
where
    F: Future<Output = Result<Sample, CallError>>,
    G: Future<Output = Result<Screenshot, CallError>>,
{
    finish(settle(sample, compare, limit).await?, whole).await
}

/// The screenshot a settle wait that ended as `settled` returns, as for `screenshot`.
pub(crate) async fn finish<G>(
    settled: Settled<Sample>,
    whole: impl FnOnce() -> G,
) -> Result<Screenshot, CallError>
where
    G: Future<Output = Result<Screenshot, CallError>>,
{
    let mut shot = match settled.last {
        Sample::Image(shot) => *shot,
        Sample::Pixels(_) => whole().await?,
    };
    shot.metadata.settled = Some(settled.stable);
    shot.metadata.changing = settled.change;
    shot.metadata.steady_since = settled.since;
    Ok(shot)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use niri_ipc::{LogicalOutput, Transform};

    use super::*;
    use crate::observe::{Metadata, Rect};

    #[expect(
        clippy::trivially_copy_pass_by_ref,
        reason = "settle passes the samples it compares by reference"
    )]
    fn exact(a: &u8, b: &u8) -> Option<f64> {
        (a == b).then_some(0.0)
    }

    /// Never settled: no comparison, or none that ended the wait.
    const fn unsettled(last: u8, change: Option<f64>) -> Settled<u8> {
        Settled {
            last,
            stable: false,
            change,
            since: None,
        }
    }

    async fn frame_after(delay: Duration) -> Result<u8, ToolError> {
        tokio::time::sleep(delay).await;
        Ok(1)
    }

    /// Captures that return `frames` in order, and the times they were taken, from the
    /// start of the test.
    async fn run(frames: &[u8], limit: Duration) -> (Settled<u8>, Vec<u128>, Instant) {
        let start = Instant::now();
        let taken = Mutex::new(Vec::new());
        let mut frames = frames.iter().copied();
        let settled = settle(
            || {
                taken.lock().unwrap().push(start.elapsed().as_millis());
                std::future::ready(Ok::<_, ToolError>(frames.next().unwrap()))
            },
            exact,
            limit,
        )
        .await
        .unwrap();
        (settled, taken.into_inner().unwrap(), start)
    }

    #[tokio::test(start_paused = true)]
    async fn the_first_repeated_image_is_the_result() {
        let (settled, taken, start) = run(&[1, 2, 3, 3, 9], Duration::from_secs(2)).await;
        assert_eq!(
            settled,
            Settled {
                last: 3,
                stable: true,
                change: Some(0.0),
                since: Some(start + Duration::from_millis(250)),
            }
        );
        // A first look after 50 ms, then one capture every 100 ms.
        assert_eq!(taken, [50, 150, 250, 350]);
    }

    #[tokio::test(start_paused = true)]
    async fn a_screen_that_keeps_changing_ends_at_the_limit_unsettled() {
        let frames: Vec<u8> = (0..100).collect();
        let (settled, taken, _) = run(&frames, Duration::from_millis(450)).await;
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
            exact,
            Duration::from_millis(450),
        )
        .await
        .unwrap();
        assert_eq!(start.elapsed(), Duration::from_millis(450));
        assert_eq!(result, unsettled(1, None));
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
            exact,
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
            exact,
            Duration::from_millis(450),
        )
        .await
        .unwrap();
        assert!(!result.stable);
    }

    #[tokio::test(start_paused = true)]
    async fn a_first_capture_completed_at_the_deadline_is_kept_unsettled() {
        let result = settle(
            || frame_after(Duration::from_millis(400)),
            exact,
            Duration::from_millis(450),
        )
        .await
        .unwrap();
        assert_eq!(result, unsettled(1, None));
    }

    #[tokio::test(start_paused = true)]
    async fn a_first_capture_completed_after_the_deadline_is_kept_unsettled() {
        let (started, running) = tokio::sync::oneshot::channel();
        let capture = async move {
            started.send(()).unwrap();
            frame_after(Duration::from_millis(400)).await
        };
        let mut capture = Some(capture);
        let waiting = tokio::spawn(async move {
            settle(
                || capture.take().unwrap(),
                exact,
                Duration::from_millis(450),
            )
            .await
        });
        running.await.unwrap();
        // Simulate the runtime resuming after both the capture and deadline timers fired.
        tokio::time::advance(Duration::from_millis(500)).await;
        assert_eq!(waiting.await.unwrap().unwrap(), unsettled(1, None));
    }

    #[tokio::test(start_paused = true)]
    async fn a_failed_capture_ends_the_wait_with_its_error() {
        let error = ToolError::new(ErrorName::UpstreamError, "grim died");
        let mut captures = [Ok(1), Err(error.clone())].into_iter();
        let failed = settle(
            || std::future::ready(captures.next().unwrap()),
            exact,
            Duration::from_secs(1),
        )
        .await;
        assert_eq!(failed, Err(error));
    }

    /// `settle` over samples that each changed `changes[i]` of the pixels from the one
    /// before, `None` for not comparable: whether it settled, with what change, and since
    /// how long after the start.
    async fn changing(
        changes: &[Option<f64>],
        limit: Duration,
    ) -> (bool, Option<f64>, Option<u128>) {
        let start = Instant::now();
        let mut samples = std::iter::once(None).chain(changes.iter().copied());
        let settled = settle(
            || std::future::ready(Ok::<_, ToolError>(samples.next().unwrap())),
            |_: &Option<f64>, next: &Option<f64>| *next,
            limit,
        )
        .await
        .unwrap();
        let since = settled.since.map(|since| (since - start).as_millis());
        (settled.stable, settled.change, since)
    }

    #[tokio::test(start_paused = true)]
    async fn two_small_changes_in_a_row_count_as_still_and_a_large_one_starts_over() {
        // Samples start at 50, 150, 250, 350 and 450 ms.
        let clock = [Some(0.3), Some(0.001), Some(0.002)];
        assert_eq!(
            changing(&clock, LIMIT).await,
            (true, Some(0.002), Some(150))
        );
        let popup = [Some(0.001), Some(0.3), Some(0.001), Some(0.0)];
        assert_eq!(changing(&popup, LIMIT).await, (true, Some(0.0), Some(250)));
        let unknown = [Some(0.001), None, Some(0.001), Some(QUIET)];
        assert_eq!(
            changing(&unknown, LIMIT).await,
            (true, Some(QUIET), Some(250))
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_large_change_that_never_stops_ends_at_the_limit_unsettled() {
        let video = [Some(0.2); 20];
        let start = Instant::now();
        let ended = changing(&video, LIMIT).await;
        assert_eq!(ended, (false, Some(0.2), None));
        assert_eq!(start.elapsed(), LIMIT);
        let just_over = [Some(QUIET + 0.0001); 20];
        assert!(!changing(&just_over, LIMIT).await.0);
    }

    fn pixels(width: u32, changed: &[usize]) -> Sample {
        let mut pixels = vec![0; usize::try_from(width).unwrap() * 3];
        for &pixel in changed {
            pixels[pixel * 3] = 255;
        }
        Sample::Pixels([format!("P6\n{width} 1\n255\n").as_bytes(), &pixels].concat())
    }

    fn shot(image: &[u8]) -> Screenshot {
        let rect = Rect {
            x: 0,
            y: 0,
            width: 100,
            height: 100,
        };
        let geometry = LogicalOutput {
            x: 0,
            y: 0,
            width: 100,
            height: 100,
            scale: 1.0,
            transform: Transform::Normal,
        };
        Screenshot {
            metadata: Metadata {
                output: "DP-1".to_owned(),
                transform: Transform::Normal,
                output_origin: (0, 0),
                captured: rect,
                scale: 1.0,
                width: 100,
                height: 100,
                mime_type: "image/jpeg",
                captured_at_unix_ms: 0,
                capture_ms: 0,
                screenshot_ref: None,
                settled: None,
                changing: None,
                steady_since: None,
                saved: None,
            },
            geometry,
            motion_geometry: None,
            image: image.to_vec(),
        }
    }

    fn image(bytes: &[u8]) -> Sample {
        Sample::Image(Box::new(shot(bytes)))
    }

    #[test]
    fn compares_pixels_by_count_and_jpegs_only_for_equality() {
        assert_eq!(compare(&pixels(200, &[]), &pixels(200, &[3])), Some(0.005));
        assert_eq!(
            compare(&pixels(200, &[1, 2]), &pixels(200, &[1, 2])),
            Some(0.0)
        );
        assert_eq!(
            compare(&pixels(200, &[]), &pixels(100, &[])),
            None,
            "resized"
        );
        assert_eq!(compare(&image(b"a"), &image(b"a")), Some(0.0));
        assert_eq!(compare(&image(b"a"), &image(b"b")), None);
        assert_eq!(compare(&pixels(200, &[]), &image(b"a")), None);
    }

    #[tokio::test(start_paused = true)]
    async fn the_result_is_one_jpeg_after_pixel_samples_and_the_last_jpeg_otherwise() {
        let since = Instant::now();
        let still = |last| Settled {
            last,
            stable: true,
            change: Some(0.001),
            since: Some(since),
        };
        let whole = || std::future::ready(Ok(shot(b"whole")));
        let after = finish(still(pixels(10, &[])), whole).await.unwrap();
        assert_eq!(after.image, b"whole");
        assert_eq!(
            (
                after.metadata.settled,
                after.metadata.changing,
                after.metadata.steady_since
            ),
            (Some(true), Some(0.001), Some(since))
        );
        let never = || {
            std::future::ready(Err::<Screenshot, _>(CallError::InvalidArguments(
                "no".to_owned(),
            )))
        };
        let last = finish(still(image(b"last")), never).await.unwrap();
        assert_eq!(last.image, b"last");
        let gone = finish(still(pixels(10, &[])), never).await;
        assert!(
            matches!(gone, Err(CallError::InvalidArguments(_))),
            "output disabled"
        );
    }
}
