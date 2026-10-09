//! Screenshots through grim, with an explicit output or region and an explicit scale.
//!
//! grim 1.5.0 sizes its image as `int common_width = geometry->width * scale`
//! (`render.c:145–146`), which truncates. The expected size follows the same rule, and a
//! capture whose header disagrees is an error rather than an image with unknown geometry.

use std::path::Path;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use niri_ipc::{LogicalOutput, Output, Transform};
use serde::Serialize;

use crate::error::{CallError, ErrorName, ToolError};
use crate::{image_header, niri, runner};

/// The slowest capture in M0 (C15) took 275 ms.
const GRIM_DEADLINE: Duration = Duration::from_secs(5);
/// Far above a 4K PNG of busy content.
const MAX_IMAGE: u64 = 64 * 1024 * 1024;
const JPEG_QUALITY: &str = "80";
/// The widest image returned unless the caller asks otherwise (plan §6).
pub(crate) const DEFAULT_MAX_WIDTH: u32 = 1280;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Format {
    Png,
    #[default]
    Jpeg,
}

impl Format {
    const fn mime(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
        }
    }
}

/// A rectangle in niri's logical layout coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub(crate) struct Rect {
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Target {
    Output(String),
    FocusedOutput,
    Region(Rect),
}

impl Target {
    /// `output:<name>`, `focused_output`, or `region` with its rectangle.
    pub(crate) fn parse(target: &str, region: Option<Rect>) -> Result<Self, String> {
        match (target, region) {
            ("focused_output", None) => Ok(Self::FocusedOutput),
            ("region", Some(rect)) => Ok(Self::Region(rect)),
            ("region", None) => Err("target `region` needs a `region` rectangle".to_owned()),
            (_, Some(_)) => Err("`region` is only allowed with target `region`".to_owned()),
            (other, None) => match other.strip_prefix("output:") {
                Some(name) if !name.is_empty() => Ok(Self::Output(name.to_owned())),
                _ => Err(format!(
                    "unknown target {other:?}; use `focused_output`, `output:<name>` or `region`"
                )),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Request {
    pub(crate) target: Target,
    pub(crate) max_width: Option<u32>,
    pub(crate) format: Format,
}

impl Request {
    /// The focused output as a JPEG at the default width, as results carry it.
    pub(crate) const fn focused() -> Self {
        Self {
            target: Target::FocusedOutput,
            max_width: Some(DEFAULT_MAX_WIDTH),
            format: Format::Jpeg,
        }
    }
}

/// What a capture shows and how its pixels map back to the layout.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct Metadata {
    pub(crate) output: String,
    pub(crate) transform: Transform,
    /// The output's top-left corner in the layout.
    pub(crate) output_origin: (i32, i32),
    /// The captured rectangle in layout coordinates.
    pub(crate) captured: Rect,
    /// Image pixels per logical pixel, as passed to `grim -s`.
    pub(crate) scale: f64,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) mime_type: &'static str,
    pub(crate) captured_at_unix_ms: u128,
    pub(crate) capture_ms: u128,
    /// What pointer tools take to aim at a pixel of this image; null unless this server
    /// holds the lease.
    pub(crate) screenshot_ref: Option<String>,
    /// For a screenshot that waited for the screen to stop changing: whether it did within
    /// the wait.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) settled: Option<bool>,
}

#[derive(Debug)]
pub(crate) struct Screenshot {
    pub(crate) metadata: Metadata,
    /// The captured output as niri described it.
    pub(crate) geometry: LogicalOutput,
    pub(crate) image: Vec<u8>,
}

pub(crate) async fn screenshot(
    socket: Option<&Path>,
    request: &Request,
) -> Result<Screenshot, CallError> {
    let outputs = niri::outputs(socket).await?;
    let focused = match request.target {
        Target::FocusedOutput => niri::focused_output(socket)
            .await?
            .map(|output| output.name),
        Target::Output(_) | Target::Region(_) => None,
    };
    let output = pick(&request.target, outputs.values(), focused.as_deref())
        .map_err(CallError::InvalidArguments)?;
    let plan = plan(output, &request.target, request.max_width, request.format)
        .map_err(CallError::InvalidArguments)?;
    let started = Instant::now();
    let captured_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis());
    let done = runner::run("grim", &plan.args, GRIM_DEADLINE, MAX_IMAGE).await?;
    if !done.status.success() {
        return Err(done.failure("grim").into());
    }
    let size = match request.format {
        Format::Png => image_header::png_size(&done.stdout),
        Format::Jpeg => image_header::jpeg_size(&done.stdout),
    };
    if size != Some(plan.size) {
        return Err(ToolError::new(
            ErrorName::UpstreamError,
            format!(
                "grim wrote an image of size {size:?}, expected {:?}",
                plan.size
            ),
        )
        .into());
    }
    Ok(Screenshot {
        metadata: Metadata {
            captured_at_unix_ms: captured_at,
            capture_ms: started.elapsed().as_millis(),
            ..plan.metadata
        },
        geometry: *output.logical,
        image: done.stdout,
    })
}

/// A target's output, with its logical geometry.
#[derive(Debug, Clone, Copy)]
struct Picked<'a> {
    name: &'a str,
    logical: &'a LogicalOutput,
}

