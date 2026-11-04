//! The part of the screen the Noctalia plugin's robot may change while it thinks in place
//! between actions, which a settle wait leaves out of its comparisons. The plugin's layer
//! publishes it in the instance's runtime directory as `overlay.json`, replaced whole:
//!
//! ```json
//! {"output": "DP-1", "rect": {"x": 900, "y": 400, "width": 120, "height": 70},
//!  "incarnation": "DP-1 1762250000000", "ts": 1762250001000}
//! ```
//!
//! `rect` is in the output's local logical pixels, as `live.json`'s aims are, and `ts` is
//! when the layer last wrote it, in Unix milliseconds; `incarnation` names the writer for
//! people reading the file. The area counts only while `ts` is at most `FRESH_MS` old and
//! at most `SKEW_MS` ahead of the server's clock, and while the rectangle is no larger
//! than the plugin's largest robot at Noctalia's largest UI scale. Anything else, or no
//! file, compares every pixel, as without the plugin.
//!
//! Only `load` does I/O, on a thread of its own that `Reader` waits for at most `WAIT`,
//! so a runtime directory on a filesystem that stalls can't stop the server. Parsing,
//! mapping and the comparison (`ppm::changed_outside`) are pure.

use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};

use niri_ipc::Transform;
use rustix::fs::{FileType, Mode, OFlags};
use serde::Deserialize;

use crate::coords::{ImagePx, ImageRect, OutputLocalRect};
use crate::observe::{self, Metadata, Rect};

/// The area's file in the runtime directory.
pub(crate) const FILE: &str = "overlay.json";

/// The longest file read; a longer one counts as none.
const LIMIT: u64 = 1024;
/// The plugin writes the file every 2 s while the robot thinks, so a file older than
/// two and a half of those is left over from a layer that stopped.
const FRESH_MS: u64 = 5000;
/// How far ahead of the server's clock a `ts` may be, for the time the write takes to be
/// read. A later one would keep counting long after the layer stopped.
const SKEW_MS: u64 = 1000;
/// The largest rectangle the plugin publishes, in logical pixels, rounded up: the 72-pixel
/// robot, its 5-pixel room to hop, the gap and its 61-pixel thought bubble, 143 x 77 units
/// at Noctalia's largest UI scale, 2.5, with 2 pixels around. A larger one isn't the
/// robot. `noctalia/niri-computer-use/tests/fixtures/overlay-areas.jsonl` holds the
/// plugin's rectangles for every size, UI scale and side, which the plugin's tests
/// generate, and a test here requires each to fit and the largest to round up to these.
pub(crate) const MAX_WIDTH: f64 = 362.0;
pub(crate) const MAX_HEIGHT: f64 = 197.0;

/// The robot's area on one output.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Area {
    pub(crate) output: String,
    pub(crate) rect: OutputLocalRect,
}

#[derive(Deserialize)]
struct Published {
    output: String,
    rect: Bounds,
    ts: u64,
}

#[derive(Deserialize)]
struct Bounds {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

/// How long a settle sample waits for the area: a read from a tmpfs takes microseconds.
const WAIT: Duration = Duration::from_millis(50);

/// What reads the file: `load`, or a stand-in in tests.
type Load = Arc<dyn Fn(&Path) -> Option<Vec<u8>> + Send + Sync>;

/// Reads the area for settle samples, each read on a thread of its own, at most one at a
/// time. A read still running when another sample asks, or one that takes longer than
/// `WAIT`, gives no area, and the sample is compared whole; the thread ends when the read
/// does.
pub(crate) struct Reader {
    path: PathBuf,
    busy: Arc<AtomicBool>,
    load: Load,
}

impl std::fmt::Debug for Reader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Reader")
            .field("path", &self.path)
            .field("busy", &self.busy)
            .finish_non_exhaustive()
    }
}

impl Reader {
    pub(crate) fn new(path: PathBuf) -> Self {
        Self::with(path, Arc::new(load))
    }

