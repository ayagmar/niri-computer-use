//! The coordinate contract (plan §8): an image pixel of a screenshot → a layout point →
//! a point on the output → the output's untransformed space → the arguments of a virtual
//! pointer's `motion_absolute`. Each space has its own type, so they can't be mixed.
//!
//! The mapping inverts niri's own (`compute_absolute_location`, `src/input/mod.rs` at
//! v26.04), and every encoded request is pushed back through a port of that forward
//! formula: a request that wouldn't land on its target is never sent.

use niri_ipc::{LogicalOutput, Output, Transform};

/// niri's pointer space comes from Smithay's `Space::output_geometry` (ff5fa7df):
/// transformed physical mode / fractional scale, ceiled, not IPC's truncated size.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "validated positive sizes bounded by u32::MAX / K, which also fits Smithay's i32 space"
)]
pub(crate) fn motion_geometry(output: &Output) -> Option<LogicalOutput> {
    let mut geometry = output.logical?;
    let mode = output.modes.get(output.current_mode?)?;
    if !geometry.scale.is_finite() || geometry.scale <= 0.0 {
        return None;
    }
    let (w, h) = match geometry.transform {
        Transform::_90 | Transform::_270 | Transform::Flipped90 | Transform::Flipped270 => {
            (mode.height, mode.width)
        }
        Transform::Normal | Transform::_180 | Transform::Flipped | Transform::Flipped180 => {
            (mode.width, mode.height)
        }
    };
    let width = (f64::from(w) / geometry.scale).ceil();
    let height = (f64::from(h) / geometry.scale).ceil();
    let max = f64::from(u32::MAX) / K;
    if width < 1.0 || height < 1.0 || width > max || height > max {
        return None;
    }
    geometry.width = width as u32;
    geometry.height = height as u32;
    Some(geometry)
}

/// `motion_absolute` resolution: an extent is the untransformed logical size times `K`.
const K: f64 = 1000.0;
/// How far niri's forward mapping may land from the target, in logical pixels.
const TOLERANCE: f64 = 0.002;

/// A pixel of a captured image, counted from its top-left corner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ImagePx {
    pub(crate) x: u32,
    pub(crate) y: u32,
}

/// A point in niri's layout space, in logical pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct LayoutPt {
    pub(crate) x: f64,
    pub(crate) y: f64,
}

/// A point relative to the output's top-left corner as displayed, in logical pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct OutputLocalPt {
    pub(crate) x: f64,
    pub(crate) y: f64,
}

/// A point in the output's untransformed logical space, which `motion_absolute` addresses.
#[derive(Debug, Clone, Copy, PartialEq)]
struct UntransformedPt {
    x: f64,
    y: f64,
}

/// The arguments of one `zwlr_virtual_pointer_v1.motion_absolute` request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ProtocolPt {
    pub(crate) x: u32,
    pub(crate) y: u32,
    pub(crate) x_extent: u32,
    pub(crate) y_extent: u32,
}

/// Where and at what scale an image was captured: the captured rectangle's layout origin
/// and the image pixels per logical pixel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Capture {
    pub(crate) origin: LayoutPt,
    pub(crate) scale: f64,
}

/// Pixel-centre convention: pixel `(px, py)` targets `(px + 0.5, py + 0.5)` in the image.
pub(crate) fn image_to_layout(pixel: ImagePx, capture: Capture) -> LayoutPt {
    LayoutPt {
        x: capture.origin.x + (f64::from(pixel.x) + 0.5) / capture.scale,
        y: capture.origin.y + (f64::from(pixel.y) + 0.5) / capture.scale,
    }
}

fn layout_to_output(point: LayoutPt, output: &LogicalOutput) -> OutputLocalPt {
    OutputLocalPt {
        x: point.x - f64::from(output.x),
        y: point.y - f64::from(output.y),
    }
}

/// The output's size before its transform, what niri's `compute_absolute_location` calls
/// `size`: a quarter turn swaps width and height.
const fn untransformed_size(output: &LogicalOutput) -> (u32, u32) {
    match output.transform {
        Transform::_90 | Transform::_270 | Transform::Flipped90 | Transform::Flipped270 => {
            (output.height, output.width)
        }
        Transform::Normal | Transform::_180 | Transform::Flipped | Transform::Flipped180 => {
            (output.width, output.height)
        }
    }
}

