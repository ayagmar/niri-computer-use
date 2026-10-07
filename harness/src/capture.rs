//! grim checks. C3: the orientation of a capture of the flipped nested output. C15: the
//! cost of PNG and JPEG captures at `-s 1` and `-s 0.5`, on the nested output and,
//! read-only, on a host output. Images stay in memory; only their size is kept.

use std::collections::HashMap;
use std::ffi::OsString;
use std::process::Output;
use std::time::{Duration, Instant};

use niri_ipc::{LogicalOutput, WindowLayout};

use crate::failure::{Context as _, Failure, Result};
use crate::image::{self, Rect};
use crate::log::Log;
use crate::run;
use crate::runner::{self, ChildEnv, Group, Invocation, Sink};
use crate::session::Session;

const MAGENTA: [u8; 3] = [0xFF, 0x00, 0xFF];
/// C3's pass rule: each edge of the box within this many pixels.
const C3_TOLERANCE: usize = 2;
const RUNS: usize = 5;
const HOST_DEADLINE: Duration = Duration::from_secs(10);

/// C3: a capture at the output's own scale shows `window` where niri placed it, so the
/// image is in displayed orientation.
pub(crate) fn c3(
    session: &mut Session<'_>,
    output: &LogicalOutput,
    window: &WindowLayout,
) -> Result<()> {
    let args = ["-t", "ppm", "-o", "winit", "-"].map(OsString::from);
    let ppm = session.run("grim", &args)?.stdout;
    session.screenshot("success-c3.png")?;
    let image = image::ppm(&ppm).ok_or_else(|| Failure::new("grim wrote an unreadable PPM"))?;
    let expected = window_in_image(output, window)?;
    let found = image::bounding_box(&image, MAGENTA);
    let pass = found.is_some_and(|found| close(found, expected));
    session.log(&format!(
        "C3: {}x{} image, non-magenta box {}, expected {} ±{C3_TOLERANCE}: {}",
        image.width,
        image.height,
        found.map_or_else(|| "none".to_owned(), describe),
        describe(expected),
        verdict(pass),
    ))?;
    if pass {
        Ok(())
    } else {
        Err(Failure::new("C3: wev is not where niri placed it"))
    }
}

/// The window's rectangle in an image of the whole output at the output's scale.
fn window_in_image(output: &LogicalOutput, window: &WindowLayout) -> Result<Rect> {
    let (tile_x, tile_y) = window
        .tile_pos_in_workspace_view
        .ok_or_else(|| Failure::new("wev has no position in the workspace view"))?;
    let (offset_x, offset_y) = window.window_offset_in_tile;
    let (width, height) = window.window_size;
    let pixels = |logical: f64| to_pixels(logical * output.scale);
    Ok(Rect {
        x: pixels(tile_x + offset_x)?,
        y: pixels(tile_y + offset_y)?,
        width: pixels(f64::from(width))?,
        height: pixels(f64::from(height))?,
    })
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "checked to be a non-negative whole number that fits first"
)]
fn to_pixels(value: f64) -> Result<usize> {
    let rounded = value.round();
    if (0.0..=f64::from(u32::MAX)).contains(&rounded) {
        Ok(rounded as usize)
    } else {
        Err(Failure::new(format!("{value} is not a pixel position")))
    }
}

fn close(found: Rect, expected: Rect) -> bool {
    [
        (found.x, expected.x),
        (found.y, expected.y),
        (found.width, expected.width),
        (found.height, expected.height),
    ]
    .into_iter()
    .all(|(a, b)| a.abs_diff(b) <= C3_TOLERANCE)
}

fn describe(rect: Rect) -> String {
    format!("{}x{} at ({}, {})", rect.width, rect.height, rect.x, rect.y)
}

const fn verdict(pass: bool) -> &'static str {
    if pass { "pass" } else { "fail" }
}

#[derive(Debug, Clone, Copy)]
struct Format {
    name: &'static str,
    args: &'static [&'static str],
    size: fn(&[u8]) -> Option<(u32, u32)>,
}