    fn with(path: PathBuf, load: Load) -> Self {
        Self {
            path,
            busy: Arc::default(),
            load,
        }
    }

    /// The area published now, if one could be read within `WAIT`.
    pub(crate) async fn read(&self) -> Option<Area> {
        if self.busy.swap(true, Ordering::AcqRel) {
            return None;
        }
        let (sent, received) = tokio::sync::oneshot::channel();
        let (path, busy, load) = (
            self.path.clone(),
            Arc::clone(&self.busy),
            Arc::clone(&self.load),
        );
        let spawned = std::thread::Builder::new()
            .name("overlay-area".to_owned())
            .spawn(move || {
                let bytes = load(&path);
                busy.store(false, Ordering::Release);
                // The sample may have stopped waiting.
                drop(sent.send(bytes));
            });
        if spawned.is_err() {
            self.busy.store(false, Ordering::Release);
            return None;
        }
        let bytes = tokio::time::timeout(WAIT, received).await.ok()?.ok()??;
        parse(&bytes, observe::unix_ms(SystemTime::now()))
    }
}

/// The file at `path`, opened without following a symlink and without waiting for a
/// writer, so a FIFO or a link put there returns at once, and read only when it is a
/// regular file of at most `LIMIT` bytes.
fn load(path: &Path) -> Option<Vec<u8>> {
    let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
    let fd = rustix::fs::open(path, flags, Mode::empty()).ok()?;
    let stat = rustix::fs::fstat(&fd).ok()?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
        return None;
    }
    let mut bytes = Vec::new();
    std::fs::File::from(fd)
        .take(LIMIT + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    (u64::try_from(bytes.len()).ok()? <= LIMIT).then_some(bytes)
}

/// The area `bytes` publish, as at `now_ms`, or `None` for a file that doesn't count.
pub(crate) fn parse(bytes: &[u8], now_ms: u64) -> Option<Area> {
    let published: Published = serde_json::from_slice(bytes).ok()?;
    let fresh = now_ms.checked_sub(published.ts).map_or_else(
        || {
            published
                .ts
                .checked_sub(now_ms)
                .is_some_and(|ahead| ahead <= SKEW_MS)
        },
        |age| age <= FRESH_MS,
    );
    let Bounds {
        x,
        y,
        width,
        height,
    } = published.rect;
    let sized = [x, y, width, height].iter().all(|value| value.is_finite())
        && width > 0.0
        && height > 0.0
        && width <= MAX_WIDTH
        && height <= MAX_HEIGHT;
    let rect = OutputLocalRect {
        x,
        y,
        width,
        height,
    };
    (fresh && sized && !published.output.is_empty()).then_some(Area {
        output: published.output,
        rect,
    })
}

/// Where a sample was captured: two samples whose geometry differs show different parts
/// of the layout in the same pixels, so no one mask fits both.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Geometry {
    pub(crate) output: String,
    pub(crate) transform: Transform,
    pub(crate) output_origin: (i32, i32),
    pub(crate) captured: Rect,
    pub(crate) scale: f64,
}

impl From<&Metadata> for Geometry {
    fn from(metadata: &Metadata) -> Self {
        Self {
            output: metadata.output.clone(),
            transform: metadata.transform,
            output_origin: metadata.output_origin,
            captured: metadata.captured,
            scale: metadata.scale,
        }
    }
}

