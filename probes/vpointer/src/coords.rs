//! The coordinate contract from plan §8: image pixel → layout point → output-local point →
//! untransformed output point → `motion_absolute` arguments, plus a port of niri's forward
//! mapping to check each encoded request.

/// `motion_absolute` resolution: extents are the untransformed logical size times `K`.
pub(crate) const K: f64 = 1000.0;

/// How far niri's forward mapping may land from the target, in logical pixels.
pub(crate) const TOLERANCE: f64 = 0.002;

/// A pixel in a captured image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ImagePx {
    pub(crate) x: u32,
    pub(crate) y: u32,
}

/// A point in niri's global layout space, in logical pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct LayoutPt {
    pub(crate) x: f64,
    pub(crate) y: f64,
}

/// A point relative to the output's top-left corner, in logical pixels, as displayed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct OutputLocalPt {
    pub(crate) x: f64,
    pub(crate) y: f64,
}

/// A point in the output's untransformed logical space, which `motion_absolute` addresses.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct UntransformedPt {
    pub(crate) x: f64,
    pub(crate) y: f64,
}

/// The arguments of one `zwlr_virtual_pointer_v1.motion_absolute` request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ProtocolPt {
    pub(crate) x: u32,
    pub(crate) y: u32,
    pub(crate) x_extent: u32,
    pub(crate) y_extent: u32,
}

/// niri's output transforms. `parse` takes the names niri's IPC uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Transform {
    Normal,
    Rotate90,
    Rotate180,
    Rotate270,
    Flipped,
    Flipped90,
    Flipped180,
    Flipped270,
}

impl Transform {
    #[cfg(test)]
    pub(crate) const ALL: [Self; 8] = [
        Self::Normal,
        Self::Rotate90,
        Self::Rotate180,
        Self::Rotate270,
        Self::Flipped,
        Self::Flipped90,
        Self::Flipped180,
        Self::Flipped270,
    ];

    pub(crate) fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "Normal" => Self::Normal,
            "90" => Self::Rotate90,
            "180" => Self::Rotate180,
            "270" => Self::Rotate270,
            "Flipped" => Self::Flipped,
            "Flipped90" => Self::Flipped90,
            "Flipped180" => Self::Flipped180,
            "Flipped270" => Self::Flipped270,
            _ => return None,
        })
    }

    /// Smithay's `Transform::invert`.
    const fn invert(self) -> Self {
        match self {
            Self::Rotate90 => Self::Rotate270,
            Self::Rotate270 => Self::Rotate90,
            Self::Flipped90 => Self::Flipped270,
            Self::Flipped270 => Self::Flipped90,
            other => other,
        }
    }

    /// Smithay's `Transform::transform_size`.
    const fn transform_size(self, (w, h): (i32, i32)) -> (i32, i32) {
        match self {
            Self::Rotate90 | Self::Rotate270 | Self::Flipped90 | Self::Flipped270 => (h, w),
            _ => (w, h),
        }
    }

    /// Smithay's `Transform::transform_point_in`, copied from Smithay `ff5fa7df`
    /// (`src/utils/geometry.rs`), the revision niri 26.04 pins.
    fn transform_point_in(self, (x, y): (f64, f64), (w, h): (f64, f64)) -> (f64, f64) {
        match self {
            Self::Normal => (x, y),
            Self::Rotate90 => (h - y, x),
            Self::Rotate180 => (w - x, h - y),
            Self::Rotate270 => (y, w - x),
            Self::Flipped => (w - x, y),
            Self::Flipped90 => (y, x),
            Self::Flipped180 => (x, h - y),
            Self::Flipped270 => (h - y, w - x),
        }
    }
}

/// An output as niri's IPC reports it: logical position and size (after the transform).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Output {
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) width: i32,
    pub(crate) height: i32,
    pub(crate) transform: Transform,
}

impl Output {
    /// What niri's `compute_absolute_location` calls `size`.
    const fn untransformed_size(&self) -> (i32, i32) {
        self.transform
            .invert()
            .transform_size((self.width, self.height))
    }
}

/// Where and at what scale an image was captured.
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

pub(crate) fn layout_to_output(point: LayoutPt, output: &Output) -> OutputLocalPt {
    OutputLocalPt {
        x: point.x - f64::from(output.x),
        y: point.y - f64::from(output.y),
    }
}

/// Inverts niri's `transform.transform_point_in(point, size)`, where `size` is the
/// untransformed size. Smithay's `Transform::invert` is not that inverse: every flipped
/// transform undoes itself, but `invert` swaps `Flipped90` and `Flipped270`.
pub(crate) fn output_to_untransformed(point: OutputLocalPt, output: &Output) -> UntransformedPt {
    let (w, h) = output.untransformed_size();
    let (w, h) = (f64::from(w), f64::from(h));
    let OutputLocalPt { x, y } = point;
    let (x, y) = match output.transform {
        Transform::Normal => (x, y),
        Transform::Rotate90 => (y, h - x),
        Transform::Rotate180 => (w - x, h - y),
        Transform::Rotate270 => (w - y, x),
        Transform::Flipped => (w - x, y),
        Transform::Flipped90 => (y, x),
        Transform::Flipped180 => (x, h - y),
        Transform::Flipped270 => (w - y, h - x),
    };
    UntransformedPt { x, y }
}

/// `x = round(ux × K)`, clamped to `[0, extent − 1]`, with `extent = width × K`.
pub(crate) fn encode(target: LayoutPt, output: &Output) -> ProtocolPt {
    let point = output_to_untransformed(layout_to_output(target, output), output);
    let (w, h) = output.untransformed_size();
    let (x, x_extent) = encode_axis(point.x, w);
    let (y, y_extent) = encode_axis(point.y, h);
    ProtocolPt {
        x,
        y,
        x_extent,
        y_extent,
    }
}

