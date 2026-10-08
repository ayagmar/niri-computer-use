//! Pointer checks against `wev`, driven by the `vpointer` probe. Each probe call carries its
//! own event time, and niri passes that time on to `wev`, so a check only reads the events
//! its own call caused, even if the host pointer crosses the nested window meanwhile.

use std::ffi::OsString;
use std::fs;
use std::path::Path;
use std::time::Duration;

use niri_ipc::{LogicalOutput, Transform, WindowLayout};

use crate::failure::{Context as _, Failure, Result};
use crate::session::Session;
use crate::wev::{self, Event};

/// A point in niri's layout space, in logical pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
struct LayoutPt {
    x: f64,
    y: f64,
}

/// A point relative to `wev`'s surface, which is what `wev` logs.
#[derive(Debug, Clone, Copy, PartialEq)]
struct SurfacePt {
    x: f64,
    y: f64,
}

impl SurfacePt {
    fn in_layout(self, surface: LayoutPt) -> LayoutPt {
        LayoutPt {
            x: surface.x + self.x,
            y: surface.y + self.y,
        }
    }

    /// The larger of the two axis distances.
    fn distance(self, other: Self) -> f64 {
        (self.x - other.x).abs().max((self.y - other.y).abs())
    }
}

impl LayoutPt {
    fn on_surface(self, surface: Self) -> SurfacePt {
        SurfacePt {
            x: self.x - surface.x,
            y: self.y - surface.y,
        }
    }
}

impl From<(f64, f64)> for SurfacePt {
    fn from((x, y): (f64, f64)) -> Self {
        Self { x, y }
    }
}

/// C4's surface points in `wev`.
const POINTS: [SurfacePt; 5] = [
    SurfacePt { x: 10.0, y: 10.0 },
    SurfacePt { x: 390.0, y: 10.0 },
    SurfacePt { x: 10.0, y: 290.0 },
    SurfacePt { x: 390.0, y: 290.0 },
    SurfacePt { x: 200.0, y: 150.0 },
];
/// Where the pointer first goes, so that it enters `wev`.
const ENTER_POINT: SurfacePt = SurfacePt { x: 100.0, y: 100.0 };
/// C4's pass rule, in logical pixels, including the client's `wl_fixed` rounding.
const TOLERANCE: f64 = 0.05;
/// An image pixel of a whole-output capture at the output's scale. The probe maps it
/// through the pixel-centre convention.
const IMAGE_PIXEL: (u32, u32) = (300, 225);
const WAIT: Duration = Duration::from_secs(5);
const BTN_LEFT: &str = "272";

const ENTER_TIME: u32 = 4000;
const C4_TIME: u32 = 4001;
const CLICK_TIME: u32 = 8000;
const SCROLL_TIME: u32 = 12000;

/// The probe, bound to the nested output.
#[derive(Debug)]
pub(crate) struct Probe<'a> {
    pub(crate) path: &'a str,
    pub(crate) output: &'a LogicalOutput,
}

impl Probe<'_> {
    pub(crate) fn send(
        &self,
        session: &mut Session<'_>,
        time: u32,
        action: &[String],
    ) -> Result<()> {
        let args = self.args(time, action)?;
        let sent = session.run(self.path, &args)?;
        for line in String::from_utf8_lossy(&sent.stdout).lines() {
            session.log(&format!("  vpointer: {line}"))?;
        }
        Ok(())
    }

    pub(crate) fn args(&self, time: u32, action: &[String]) -> Result<Vec<OsString>> {
        let output = self.output;
        let geometry = format!(
            "{},{},{},{}",
            output.x, output.y, output.width, output.height
        );
        let mut args: Vec<OsString> = vec![
            "winit".into(),
            geometry.into(),
            transform_name(output.transform)?.into(),
            time.to_string().into(),
        ];
        args.extend(action.iter().map(OsString::from));
        Ok(args)
    }
}

/// The name niri's IPC gives a transform, which the probe parses.
fn transform_name(transform: Transform) -> Result<String> {
    let value = serde_json::to_value(transform).context("name the transform")?;
    value
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| Failure::new(format!("transform serialized as {value}")))
}

pub(crate) fn run(
    session: &mut Session<'_>,
    probe: &Probe<'_>,
    wev_log: &Path,
    window: &WindowLayout,
) -> Result<()> {
    let surface = surface_origin(probe.output, window)?;
    enter(session, probe, wev_log, surface)?;
    c4(session, probe, wev_log, surface)?;
    click(session, probe, wev_log)?;
    c12(session, probe, wev_log)
}

/// Shared pointer entry for automatic and supervised checks.
pub(crate) fn enter_window(
    session: &mut Session<'_>,
    probe: &Probe<'_>,
    wev_log: &Path,
    window: &WindowLayout,
) -> Result<()> {
    enter(
        session,
        probe,
        wev_log,
        surface_origin(probe.output, window)?,
    )
}