const FORMATS: [Format; 2] = [
    Format {
        name: "png",
        args: &["-t", "png"],
        size: image::png_size,
    },
    Format {
        name: "jpeg",
        args: &["-t", "jpeg", "-q", "80"],
        size: image::jpeg_size,
    },
];

const SCALES: [(&str, f64); 2] = [("1", 1.0), ("0.5", 0.5)];

#[derive(Debug)]
struct Sample {
    elapsed: Duration,
    bytes: u32,
    size: Option<(u32, u32)>,
}

/// One format at one scale, and its samples.
#[derive(Debug)]
struct Case {
    format: Format,
    scale: &'static str,
    factor: f64,
    samples: Vec<Sample>,
}

impl Case {
    fn all() -> Vec<Self> {
        FORMATS
            .into_iter()
            .flat_map(|format| {
                SCALES.map(|(scale, factor)| Self {
                    format,
                    scale,
                    factor,
                    samples: Vec::new(),
                })
            })
            .collect()
    }

    fn args(&self, output: &str) -> Vec<OsString> {
        let mut args = vec!["-o", output, "-s", self.scale];
        args.extend(self.format.args);
        args.push("-");
        args.into_iter().map(OsString::from).collect()
    }

    fn record(&mut self, elapsed: Duration, image: &[u8]) -> Result<()> {
        self.samples.push(Sample {
            elapsed,
            bytes: u32::try_from(image.len()).context("image length")?,
            size: (self.format.size)(image),
        });
        Ok(())
    }

    /// One line per capture, in the order taken.
    fn sample_lines(&self, output: &str) -> impl Iterator<Item = String> {
        self.samples.iter().zip(1..).map(move |(sample, run)| {
            format!(
                "C15 {output} {} -s {} run {run}: {:.1} ms, {} bytes, {}",
                self.format.name,
                self.scale,
                millis(sample),
                sample.bytes,
                describe_size(sample.size),
            )
        })
    }

    /// One line for the log, and whether every image had the `expected` size.
    fn report(&self, output: &str, expected: (u32, u32)) -> (String, bool) {
        let pass = self
            .samples
            .iter()
            .all(|sample| sample.size == Some(expected));
        let sizes: Vec<String> = self
            .samples
            .iter()
            .map(|sample| describe_size(sample.size))
            .collect();
        let (time, bytes) = (self.spread(millis), self.spread(bytes));
        let line = format!(
            "C15 {output} {} -s {}: sizes {} (expected {}x{}), ms min/median/max \
             {:.0}/{:.0}/{:.0}, bytes min/median/max {:.0}/{:.0}/{:.0}: {}",
            self.format.name,
            self.scale,
            sizes.join(" "),
            expected.0,
            expected.1,
            time.0,
            time.1,
            time.2,
            bytes.0,
            bytes.1,
            bytes.2,
            verdict(pass),
        );
        (line, pass)
    }

    /// Minimum, median and maximum.
    fn spread(&self, of: fn(&Sample) -> f64) -> (f64, f64, f64) {
        spread(self.samples.iter().map(of).collect())
    }
}

fn describe_size(size: Option<(u32, u32)>) -> String {
    size.map_or_else(|| "unreadable".to_owned(), |(w, h)| format!("{w}x{h}"))
}

fn millis(sample: &Sample) -> f64 {
    sample.elapsed.as_secs_f64() * 1000.0
}

fn bytes(sample: &Sample) -> f64 {
    f64::from(sample.bytes)
}

fn spread(mut values: Vec<f64>) -> (f64, f64, f64) {
    values.sort_by(f64::total_cmp);
    let at = |index: usize| values.get(index).copied().unwrap_or(f64::NAN);
    (
        at(0),
        at(values.len() / 2),
        at(values.len().saturating_sub(1)),
    )
}

/// C15 on the nested output.
pub(crate) fn nested_c15(session: &mut Session<'_>, output: &LogicalOutput) -> Result<()> {
    let lines = c15("winit", output, |args| session.run("grim", args))?;
    lines.iter().try_for_each(|line| session.log(line))
}