fn encode_axis(value: f64, size: i32) -> (u32, u32) {
    let extent = f64::from(size) * K;
    let encoded = (value * K).round().clamp(0.0, extent - 1.0);
    (encoded as u32, extent as u32)
}

/// niri's mapping of an absolute motion on a pointer bound to `output`: the virtual
/// pointer's `x_transformed` (`src/protocols/virtual_pointer.rs`), then
/// `compute_absolute_location` (`src/input/mod.rs`), at `v26.04`.
pub(crate) fn niri_forward(request: ProtocolPt, output: &Output) -> LayoutPt {
    let (w, h) = output.untransformed_size();
    let x = (i64::from(request.x) * i64::from(w)) as f64 / f64::from(request.x_extent);
    let y = (i64::from(request.y) * i64::from(h)) as f64 / f64::from(request.y_extent);
    let (x, y) = output
        .transform
        .transform_point_in((x, y), (f64::from(w), f64::from(h)));
    LayoutPt {
        x: x + f64::from(output.x),
        y: y + f64::from(output.y),
    }
}

/// Encodes `target`, and refuses with the distance if niri's own mapping would put the
/// pointer more than `TOLERANCE` away from it.
pub(crate) fn checked_encode(target: LayoutPt, output: &Output) -> Result<ProtocolPt, f64> {
    let request = encode(target, output);
    let error = forward_error(target, request, output);
    if error <= TOLERANCE {
        Ok(request)
    } else {
        Err(error)
    }
}

/// The largest per-axis distance between the target and where niri puts the pointer.
pub(crate) fn forward_error(target: LayoutPt, request: ProtocolPt, output: &Output) -> f64 {
    let landed = niri_forward(request, output);
    (landed.x - target.x).abs().max((landed.y - target.y).abs())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output(x: i32, y: i32, (width, height): (i32, i32), transform: Transform) -> Output {
        Output {
            x,
            y,
            width,
            height,
            transform,
        }
    }

    /// Displayed logical sizes for 2560x1440 at scale 1, 1.25 and 1.5, the nested
    /// 960x720 window at 1 and 1.5, and an odd size.
    const SIZES: [(i32, i32); 6] = [
        (2560, 1440),
        (2048, 1152),
        (1707, 960),
        (960, 720),
        (640, 480),
        (1001, 333),
    ];

    fn assert_lands(target: LayoutPt, output: &Output) {
        let request = encode(target, output);
        let error = forward_error(target, request, output);
        assert!(
            error <= TOLERANCE,
            "{target:?} on {output:?} encoded as {request:?} lands {error} away"
        );
    }

    fn outputs() -> impl Iterator<Item = Output> {
        Transform::ALL.into_iter().flat_map(|transform| {
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
            for (fx, fy) in [(0.01, 0.02), (0.5, 0.5), (0.99, 0.3), (0.25, 0.999)] {
                let target = LayoutPt {
                    x: f64::from(out.x) + fx * w,
                    y: f64::from(out.y) + fy * h,
                };
                assert_lands(target, &out);
            }
        }
    }

    #[test]
    fn extents_are_the_untransformed_size_times_k() {
        let rotated = output(0, 0, (1440, 2560), Transform::Rotate90);
        let request = encode(LayoutPt { x: 1.0, y: 1.0 }, &rotated);
        assert_eq!((request.x_extent, request.y_extent), (2_560_000, 1_440_000));
    }

    #[test]
    fn flipped180_mirrors_only_the_vertical_axis() {
        let nested = output(0, 0, (960, 720), Transform::Flipped180);
        let request = encode(LayoutPt { x: 10.0, y: 10.0 }, &nested);
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
        for out in Transform::ALL.map(|transform| output(-1280, 0, (1280, 1024), transform)) {
            for (scale, pixel) in [0.5, 0.75, 1.0, 1.25, 1.5]
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
    fn the_far_edge_is_clamped_inside_the_output() {
        let out = output(0, 0, (960, 720), Transform::Normal);
        let request = encode(LayoutPt { x: 960.0, y: 0.0 }, &out);
        assert_eq!((request.x, request.y), (959_999, 0));
        let outside = encode(LayoutPt { x: 1000.0, y: -5.0 }, &out);
        assert!(forward_error(LayoutPt { x: 1000.0, y: -5.0 }, outside, &out) > TOLERANCE);
    }

    #[test]
    fn parses_niri_transform_names() {
        // `niri_ipc::Transform` renames `_90`, `_180` and `_270` to "90", "180" and "270"
        // (niri-ipc 26.4.0, `src/lib.rs`).
        let names = [
            "Normal",
            "90",
            "180",
            "270",
            "Flipped",
            "Flipped90",
            "Flipped180",
            "Flipped270",
        ];
        assert_eq!(names.map(Transform::parse), Transform::ALL.map(Some));
        assert_eq!(Transform::parse("_90"), None);
    }

    #[test]
    fn checked_encode_refuses_a_target_outside_the_output() {
        let out = output(0, 0, (960, 720), Transform::Flipped180);
        let inside = checked_encode(LayoutPt { x: 10.0, y: 10.0 }, &out);
        assert_eq!(inside.map(|request| request.y), Ok(710_000));
        let outside = checked_encode(LayoutPt { x: 1000.0, y: -5.0 }, &out);
        assert!(outside.is_err_and(|error| error > TOLERANCE));
    }
}