/// The layout position of `wev`'s surface. Borders are off, so it is the tile position
/// plus a zero offset.
fn surface_origin(output: &LogicalOutput, window: &WindowLayout) -> Result<LayoutPt> {
    let (tile_x, tile_y) = window
        .tile_pos_in_workspace_view
        .ok_or_else(|| Failure::new("wev has no position in the workspace view"))?;
    let (offset_x, offset_y) = window.window_offset_in_tile;
    Ok(LayoutPt {
        x: f64::from(output.x) + tile_x + offset_x,
        y: f64::from(output.y) + tile_y + offset_y,
    })
}

fn read(wev_log: &Path) -> Result<String> {
    fs::read_to_string(wev_log).context(format!("read {}", wev_log.display()))
}

fn motion_to(target: LayoutPt) -> Vec<String> {
    vec![
        "motion-layout".to_owned(),
        target.x.to_string(),
        target.y.to_string(),
    ]
}

/// Moves the pointer into `wev`. Either `wev` logs `wl_pointer.enter` at that point, or
/// the pointer was already inside and `wev` logs the motion itself.
fn enter(
    session: &mut Session<'_>,
    probe: &Probe<'_>,
    wev_log: &Path,
    surface: LayoutPt,
) -> Result<()> {
    probe.send(
        session,
        ENTER_TIME,
        &motion_to(ENTER_POINT.in_layout(surface)),
    )?;
    let seen = session.wait_until("pointer-enter", "the pointer in wev", WAIT, |_| {
        let log = read(wev_log)?;
        Ok(entry_observation(&log))
    })?;
    session.log(&format!("pointer: wev logged {seen}"))
}

fn entry_observation(log: &str) -> Option<&'static str> {
    let entered = wev::enters(log)
        .into_iter()
        .any(|at| SurfacePt::from(at).distance(ENTER_POINT) <= TOLERANCE);
    match (entered, wev::motion(log, ENTER_TIME).is_some()) {
        (true, _) => Some("wl_pointer.enter at the probe's point"),
        (false, true) => Some("the probe's motion; the pointer was already inside"),
        (false, false) => None,
    }
}

/// C4: each surface point, then one image pixel, must arrive within ±0.05 px.
fn c4(
    session: &mut Session<'_>,
    probe: &Probe<'_>,
    wev_log: &Path,
    surface: LayoutPt,
) -> Result<()> {
    let output = probe.output;
    let (px, py) = IMAGE_PIXEL;
    let origin = LayoutPt {
        x: f64::from(output.x),
        y: f64::from(output.y),
    };
    // The expected point, worked out here independently of the probe's encoding.
    let pixel_centre = LayoutPt {
        x: origin.x + (f64::from(px) + 0.5) / output.scale,
        y: origin.y + (f64::from(py) + 0.5) / output.scale,
    };
    let image_motion: Vec<String> = [
        "motion-image".to_owned(),
        px.to_string(),
        py.to_string(),
        origin.x.to_string(),
        origin.y.to_string(),
        output.scale.to_string(),
    ]
    .into();
    let cases = POINTS
        .iter()
        .map(|&point| (point, motion_to(point.in_layout(surface))))
        .chain([(pixel_centre.on_surface(surface), image_motion)]);
    let mut failed = false;
    for ((target, action), time) in cases.zip(C4_TIME..) {
        probe.send(session, time, &action)?;
        let seen = session.wait_until("c4", "wl_pointer.motion in wev", WAIT, |_| {
            Ok(wev::motion(&read(wev_log)?, time).map(SurfacePt::from))
        })?;
        let off = seen.distance(target);
        let pass = off <= TOLERANCE;
        failed |= !pass;
        session.log(&format!(
            "C4 ({:.4}, {:.4}): wev ({:.6}, {:.6}), off by {off:.4}: {}",
            target.x,
            target.y,
            seen.x,
            seen.y,
            if pass { "pass" } else { "fail" }
        ))?;
    }
    if failed {
        Err(Failure::new("C4: a point landed outside ±0.05 px"))
    } else {
        Ok(())
    }
}

/// Not a criterion: checks the probe's button path, which C8 uses later.
fn click(session: &mut Session<'_>, probe: &Probe<'_>, wev_log: &Path) -> Result<()> {
    probe.send(
        session,
        CLICK_TIME,
        &["click".to_owned(), BTN_LEFT.to_owned()],
    )?;
    let buttons = session.wait_until("click", "a press and a release in wev", WAIT, |_| {
        let log = read(wev_log)?;
        let buttons = wev::buttons(&log, CLICK_TIME);
        Ok((buttons.len() >= 2).then(|| buttons.join("; ")))
    })?;
    session.log(&format!("click: wev logged {buttons}"))?;
    let expected = "272 (left), state: 1 (pressed); 272 (left), state: 0 (released)";
    if buttons == expected {
        Ok(())
    } else {
        Err(Failure::new(format!("click: expected {expected}")))
    }
}