fn pick<'a>(
    target: &Target,
    mut outputs: impl Iterator<Item = &'a Output>,
    focused: Option<&str>,
) -> Result<Picked<'a>, String> {
    let with_logical = |output: &'a Output| {
        output.logical.as_ref().map(|logical| Picked {
            name: &output.name,
            logical,
        })
    };
    match target {
        Target::Output(name) => outputs
            .find(|output| &output.name == name)
            .and_then(with_logical)
            .ok_or_else(|| format!("no enabled output named {name:?}")),
        Target::FocusedOutput => {
            let name = focused.ok_or("niri reports no focused output")?;
            outputs
                .find(|output| output.name == name)
                .and_then(with_logical)
                .ok_or_else(|| format!("the focused output {name:?} has no logical geometry"))
        }
        Target::Region(rect) => outputs
            .filter_map(with_logical)
            .find(|picked| contains(picked.logical, rect))
            .ok_or_else(|| {
                format!(
                    "region {},{} {}x{} is not inside one output",
                    rect.x, rect.y, rect.width, rect.height
                )
            }),
    }
}

fn contains(output: &LogicalOutput, rect: &Rect) -> bool {
    let (x, y) = (i64::from(rect.x), i64::from(rect.y));
    let (left, top) = (i64::from(output.x), i64::from(output.y));
    rect.width > 0
        && rect.height > 0
        && x >= left
        && y >= top
        && x + i64::from(rect.width) <= left + i64::from(output.width)
        && y + i64::from(rect.height) <= top + i64::from(output.height)
}

/// grim's arguments, the image size they must produce, and the metadata to return.
#[derive(Debug, PartialEq)]
struct Plan {
    args: Vec<String>,
    size: (u32, u32),
    metadata: Metadata,
}

fn plan(
    output: Picked<'_>,
    target: &Target,
    max_width: Option<u32>,
    format: Format,
) -> Result<Plan, String> {
    let logical = output.logical;
    let captured = match target {
        Target::Region(rect) => *rect,
        Target::Output(_) | Target::FocusedOutput => Rect {
            x: logical.x,
            y: logical.y,
            width: logical.width,
            height: logical.height,
        },
    };
    let scale = capture_scale(logical.scale, captured.width, max_width)?;
    let size = (
        grim_pixels(captured.width, scale),
        grim_pixels(captured.height, scale),
    );
    if size.0 == 0 || size.1 == 0 {
        return Err(format!("a {size:?} image is empty; raise `max_width`"));
    }
    let mut args: Vec<String> = match format {
        Format::Png => vec!["-t".into(), "png".into()],
        Format::Jpeg => vec!["-t".into(), "jpeg".into(), "-q".into(), JPEG_QUALITY.into()],
    };
    args.extend(["-s".to_owned(), scale.to_string()]);
    match target {
        Target::Region(rect) => args.extend([
            "-g".to_owned(),
            format!("{},{} {}x{}", rect.x, rect.y, rect.width, rect.height),
        ]),
        Target::Output(_) | Target::FocusedOutput => {
            args.extend(["-o".to_owned(), output.name.to_owned()]);
        }
    }
    args.push("-".to_owned());
    Ok(Plan {
        args,
        size,
        metadata: Metadata {
            output: output.name.to_owned(),
            transform: logical.transform,
            output_origin: (logical.x, logical.y),
            captured,
            scale,
            width: size.0,
            height: size.1,
            mime_type: format.mime(),
            captured_at_unix_ms: 0,
            capture_ms: 0,
            screenshot_ref: None,
            settled: None,
        },
    })
}

/// The output's own scale, lowered so the image is at most `max_width` pixels wide. The
/// lowered scale is nudged up until grim's truncation gives exactly `max_width`, because
/// `max_width / width` can land a hair below it in floating point.
fn capture_scale(native: f64, width: u32, max_width: Option<u32>) -> Result<f64, String> {
    let Some(max_width) = max_width else {
        return Ok(native);
    };
    if max_width == 0 {
        return Err("`max_width` must be at least 1".to_owned());
    }
    if grim_pixels(width, native) <= max_width {
        return Ok(native);
    }
    let mut scale = f64::from(max_width) / f64::from(width);
    while grim_pixels(width, scale) < max_width {
        scale = scale.next_up();
    }
    Ok(scale)
}