/// The pixels of a `width` x `height` image captured at `at` that `area` covers, rounded
/// outward and clipped to the image; `None` for another output's area or one wholly
/// outside the image. An edge maps as
/// `(output_origin + local edge - captured origin) x scale`: the captured rectangle may be
/// a region of the output, and `scale` is the image's pixels per logical pixel, below 1
/// when `max_width` shrank it.
pub(crate) fn mask(area: &Area, at: &Geometry, width: u32, height: u32) -> Option<ImageRect> {
    if area.output != at.output {
        return None;
    }
    let edge = |origin: i32, local: f64, captured: i32| {
        (f64::from(origin) + local - f64::from(captured)) * at.scale
    };
    let (ox, oy) = at.output_origin;
    let (cx, cy) = (at.captured.x, at.captured.y);
    let rect = area.rect;
    let (left, top) = (
        clip(edge(ox, rect.x, cx).floor(), width),
        clip(edge(oy, rect.y, cy).floor(), height),
    );
    let (right, bottom) = (
        clip(edge(ox, rect.x + rect.width, cx).ceil(), width),
        clip(edge(oy, rect.y + rect.height, cy).ceil(), height),
    );
    (left < right && top < bottom).then_some(ImageRect {
        origin: ImagePx { x: left, y: top },
        width: right - left,
        height: bottom - top,
    })
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "clamped to 0..=max first, a whole number of pixels"
)]
fn clip(edge: f64, max: u32) -> u32 {
    edge.clamp(0.0, f64::from(max)) as u32
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;
    use std::time::{Duration, Instant};

    use super::*;

    const NOW: u64 = 1_762_250_000_000;

    fn file(output: &str, rect: [f64; 4], ts: u64) -> String {
        let [x, y, width, height] = rect;
        serde_json::json!({
            "output": output,
            "rect": {"x": x, "y": y, "width": width, "height": height},
            "incarnation": "DP-1 1",
            "ts": ts,
        })
        .to_string()
    }

    fn area(rect: [f64; 4]) -> Area {
        parse(file("DP-1", rect, NOW).as_bytes(), NOW).unwrap()
    }

    // The bug: a file that doesn't describe a live robot hides real changes from a settle.
    #[test]
    fn only_a_fresh_sane_rectangle_counts() {
        let rect = [10.0, 20.0, 120.0, 70.0];
        assert_eq!(
            parse(file("DP-1", rect, NOW - FRESH_MS).as_bytes(), NOW),
            Some(Area {
                output: "DP-1".to_owned(),
                rect: OutputLocalRect {
                    x: 10.0,
                    y: 20.0,
                    width: 120.0,
                    height: 70.0
                },
            })
        );
        assert!(parse(file("DP-1", rect, NOW + SKEW_MS).as_bytes(), NOW).is_some());
        let refused = [
            ("stale", file("DP-1", rect, NOW - FRESH_MS - 1)),
            ("ahead", file("DP-1", rect, NOW + SKEW_MS + 1)),
            // A clock set back a day leaves the last write in the future.
            ("clock set back", file("DP-1", rect, NOW + 86_400_000)),
            ("far future", file("DP-1", rect, u64::MAX)),
            ("no output", file("", rect, NOW)),
            ("empty", file("DP-1", [0.0, 0.0, 0.0, 70.0], NOW)),
            ("negative", file("DP-1", [0.0, 0.0, 120.0, -1.0], NOW)),
            ("wide", file("DP-1", [0.0, 0.0, MAX_WIDTH + 0.5, 70.0], NOW)),
            (
                "tall",
                file("DP-1", [0.0, 0.0, 120.0, MAX_HEIGHT + 0.5], NOW),
            ),
            (
                "negative ts",
                file("DP-1", rect, NOW).replace(&NOW.to_string(), "-1"),
            ),
            (
                "fractional ts",
                file("DP-1", rect, NOW).replace(&NOW.to_string(), "1.5"),
            ),
            (
                "infinite",
                r#"{"output":"DP-1","rect":{"x":1e999,"y":0,"width":1,"height":1},"ts":1}"#
                    .to_owned(),
            ),
            ("no rect", format!(r#"{{"output":"DP-1","ts":{NOW}}}"#)),
            ("not JSON", "{".to_owned()),
        ];
        for (name, bytes) in refused {
            assert_eq!(parse(bytes.as_bytes(), NOW), None, "{name}");
        }
    }

    fn geometry(origin: (i32, i32), captured: [i32; 2], size: (u32, u32), scale: f64) -> Geometry {
        Geometry {
            output: "DP-1".to_owned(),
            transform: Transform::Normal,
            output_origin: origin,
            captured: Rect {
                x: captured[0],
                y: captured[1],
                width: size.0,
                height: size.1,
            },
            scale,
        }
    }

    fn masked(area: &Area, at: &Geometry, image: (u32, u32)) -> Option<[u32; 4]> {
        mask(area, at, image.0, image.1).map(|m| {
            let ImagePx { x, y } = m.origin;
            [x, y, x + m.width, y + m.height]
        })
    }

    // The bug: mapping in the wrong space masks pixels the robot never touches and
    // compares the ones it does.
    #[test]
    fn maps_the_rectangle_into_the_captured_image_rounded_outward() {
        let robot = area([200.5, 150.0, 40.2, 30.0]);
        // A whole output at the layout's origin, at its own scale and shrunk by max_width.
        let whole = geometry((0, 0), [0, 0], (2560, 1440), 1.0);
        assert_eq!(
            masked(&robot, &whole, (2560, 1440)),
            Some([200, 150, 241, 180])
        );
        let shrunk = Geometry {
            scale: 0.5,
            ..whole.clone()
        };
        assert_eq!(
            masked(&robot, &shrunk, (1280, 720)),
            Some([100, 75, 121, 90])
        );
        let scaled = Geometry {
            scale: 1.25,
            ..whole
        };
        assert_eq!(
            masked(&robot, &scaled, (3200, 1800)),
            Some([250, 187, 301, 225])
        );
        // A region of an output left of and above the layout's origin.
        let whole_pixels = area([200.0, 150.0, 40.0, 30.0]);
        let region = geometry((-2048, -200), [-1900, -100], (400, 300), 0.5);
        assert_eq!(
            masked(&whole_pixels, &region, (200, 150)),
            Some([26, 25, 46, 40])
        );
        // A whole rotated output maps the same: its logical size is already turned.
        let rotated = Geometry {
            transform: Transform::_90,
            ..geometry((-1440, 0), [-1440, 0], (1440, 2560), 1.0)
        };
        assert_eq!(
            masked(&whole_pixels, &rotated, (1440, 2560)),
            Some([200, 150, 240, 180])
        );
    }

    #[test]
    fn clips_to_the_image_and_names_nothing_outside_it_or_on_another_output() {
        let at = geometry((0, 0), [100, 100], (200, 100), 1.0);
        assert_eq!(
            masked(&area([90.0, 150.0, 40.0, 30.0]), &at, (200, 100)),
            Some([0, 50, 30, 80])
        );
        assert_eq!(
            masked(&area([280.0, 180.0, 40.0, 30.0]), &at, (200, 100)),
            Some([180, 80, 200, 100])
        );
        assert_eq!(
            masked(&area([0.0, 0.0, 50.0, 50.0]), &at, (200, 100)),
            None,
            "above-left"
        );
        assert_eq!(
            masked(&area([300.0, 100.0, 40.0, 30.0]), &at, (200, 100)),
            None,
            "right of it"
        );
        let other = Geometry {
            output: "HDMI-A-1".to_owned(),
            ..at
        };
        assert_eq!(
            masked(&area([150.0, 150.0, 40.0, 30.0]), &other, (200, 100)),
            None
        );
    }

    // The bug: an area the plugin publishes that the server refuses as too large, so a
    // size or UI scale the plugin offers thinks with every pixel compared; or a cap far
    // above the plugin's largest area, which lets a stray file hide more than the robot.
    #[test]
    fn takes_every_area_the_plugin_publishes_and_no_larger_cap() {
        const SHARED: &str =
            include_str!("../noctalia/niri-computer-use/tests/fixtures/overlay-areas.jsonl");
        let (mut widest, mut tallest, mut areas) = (0.0_f64, 0.0_f64, 0);
        for line in SHARED.lines() {
            let entry: serde_json::Value = serde_json::from_str(line).unwrap();
            if let Some(max) = entry.get("max") {
                assert_eq!(
                    (max["width"].as_f64(), max["height"].as_f64()),
                    (Some(MAX_WIDTH), Some(MAX_HEIGHT))
                );
                continue;
            }
            let rect = &entry["rect"];
            let published = serde_json::json!({"output": "DP-1", "rect": rect, "incarnation": "DP-1 1", "ts": NOW});
            assert!(
                parse(published.to_string().as_bytes(), NOW).is_some(),
                "{line}"
            );
            widest = widest.max(rect["width"].as_f64().unwrap());
            tallest = tallest.max(rect["height"].as_f64().unwrap());
            areas += 1;
        }
        assert!(areas > 200, "{areas} areas");
        assert_eq!((widest.ceil(), tallest.ceil()), (MAX_WIDTH, MAX_HEIGHT));
    }

    // The bug: a read that stalls, as on a FUSE filesystem, stops the server's one thread,
    // and with it every deadline and every other call.
    #[tokio::test]
    async fn a_stalled_read_gives_no_area_at_once_and_starts_no_second_read() {
        let now = observe::unix_ms(SystemTime::now());
        let good = file("DP-1", [10.0, 20.0, 120.0, 70.0], now).into_bytes();
        let (release, stalled) = std::sync::mpsc::channel::<()>();
        let stalled = std::sync::Mutex::new(Some(stalled));
        let reads = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = Arc::clone(&reads);
        let reader = Reader::with(
            PathBuf::from("/stalls"),
            Arc::new(move |_: &Path| {
                // The first read waits until the test lets it go.
                let first = (counted.fetch_add(1, Ordering::SeqCst) == 0)
                    .then(|| stalled.lock().unwrap().take())
                    .flatten();
                first.map(|first| first.recv());
                Some(good.clone())
            }),
        );
        let start = Instant::now();
        assert_eq!(reader.read().await, None, "stalled");
        assert!(start.elapsed() < Duration::from_secs(1));
        assert_eq!(reader.read().await, None, "still stalled");
        assert_eq!(reads.load(Ordering::SeqCst), 1, "one read at a time");
        release.send(()).unwrap();
        let mut read = None;
        let mut tries = 0;
        while read.is_none() && tries < 100 {
            tokio::time::sleep(Duration::from_millis(10)).await;
            read = reader.read().await;
            tries += 1;
        }
        assert!(read.is_some(), "reads again once the stalled read ended");
    }

    // The bug: a FIFO or a link to one in the runtime directory hangs every settle.
    #[test]
    fn reads_only_a_small_regular_file_and_returns_at_once_otherwise() {
        let dir = crate::test_support::fresh_dir("overlay-area-read");
        let path = dir.join(FILE);
        let good = file("DP-1", [10.0, 20.0, 120.0, 70.0], NOW);
        std::fs::write(&path, &good).unwrap();
        assert!(load(&path).and_then(|bytes| parse(&bytes, NOW)).is_some());
        let link = dir.join("link");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert_eq!(load(&link), None, "a symlink to a good file");
        std::fs::write(&path, format!("{good}{}", " ".repeat(1024))).unwrap();
        assert_eq!(load(&path), None, "oversized");
        std::fs::write(&path, &good).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
        // Root reads it anyway; everyone else can't.
        if std::fs::read(&path).is_err() {
            assert_eq!(load(&path), None, "unreadable");
        }
        std::fs::remove_file(&path).unwrap();
        assert_eq!(load(&path), None, "missing");
        let fifo = dir.join("fifo");
        rustix::fs::mknodat(
            rustix::fs::CWD,
            &fifo,
            FileType::Fifo,
            Mode::RUSR | Mode::WUSR,
            0,
        )
        .unwrap();
        let fifo_link = dir.join("fifo-link");
        std::os::unix::fs::symlink(&fifo, &fifo_link).unwrap();
        std::fs::create_dir(dir.join("dir")).unwrap();
        let start = Instant::now();
        for special in [fifo, fifo_link, dir.join("dir")] {
            assert_eq!(load(&special), None, "{}", special.display());
        }
        assert!(start.elapsed() < Duration::from_secs(1));
    }
}
