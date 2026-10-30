//! Waiting for the screen to stop changing. An input tool's `sent` only means niri handled
//! the input; the app redraws after that, at its own pace. A screenshot taken right away
//! can show the screen from before, so a capture counts as the result only once the next
//! one, taken at least `GAP` later, is the same image.
//!
//! An action aimed at the window with keyboard focus (`Scope::Focused`) compares only that
//! window's rectangle where niri gives its place, so a bar's clock, a terminal's spinner or
//! a video elsewhere on the output doesn't keep the wait going. The result is still the
//! whole output, captured once more after the wait.

use std::time::Duration;

use niri_ipc::Window;
use tokio::time::Instant;

use crate::a11y::model::{self, Scene};
use crate::error::{CallError, ErrorName, ToolError};
use crate::input::keyboard::Expect;
use crate::observe::{Rect, Screenshot};

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
    let mut started = Instant::now();
    let mut last = tokio::time::timeout_at(deadline, Box::pin(capture()))
        .await
        .map_err(|_| E::from(no_sample()))??;
    let mut stable = false;
    loop {
        let expired = Instant::now() >= deadline;
        if stable || expired {
            return Ok(Settled {
                last,
                stable: stable && !expired,
            });
        }
        tokio::select! {
            biased;
            () = tokio::time::sleep_until(deadline) => continue,
            () = tokio::time::sleep_until(started + GAP) => {},
        }
        started = Instant::now();
        if let Ok(next) = tokio::time::timeout_at(deadline, Box::pin(capture())).await {
            let next = next?;
            stable = same(&last, &next);
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

/// What a screenshot that comes with an action waits on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Scope {
    /// The whole focused output.
    Output,
    /// The window with keyboard focus, sample by sample (`focused_region`), and the whole
    /// output while it has no place.
    Focused,
}

impl Scope {
    /// A keyboard tool's: the window `expect` checked has keyboard focus, so its own
    /// rectangle; with `expect: "none"`, where the keys go isn't known, so the whole output.
    pub(crate) const fn typed_into(expect: &Expect) -> Self {
        match expect {
            Expect::Window(_) | Expect::App(_) => Self::Focused,
            Expect::Unchecked => Self::Output,
        }
    }
}

/// One capture of a settle wait: the whole output, which the result returns as it is, or
/// the rectangle of the window with keyboard focus, in layout coordinates, and its image.
#[derive(Debug)]
pub(crate) enum Sample {
    Output(Box<Screenshot>),
    Window { rect: Rect, image: Vec<u8> },
}

/// Two samples show the same: the same rectangle, or the whole output, with the same image.
/// A sample of the window and one of the output never match, so a wait whose window moves,
/// closes or loses its place goes on until two samples of one kind match.
fn same(a: &Sample, b: &Sample) -> bool {
    match (a, b) {
        (Sample::Output(a), Sample::Output(b)) => a.image == b.image,
        (
            Sample::Window { rect, image },
            Sample::Window {
                rect: other,
                image: same,
            },
        ) => rect == other && image == same,
        (Sample::Output(_), Sample::Window { .. }) | (Sample::Window { .. }, Sample::Output(_)) => {
            false
        }
    }
}

/// Samples until two in a row match (`settle`), then returns the whole output: the last
/// sample when it is one, or else the capture `whole` takes, after the wait. Its metadata
/// says whether the samples had stopped changing.
pub(crate) async fn on<F, G>(
    sample: impl FnMut() -> F,
    whole: impl FnOnce() -> G,
    limit: Duration,
) -> Result<Screenshot, CallError>
where
    F: Future<Output = Result<Sample, CallError>>,
    G: Future<Output = Result<Screenshot, CallError>>,
{
    let settled = settle(sample, same, limit).await?;
    let mut shot = match settled.last {
        Sample::Output(shot) => *shot,
        Sample::Window { .. } => whole().await?,
    };
    shot.metadata.settled = Some(settled.stable);
    Ok(shot)
}

/// The rectangle a `Scope::Focused` sample captures, in layout coordinates: the tile of
/// `focused`, the window with keyboard focus, clipped to its output, when niri places it
/// (`model::origin`: floating, on its output's active workspace, with the overview closed
/// and no fullscreen window over it) on `output`, the output the screenshot is of. `None`
/// otherwise, for the whole output: niri 26.04 gives a tiled window no place. The tile
/// includes the window's border; a popup drawn past it, such as a menu, isn't in it.
pub(crate) fn focused_region(focused: &Window, scene: &Scene<'_>, output: &str) -> Option<Rect> {
    let origin = model::origin(focused, scene).ok()?;
    if origin.output != output {
        return None;
    }
    let tile = model::floating_tile(&origin.area, focused)?;
    let area = origin.area;
    let (area_x, area_y) = (f64::from(area.x), f64::from(area.y));
    let left = tile.x.floor().max(area_x);
    let top = tile.y.floor().max(area_y);
    let right = (tile.x + tile.width)
        .ceil()
        .min(area_x + f64::from(area.width));
    let bottom = (tile.y + tile.height)
        .ceil()
        .min(area_y + f64::from(area.height));
    if right <= left || bottom <= top {
        return None;
    }
    let rect = clipped(left, top, right - left, bottom - top);
    let whole = Rect {
        x: area.x,
        y: area.y,
        width: area.width,
        height: area.height,
    };
    (rect != whole).then_some(rect)
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "whole numbers clamped to an output's bounds, which niri gives as i32 and u32"
)]
const fn clipped(x: f64, y: f64, width: f64, height: f64) -> Rect {
    Rect {
        x: x as i32,
        y: y as i32,
        width: width as u32,
        height: height as u32,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    use niri_ipc::Transform;

    use super::*;
    use crate::observe::Metadata;

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
    async fn a_first_capture_completed_at_the_deadline_is_kept_unsettled() {
        let result = settle(
            || frame_after(Duration::from_millis(400)),
            |a, b| a == b,
            Duration::from_millis(450),
        )
        .await
        .unwrap();
        assert_eq!(
            result,
            Settled {
                last: 1,
                stable: false
            }
        );
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
                |a, b| a == b,
                Duration::from_millis(450),
            )
            .await
        });
        running.await.unwrap();
        // Simulate the runtime resuming after both the capture and deadline timers fired.
        tokio::time::advance(Duration::from_millis(500)).await;
        assert_eq!(
            waiting.await.unwrap().unwrap(),
            Settled {
                last: 1,
                stable: false
            }
        );
    }

    fn rect(x: i32) -> Rect {
        Rect {
            x,
            y: 0,
            width: 100,
            height: 100,
        }
    }

    fn whole_output(image: &[u8]) -> Screenshot {
        let metadata = Metadata {
            output: "DP-1".to_owned(),
            transform: Transform::Normal,
            output_origin: (0, 0),
            captured: rect(0),
            scale: 1.0,
            width: 100,
            height: 100,
            mime_type: "image/jpeg",
            captured_at_unix_ms: 0,
            capture_ms: 0,
            screenshot_ref: None,
            settled: None,
            saved: None,
        };
        Screenshot {
            metadata,
            geometry: niri_ipc::LogicalOutput {
                x: 0,
                y: 0,
                width: 100,
                height: 100,
                scale: 1.0,
                transform: Transform::Normal,
            },
            motion_geometry: None,
            image: image.to_vec(),
        }
    }

    fn window(x: i32, image: &str) -> Sample {
        Sample::Window {
            rect: rect(x),
            image: image.as_bytes().to_vec(),
        }
    }

    fn output(image: &str) -> Sample {
        Sample::Output(Box::new(whole_output(image.as_bytes())))
    }

    /// `on` over `samples` in order, one every 100 ms, with whole captures of `whole`: the
    /// image it returns, whether it settled, and how many whole captures it took.
    async fn sampled(samples: Vec<Sample>, limit: Duration) -> (Vec<u8>, Option<bool>, usize) {
        let mut samples = samples.into_iter();
        let wholes = Mutex::new(0);
        let shot = on(
            || std::future::ready(Ok(samples.next().unwrap())),
            || {
                *wholes.lock().unwrap() += 1;
                std::future::ready(Ok(whole_output(b"whole")))
            },
            limit,
        )
        .await
        .unwrap();
        (
            shot.image,
            shot.metadata.settled,
            wholes.into_inner().unwrap(),
        )
    }

    #[tokio::test(start_paused = true)]
    async fn a_window_that_stops_changing_settles_and_returns_the_whole_output_after() {
        let typed = vec![
            window(0, "a"),
            window(0, "ab"),
            window(0, "abc"),
            window(0, "abc"),
        ];
        let start = Instant::now();
        let (image, settled, wholes) = sampled(typed, LIMIT).await;
        assert_eq!(
            (image.as_slice(), settled, wholes),
            (&b"whole"[..], Some(true), 1)
        );
        // The window kept changing for three samples: the wait went on until it stopped.
        assert_eq!(start.elapsed(), Duration::from_millis(350));
    }

    #[tokio::test(start_paused = true)]
    async fn a_window_that_never_stops_ends_at_the_limit_unsettled() {
        let video: Vec<Sample> = (0..100)
            .map(|frame| window(0, &frame.to_string()))
            .collect();
        let start = Instant::now();
        let (image, settled, wholes) = sampled(video, Duration::from_millis(450)).await;
        assert_eq!(
            (image.as_slice(), settled, wholes),
            (&b"whole"[..], Some(false), 1)
        );
        assert_eq!(start.elapsed(), Duration::from_millis(450));
    }

    #[tokio::test(start_paused = true)]
    async fn a_window_that_moves_or_loses_its_place_waits_for_two_samples_of_one_kind() {
        let moved = vec![window(0, "a"), window(40, "a"), window(40, "a")];
        assert_eq!(sampled(moved, LIMIT).await.1, Some(true));
        // Closed, or gone to another workspace: the whole output from then on, which the
        // result is, as it is, with no capture after.
        let gone = vec![window(0, "a"), output("a"), output("a")];
        let (image, settled, wholes) = sampled(gone, LIMIT).await;
        assert_eq!(
            (image.as_slice(), settled, wholes),
            (&b"a"[..], Some(true), 0)
        );
        let back = vec![output("a"), window(0, "a"), output("a"), output("a")];
        assert_eq!(sampled(back, LIMIT).await.1, Some(true));
    }

    #[tokio::test(start_paused = true)]
    async fn a_failed_whole_capture_after_the_window_settled_is_the_error() {
        let failed = on(
            || std::future::ready(Ok(window(0, "a"))),
            || {
                let gone = CallError::InvalidArguments("niri reports no focused output".to_owned());
                std::future::ready(Err::<Screenshot, _>(gone))
            },
            LIMIT,
        )
        .await;
        assert!(matches!(failed, Err(CallError::InvalidArguments(_))));
    }

    fn placed(id: u64, workspace: u64, tile: Option<(f64, f64)>, focused: bool) -> Window {
        serde_json::from_value(serde_json::json!({
            "id": id, "title": null, "app_id": "a", "pid": 1, "workspace_id": workspace,
            "is_focused": focused, "is_floating": tile.is_some(), "is_urgent": false,
            "layout": {
                "pos_in_scrolling_layout": if tile.is_some() { serde_json::Value::Null } else { serde_json::json!([1, 1]) },
                "tile_size": [400.5, 300.0], "window_size": [400, 300],
                "tile_pos_in_workspace_view": tile, "window_offset_in_tile": [0.0, 0.0]
            }
        }))
        .unwrap()
    }

    fn shown(
        outputs: &[(&str, i32, u64)],
    ) -> (Vec<niri_ipc::Workspace>, BTreeMap<String, niri_ipc::Output>) {
        let workspaces = outputs
            .iter()
            .map(|(name, _, id)| {
                serde_json::from_value(serde_json::json!({
                    "id": id, "idx": 1, "name": null, "output": name, "is_urgent": false,
                    "is_active": true, "is_focused": false, "active_window_id": null
                }))
                .unwrap()
            })
            .collect();
        let outputs = outputs
            .iter()
            .map(|(name, x, _)| {
                let output = serde_json::from_value(serde_json::json!({
                    "name": name, "make": "", "model": "", "serial": null,
                    "physical_size": null, "modes": [], "current_mode": null,
                    "is_custom_mode": false, "vrr_supported": false, "vrr_enabled": false,
                    "logical": {"x": x, "y": 0, "width": 1280, "height": 720, "scale": 1.0, "transform": "Normal"}
                }))
                .unwrap();
                ((*name).to_owned(), output)
            })
            .collect();
        (workspaces, outputs)
    }

    #[test]
    fn the_focused_region_is_a_placed_floating_tile_on_the_screenshots_output() {
        let (workspaces, outputs) = shown(&[("DP-1", 0, 1), ("HDMI-A-1", 1280, 2)]);
        let scene = |overview_open| Scene {
            workspaces: &workspaces,
            outputs: &outputs,
            windows: &[],
            overview_open,
        };
        let region = |window: &Window, output| focused_region(window, &scene(false), output);
        let floating = placed(1, 1, Some((20.25, 30.0)), true);
        assert_eq!(
            region(&floating, "DP-1"),
            Some(Rect {
                x: 20,
                y: 30,
                width: 401,
                height: 300
            }),
            "rounded outwards"
        );
        assert_eq!(
            region(&floating, "HDMI-A-1"),
            None,
            "another output's screenshot"
        );
        assert_eq!(
            focused_region(&floating, &scene(true), "DP-1"),
            None,
            "overview"
        );
        assert_eq!(region(&placed(1, 1, None, true), "DP-1"), None, "tiled");
        let on_the_right = placed(2, 2, Some((1000.0, 600.0)), true);
        assert_eq!(
            region(&on_the_right, "HDMI-A-1"),
            Some(Rect {
                x: 2280,
                y: 600,
                width: 280,
                height: 120
            }),
            "clipped to its output"
        );
        assert_eq!(
            region(&placed(3, 9, Some((0.0, 0.0)), true), "DP-1"),
            None,
            "hidden workspace"
        );
        let mut filling = placed(1, 1, Some((0.0, 0.0)), true);
        filling.layout.tile_size = (1280.0, 720.0);
        assert_eq!(region(&filling, "DP-1"), None, "the whole output");
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