/// grim's image size for `logical` pixels at `scale`.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "matches grim's `int common_width = geometry->width * scale` (render.c:145)"
)]
fn grim_pixels(logical: u32, scale: f64) -> u32 {
    (f64::from(logical) * scale) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn logical(x: i32, y: i32, width: u32, height: u32, scale: f64) -> LogicalOutput {
        LogicalOutput {
            x,
            y,
            width,
            height,
            scale,
            transform: Transform::Normal,
        }
    }

    fn output(name: &str, geometry: Option<LogicalOutput>) -> Output {
        Output {
            name: name.to_owned(),
            make: String::new(),
            model: String::new(),
            serial: None,
            physical_size: None,
            modes: Vec::new(),
            current_mode: None,
            is_custom_mode: false,
            vrr_supported: false,
            vrr_enabled: false,
            logical: geometry,
        }
    }

    #[test]
    fn parses_the_three_targets() {
        assert_eq!(
            Target::parse("focused_output", None),
            Ok(Target::FocusedOutput)
        );
        assert_eq!(
            Target::parse("output:DP-1", None),
            Ok(Target::Output("DP-1".to_owned()))
        );
        let rect = Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };
        assert_eq!(
            Target::parse("region", Some(rect)),
            Ok(Target::Region(rect))
        );
        for (target, region) in [
            ("region", None),
            ("output:", None),
            ("DP-1", None),
            ("focused_output", Some(rect)),
        ] {
            assert!(Target::parse(target, region).is_err(), "{target}");
        }
    }

    #[test]
    fn a_region_must_fit_inside_one_output() {
        let outputs = [
            output("DP-1", Some(logical(0, 0, 2560, 1440, 1.0))),
            output("HDMI-A-1", Some(logical(2560, 0, 1920, 1080, 1.0))),
            output("off", None),
        ];
        let region = |x, y, width, height| {
            Target::Region(Rect {
                x,
                y,
                width,
                height,
            })
        };
        let picked = |target: &Target| pick(target, outputs.iter(), None).map(|p| p.name);
        assert_eq!(picked(&region(2560, 0, 100, 100)), Ok("HDMI-A-1"));
        assert_eq!(picked(&region(0, 0, 2560, 1440)), Ok("DP-1"));
        assert!(
            picked(&region(2500, 0, 100, 100)).is_err(),
            "spans two outputs"
        );
        assert!(picked(&region(-1, 0, 10, 10)).is_err());
        assert!(picked(&region(0, 0, 0, 10)).is_err());
        assert!(picked(&Target::Output("off".to_owned())).is_err());
        assert!(picked(&Target::Output("nope".to_owned())).is_err());
        assert_eq!(
            pick(&Target::FocusedOutput, outputs.iter(), Some("HDMI-A-1")).map(|p| p.name),
            Ok("HDMI-A-1")
        );
        assert!(pick(&Target::FocusedOutput, outputs.iter(), None).is_err());
    }

    #[test]
    fn max_width_gives_exactly_that_width_under_grim_truncation() {
        assert_eq!(capture_scale(1.5, 640, None), Ok(1.5));
        assert_eq!(capture_scale(1.0, 2560, Some(4000)), Ok(1.0));
        assert!(capture_scale(1.0, 2560, Some(0)).is_err());
        for width in 1281..4000 {
            for native in [1.0, 1.25, 1.5, 2.0] {
                let scale = capture_scale(native, width, Some(1280)).unwrap();
                assert_eq!(grim_pixels(width, scale), 1280, "{width} at {native}");
                // The text grim parses is the same double.
                assert_eq!(scale.to_string().parse::<f64>().unwrap(), scale);
            }
        }
    }

    #[test]
    fn plans_grim_arguments_and_the_expected_size() {
        let geometry = logical(0, 0, 2560, 1440, 1.0);
        let picked = || Picked {
            name: "DP-1",
            logical: &geometry,
        };
        let whole = plan(picked(), &Target::FocusedOutput, Some(1280), Format::Jpeg).unwrap();
        assert_eq!(
            whole.args,
            ["-t", "jpeg", "-q", "80", "-s", "0.5", "-o", "DP-1", "-"]
        );
        assert_eq!(whole.size, (1280, 720));
        assert_eq!(whole.metadata.mime_type, "image/jpeg");

        let rect = Rect {
            x: 10,
            y: 20,
            width: 301,
            height: 7,
        };
        let region = plan(picked(), &Target::Region(rect), None, Format::Png).unwrap();
        assert_eq!(
            region.args,
            ["-t", "png", "-s", "1", "-g", "10,20 301x7", "-"]
        );
        assert_eq!(region.size, (301, 7));
        assert_eq!(region.metadata.captured, rect);

        // 7 logical pixels at the scale that makes 301 wide fit in 2 is 0 pixels high.
        assert!(plan(picked(), &Target::Region(rect), Some(2), Format::Png).is_err());
    }
}