/// Inverts niri's `transform.transform_point_in(point, size)`, where `size` is the
/// untransformed size. Smithay's `Transform::invert` is not that inverse: every flipped
/// transform undoes itself, but `invert` swaps `Flipped90` and `Flipped270`.
fn output_to_untransformed(point: OutputLocalPt, output: &LogicalOutput) -> UntransformedPt {
    let (w, h) = untransformed_size(output);
    let (w, h) = (f64::from(w), f64::from(h));
    let OutputLocalPt { x, y } = point;
    let (x, y) = match output.transform {
        Transform::Normal => (x, y),
        Transform::_90 => (y, h - x),
        Transform::_180 => (w - x, h - y),
        Transform::_270 => (w - y, x),
        Transform::Flipped => (w - x, y),
        Transform::Flipped90 => (y, x),
        Transform::Flipped180 => (x, h - y),
        Transform::Flipped270 => (w - y, h - x),
    };
    UntransformedPt { x, y }
}

/// `x = round(ux × K)`, clamped to `[0, extent − 1]`, with `extent = width × K`.
fn encode(target: LayoutPt, output: &LogicalOutput) -> ProtocolPt {
    let point = output_to_untransformed(layout_to_output(target, output), output);
    let (w, h) = untransformed_size(output);
    let (x, x_extent) = encode_axis(point.x, w);
    let (y, y_extent) = encode_axis(point.y, h);
    ProtocolPt {
        x,
        y,
        x_extent,
        y_extent,
    }
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "clamped to [0, extent − 1] first, and an extent of a u32 size times 1000 fits"
)]
fn encode_axis(value: f64, size: u32) -> (u32, u32) {
    let extent = f64::from(size) * K;
    let encoded = (value * K).round().clamp(0.0, extent - 1.0);
    (encoded as u32, extent as u32)
}

/// niri's mapping of an absolute motion on a pointer bound to `output`: the virtual
/// pointer's `x_transformed` (`src/protocols/virtual_pointer.rs`), then
/// `compute_absolute_location` (`src/input/mod.rs`), at v26.04. The casts are niri's.
#[expect(
    clippy::cast_precision_loss,
    reason = "niri's own expression, `(x * width) as i64 as f64 / x_extent as f64`"
)]
fn niri_forward(request: ProtocolPt, output: &LogicalOutput) -> LayoutPt {
    let (w, h) = untransformed_size(output);
    let x = (i64::from(request.x) * i64::from(w)) as f64 / f64::from(request.x_extent);
    let y = (i64::from(request.y) * i64::from(h)) as f64 / f64::from(request.y_extent);
    let (x, y) = transform_point_in(output.transform, (x, y), (f64::from(w), f64::from(h)));
    LayoutPt {
        x: x + f64::from(output.x),
        y: y + f64::from(output.y),
    }
}

/// Smithay's `Transform::transform_point_in`, copied from Smithay `ff5fa7df`
/// (`src/utils/geometry.rs`), the revision niri 26.04 pins.
fn transform_point_in(transform: Transform, (x, y): (f64, f64), (w, h): (f64, f64)) -> (f64, f64) {
    match transform {
        Transform::Normal => (x, y),
        Transform::_90 => (h - y, x),
        Transform::_180 => (w - x, h - y),
        Transform::_270 => (y, w - x),
        Transform::Flipped => (w - x, y),
        Transform::Flipped90 => (y, x),
        Transform::Flipped180 => (x, h - y),
        Transform::Flipped270 => (h - y, w - x),
    }
}