/// `harness host-capture <output>`: C15 on a host output. Only reads: the host outputs
/// from `niri msg`, and grim captures that never leave memory.
pub(crate) fn host(name: &str) -> Result<()> {
    let artifacts = run::create_artifacts(&run::stamp()?)?;
    let mut log = Log::create(&artifacts.join("harness.log"), true)?;
    let outputs = host_run(
        "niri",
        ["msg", "--json", "outputs"].map(OsString::from).to_vec(),
    )?;
    let mut outputs: HashMap<String, niri_ipc::Output> =
        serde_json::from_slice(&outputs.stdout).context("parse niri msg --json outputs")?;
    let logical = outputs
        .remove(name)
        .and_then(|output| output.logical)
        .ok_or_else(|| Failure::new(format!("the host has no active output {name}")))?;
    let lines = c15(name, &logical, |args| host_run("grim", args.to_vec()))?;
    lines.iter().try_for_each(|line| log.line(line))
}

fn host_run(program: &str, args: Vec<OsString>) -> Result<Output> {
    runner::run(&Invocation {
        program,
        args,
        env: ChildEnv::Inherit,
        output: Sink::Capture,
        group: Group::Own,
        deadline: HOST_DEADLINE,
    })
}

/// Captures `name` five times per format and scale, interleaved, and reports each case.
/// Fails if any image has a size other than round(logical size × scale).
fn c15(
    name: &str,
    output: &LogicalOutput,
    mut grim: impl FnMut(&[OsString]) -> Result<Output>,
) -> Result<Vec<String>> {
    let mut cases = Case::all();
    for _ in 0..RUNS {
        for case in &mut cases {
            let started = Instant::now();
            let image = grim(&case.args(name))?.stdout;
            case.record(started.elapsed(), &image)?;
        }
    }
    let mut lines = Vec::new();
    let mut failed = false;
    for case in &cases {
        let expected = (
            scaled(output.width, case.factor)?,
            scaled(output.height, case.factor)?,
        );
        let (line, pass) = case.report(name, expected);
        failed |= !pass;
        lines.extend(case.sample_lines(name));
        lines.push(line);
    }
    lines.extend(ratios(name, &cases));
    if failed {
        return Err(Failure::new(format!(
            "C15: a capture of {name} had the wrong size: {}",
            lines.join("; ")
        )));
    }
    Ok(lines)
}

fn scaled(logical: u32, factor: f64) -> Result<u32> {
    let pixels = to_pixels(f64::from(logical) * factor)?;
    u32::try_from(pixels).context("image size")
}