/// C12: `axis_discrete(vertical, 15.0, 1)`, `axis_source(wheel)`, `frame`.
fn c12(session: &mut Session<'_>, probe: &Probe<'_>, wev_log: &Path) -> Result<()> {
    probe.send(session, SCROLL_TIME, &["scroll".to_owned()])?;
    let frame = session.wait_until("c12", "an axis frame in wev", WAIT, |_| {
        let log = read(wev_log)?;
        Ok(wev::axis_frame(&log, SCROLL_TIME).map(|frame| describe(&frame)))
    })?;
    let pass = c12_passes(&frame);
    session.log(&format!(
        "C12: wev frame [{}]: {}",
        frame.join("; "),
        if pass { "pass" } else { "fail" }
    ))?;
    if pass {
        Ok(())
    } else {
        Err(Failure::new(
            "C12: wheel source, value120 120 or value 15 missing",
        ))
    }
}

fn describe(frame: &[Event<'_>]) -> Vec<String> {
    frame
        .iter()
        .map(|event| format!("{}: {}", event.name, event.detail))
        .collect()
}

fn c12_passes(frame: &[String]) -> bool {
    let has = |line: &str| frame.iter().any(|event| event == line);
    let axis = frame.iter().any(|event| {
        event.starts_with("axis: time: ")
            && event.ends_with("; axis: 0 (vertical), value: 15.000000")
    });
    has("axis_source: 0 (wheel)") && has("axis_value120: axis: 0 (vertical), value120: 120") && axis
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_accepts_enter_without_a_motion_event() {
        let enter =
            "[ 15: wl_pointer] enter: serial: 3997; surface: 3, x, y: 100.000000, 100.000000\n";
        assert_eq!(
            entry_observation(enter),
            Some("wl_pointer.enter at the probe's point")
        );
        assert_eq!(
            entry_observation(&enter.replace("100.000000", "200.000000")),
            None
        );
        assert_eq!(
            entry_observation(
                "[ 15: wl_pointer] motion: time: 4000; x, y: 100.000000, 100.000000\n"
            ),
            Some("the probe's motion; the pointer was already inside")
        );
        assert_eq!(entry_observation(""), None);
    }

    #[test]
    fn surface_origin_adds_output_tile_and_offset() {
        let output = LogicalOutput {
            x: 100,
            y: -20,
            width: 640,
            height: 480,
            scale: 1.5,
            transform: Transform::Flipped180,
        };
        let window = WindowLayout {
            pos_in_scrolling_layout: None,
            tile_size: (404.0, 304.0),
            window_size: (400, 300),
            tile_pos_in_workspace_view: Some((10.0, 5.0)),
            window_offset_in_tile: (2.0, 2.0),
        };
        let surface = surface_origin(&output, &window).unwrap();
        assert_eq!(surface, LayoutPt { x: 112.0, y: -13.0 });
        let point = SurfacePt { x: 10.0, y: 10.0 };
        assert_eq!(point.in_layout(surface).on_surface(surface), point);
    }

    #[test]
    fn transform_names_are_the_ones_the_probe_parses() {
        let transforms = [
            Transform::Normal,
            Transform::_90,
            Transform::_180,
            Transform::_270,
            Transform::Flipped,
            Transform::Flipped90,
            Transform::Flipped180,
            Transform::Flipped270,
        ];
        let names: Vec<String> = transforms
            .into_iter()
            .map(|transform| transform_name(transform).unwrap())
            .collect();
        // `Transform::parse` in `probes/vpointer/src/coords.rs` accepts exactly these.
        let probe = [
            "Normal",
            "90",
            "180",
            "270",
            "Flipped",
            "Flipped90",
            "Flipped180",
            "Flipped270",
        ];
        assert_eq!(names, probe);
    }

    #[test]
    fn c12_needs_source_value120_and_value() {
        let frame = [
            "axis_source: 0 (wheel)",
            "axis_value120: axis: 0 (vertical), value120: 120",
            "axis: time: 12000; axis: 0 (vertical), value: 15.000000",
        ]
        .map(str::to_owned);
        assert!(c12_passes(&frame));
        for missing in 0..frame.len() {
            let partial: Vec<String> = frame
                .iter()
                .enumerate()
                .filter(|&(index, _)| index != missing)
                .map(|(_, event)| event.clone())
                .collect();
            assert!(!c12_passes(&partial), "without {missing}");
        }
        let continuous = frame.map(|event| event.replace("0 (wheel)", "2 (continuous)"));
        assert!(!c12_passes(&continuous));
    }
}