/// Encodes `target` for a pointer bound to `output`, or returns how far from it niri would
/// put the pointer when that is more than the tolerance, as for a point off the output.
pub(crate) fn checked_encode(target: LayoutPt, output: &LogicalOutput) -> Result<ProtocolPt, f64> {
    let request = encode(target, output);
    let landed = niri_forward(request, output);
    let error = (landed.x - target.x).abs().max((landed.y - target.y).abs());
    if error <= TOLERANCE {
        Ok(request)
    } else {
        Err(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Transform; 8] = [
        Transform::Normal,
        Transform::_90,
        Transform::_180,
        Transform::_270,
        Transform::Flipped,
        Transform::Flipped90,
        Transform::Flipped180,
        Transform::Flipped270,
    ];

    fn output(x: i32, y: i32, (width, height): (u32, u32), transform: Transform) -> LogicalOutput {
        LogicalOutput {
            x,
            y,
            width,
            height,
            scale: 1.0,
            transform,
        }
    }

    /// Displayed logical sizes for 2560x1440 at scale 1, 1.25 and 1.5, the nested
    /// 960x720 window at 1 and 1.5, and an odd size.
    const SIZES: [(u32, u32); 6] = [
        (2560, 1440),
        (2048, 1152),
        (1707, 960),
        (960, 720),
        (640, 480),
        (1001, 333),
    ];

    fn assert_lands(target: LayoutPt, output: &LogicalOutput) {
        let encoded = checked_encode(target, output);
        assert!(
            encoded.is_ok(),
            "{target:?} on {output:?} lands {encoded:?} away"
        );
    }

    /// Every transform, at each size, at the origin, right of another output, and left of
    /// and above the origin.
    fn outputs() -> impl Iterator<Item = LogicalOutput> {
        ALL.into_iter().flat_map(|transform| {
            SIZES.into_iter().flat_map(move |size| {
                [(0, 0), (2560, 0), (-1920, -200)]
                    .into_iter()
                    .map(move |(x, y)| output(x, y, size, transform))
            })
        })
    }

    #[test]
    fn every_transform_round_trips_through_niri() {
        for out in outputs() {
            let (w, h) = (f64::from(out.width), f64::from(out.height));
            for (fx, fy) in [(0.01_f64, 0.02_f64), (0.5, 0.5), (0.99, 0.3), (0.25, 0.999)] {
                let target = LayoutPt {
                    x: fx.mul_add(w, f64::from(out.x)),
                    y: fy.mul_add(h, f64::from(out.y)),
                };
                assert_lands(target, &out);
            }
        }
    }

    #[test]
    fn extents_are_the_untransformed_size_times_k() {
        let rotated = output(0, 0, (1440, 2560), Transform::_90);
        let request = encode(LayoutPt { x: 1.0, y: 1.0 }, &rotated);
        assert_eq!((request.x_extent, request.y_extent), (2_560_000, 1_440_000));
    }

    #[test]
    fn flipped180_mirrors_only_the_vertical_axis() {
        let nested = output(0, 0, (960, 720), Transform::Flipped180);
        let request = checked_encode(LayoutPt { x: 10.0, y: 10.0 }, &nested).unwrap();
        assert_eq!(
            request,
            ProtocolPt {
                x: 10_000,
                y: 710_000,
                x_extent: 960_000,
                y_extent: 720_000,
            }
        );
        assert_eq!(
            niri_forward(request, &nested),
            LayoutPt { x: 10.0, y: 10.0 }
        );
    }

    #[test]
    fn pixel_centres_map_through_capture_scale_and_crop() {
        let capture = Capture {
            origin: LayoutPt { x: 100.0, y: -50.0 },
            scale: 0.5,
        };
        let point = image_to_layout(ImagePx { x: 3, y: 0 }, capture);
        assert_eq!(point, LayoutPt { x: 107.0, y: -49.0 });
    }

    #[test]
    fn captured_pixels_land_within_tolerance() {
        let pixels = [ImagePx { x: 0, y: 0 }, ImagePx { x: 123, y: 400 }];
        let scales = [0.5, 0.75, 1.0, 1.25, 1.5];
        for out in ALL.map(|transform| output(-1280, 0, (1280, 1024), transform)) {
            for (scale, pixel) in scales
                .into_iter()
                .flat_map(|scale| pixels.map(|pixel| (scale, pixel)))
            {
                let capture = Capture {
                    origin: LayoutPt {
                        x: -1000.0,
                        y: 200.0,
                    },
                    scale,
                };
                assert_lands(image_to_layout(pixel, capture), &out);
            }
        }
    }

    #[test]
    fn a_point_off_the_output_is_refused() {
        let out = output(0, 0, (960, 720), Transform::Flipped180);
        // The far edge is clamped to the last encodable point, within the tolerance.
        let edge = checked_encode(LayoutPt { x: 960.0, y: 0.0 }, &out).unwrap();
        assert_eq!((edge.x, edge.y), (959_999, 719_999));
        let outside = checked_encode(LayoutPt { x: 1000.0, y: -5.0 }, &out);
        assert!(outside.is_err_and(|error| error > TOLERANCE));
    }
}