/// JPEG's median time and size as a fraction of PNG's, at each scale.
fn ratios(name: &str, cases: &[Case]) -> Vec<String> {
    let median = |format: &str, scale: &str, of: fn(&Sample) -> f64| {
        cases
            .iter()
            .find(|case| case.format.name == format && case.scale == scale)
            .map(|case| case.spread(of).1)
    };
    SCALES
        .iter()
        .filter_map(|&(scale, _)| {
            let time = median("jpeg", scale, millis)? / median("png", scale, millis)?;
            let size = median("jpeg", scale, bytes)? / median("png", scale, bytes)?;
            Some(format!(
                "C15 {name} -s {scale}: JPEG takes {time:.2}x the time and {size:.2}x the \
                 bytes of PNG"
            ))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::process::ExitStatus;

    use niri_ipc::Transform;

    use super::*;

    fn output(width: u32, height: u32, scale: f64) -> LogicalOutput {
        LogicalOutput {
            x: 0,
            y: 0,
            width,
            height,
            scale,
            transform: Transform::Flipped180,
        }
    }

    fn wev(tile: (f64, f64)) -> WindowLayout {
        WindowLayout {
            pos_in_scrolling_layout: None,
            tile_size: (400.0, 300.0),
            window_size: (400, 300),
            tile_pos_in_workspace_view: Some(tile),
            window_offset_in_tile: (0.0, 0.0),
        }
    }

    #[test]
    fn the_window_is_scaled_into_the_image() {
        let at_one = window_in_image(&output(960, 720, 1.0), &wev((0.0, 0.0))).unwrap();
        assert_eq!(describe(at_one), "400x300 at (0, 0)");
        let at_one_and_a_half = window_in_image(&output(640, 480, 1.5), &wev((10.0, 0.0))).unwrap();
        assert_eq!(describe(at_one_and_a_half), "600x450 at (15, 0)");
    }

    #[test]
    fn c3_allows_two_pixels_per_edge() {
        let expected = Rect {
            x: 0,
            y: 0,
            width: 400,
            height: 300,
        };
        assert!(close(
            Rect {
                x: 2,
                width: 398,
                ..expected
            },
            expected
        ));
        let flipped = Rect { y: 420, ..expected };
        assert!(!close(flipped, expected));
    }

    /// A PNG or JPEG header for what grim would write with `args`, from a 480x360 output,
    /// except that JPEGs are always `jpeg_size`.
    fn header(args: &[OsString], jpeg_size: Option<(u16, u16)>) -> Vec<u8> {
        let half = args.iter().any(|arg| arg == "0.5");
        let (width, height): (u16, u16) = if half { (240, 180) } else { (480, 360) };
        if args.iter().any(|arg| arg == "png") {
            let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
            png.extend_from_slice(&u32::from(width).to_be_bytes());
            png.extend_from_slice(&u32::from(height).to_be_bytes());
            return png;
        }
        let (width, height) = jpeg_size.unwrap_or((width, height));
        let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xC0, 0x00, 0x08, 0x08];
        jpeg.extend_from_slice(&height.to_be_bytes());
        jpeg.extend_from_slice(&width.to_be_bytes());
        jpeg.push(0x01);
        jpeg
    }

    fn written(stdout: Vec<u8>) -> Output {
        Output {
            status: ExitStatus::default(),
            stdout,
            stderr: Vec::new(),
        }
    }

    #[test]
    fn c15_takes_five_interleaved_captures_per_case_and_logs_each() {
        let mut calls: Vec<String> = Vec::new();
        let lines = c15("o", &output(480, 360, 1.0), |args| {
            let call: Vec<_> = args.iter().map(|arg| arg.to_string_lossy()).collect();
            calls.push(call.join(" "));
            Ok(written(header(args, None)))
        })
        .unwrap();
        assert_eq!(calls.len(), 20);
        assert_eq!(
            calls.get(..4).unwrap(),
            [
                "-o o -s 1 -t png -",
                "-o o -s 0.5 -t png -",
                "-o o -s 1 -t jpeg -q 80 -",
                "-o o -s 0.5 -t jpeg -q 80 -",
            ]
        );
        let runs = lines.iter().filter(|line| line.contains(" run ")).count();
        assert_eq!(runs, 20);
        assert!(
            lines
                .iter()
                .any(|line| line.starts_with("C15 o jpeg -s 0.5 run 5: "))
        );
        assert!(lines.iter().any(|line| line.contains("JPEG takes")));
    }

    #[test]
    fn c15_checks_every_capture_against_the_scaled_size() {
        let lines = c15("o", &output(480, 360, 1.0), |args| {
            Ok(written(header(args, Some((240, 180)))))
        });
        let message = lines.unwrap_err().to_string();
        assert!(message.contains("png -s 1: sizes 480x360"), "{message}");
        assert!(message.contains("jpeg -s 1: sizes 240x180"), "{message}");
        assert!(
            message.contains(
                "jpeg -s 1: sizes 240x180 240x180 240x180 240x180 240x180 (expected 480x360)"
            ),
            "{message}"
        );
    }

    #[test]
    fn spread_is_min_median_max() {
        assert_eq!(spread(vec![5.0, 1.0, 3.0, 2.0, 4.0]), (1.0, 3.0, 5.0));
    }
}
