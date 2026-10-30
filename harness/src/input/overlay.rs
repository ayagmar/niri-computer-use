//! The Noctalia plugin's robot overlay (`--overlay`), on `--multi`'s two outputs with the
//! first at `--scale`. The nested Noctalia loads this checkout's plugin, whose service runs
//! the server's `watch`, and one server holds the lease. The robot must sit `GAP` above-left
//! of a click on each output, a click under it must reach the window beneath, and `click`
//! with `screenshot: true` must settle about as fast with the overlay as without it, which
//! shows only that the overlay doesn't keep a settle from ending: the layer draws too
//! seldom here to show whether the robot holds still while the agent looks. How
//! smoothly it glides isn't checked here: niri's headless backend sends frame callbacks
//! only from its fallback timer, about once a second, so the layer draws a glide's last
//! frame at most and only a real display shows the frames between. The robot is found by its antenna's
//! colour, which nothing else on the nested desktop has.

use std::ffi::OsString;
use std::time::{Duration, Instant};

use niri_ipc::{LogicalOutput, Request, Response};
use serde_json::{Value, json};

use super::{READY, SERVER_DEADLINE, Shot, WAIT, send, surface_origin};
use crate::failure::{Failure, Result};
use crate::image::{self, Rgb};
use crate::mcp::{self, Client, field, structured};
use crate::scale::Scale;
use crate::session::Session;

pub(crate) const PLUGIN_ID: &str = "ayagmar/niri-computer-use";
/// Noctalia names a plugin's layer `noctalia-plugin:<plugin>:<layer>`.
const NAMESPACE: &str = "noctalia-plugin:ayagmar/niri-computer-use:robot";
/// The oldest Noctalia with `[[layer]]` entries, plugin API 36.
const NOCTALIA: (u32, u32) = (5, 4);
const NOCTALIA_DEADLINE: Duration = Duration::from_secs(190);
/// The antenna tip's fill in `robot/*.svg`, and its centre in the 32x32 frame.
const ANTENNA: [u8; 3] = [0x4c, 0x8d, 0xff];
const TIP: (f64, f64) = (16.0, 3.2);
/// `lib/stage.luau`'s sizes, in logical pixels at a UI scale of 1.
const SPRITE: f64 = 32.0;
const GAP: f64 = 12.0;
/// How far the antenna may be from where it should be, in logical pixels: its circle
/// rasterized at fractional scales, and the robot's position rounded to whole pixels.
const PLACED: f64 = 2.5;
/// How long the robot may take to arrive beside a click, from the click's reply: the
/// headless backend's frame callbacks come about once a second, and on another output
/// Noctalia makes the layer's surface again first.
const ARRIVAL: Duration = Duration::from_secs(3);
const SETTLE_CLICKS: usize = 8;
/// What a median `click … screenshot: true` may take longer with the overlay than without:
/// one more of the settle wait's 100 ms samples, and some noise.
const SETTLE_SLACK: Duration = Duration::from_millis(150);

/// The outputs `--multi` uses, with the first at `scale`.
pub(crate) fn output_config(scale: Scale) -> String {
    format!(
        r#"
output "headless-1" {{
    scale {scale}
    position x=0 y=0
}}

output "headless-2" {{
    scale 1.5
    position x=-1280 y=100
}}
"#
    )
}

pub(crate) fn run(session: &mut Session<'_>, server: &str, scale: Scale) -> Result<()> {
    noctalia_version(session)?;
    let noctalia = session.start(
        "noctalia",
        &[],
        session.artifact("noctalia.log"),
        NOCTALIA_DEADLINE,
    )?;
    let mut client = Client::start(session, server, "harness-overlay", SERVER_DEADLINE)?;
    mcp::ready(session, &mut client, "overlay-ready", READY)?;
    structured(&client.call(session, "acquire_desktop", json!({}))?)?;
    let shown = shown_on(session, "headless-1")?;
    session.log(&format!(
        "overlay layer: {NAMESPACE} on {shown} (UI scale 1, output scale {scale})"
    ))?;
    for name in ["headless-1", "headless-2"] {
        beside_click(session, &mut client, name, 0.6, above_left)?;
    }
    // Too near the left edge for a caption on the left, the robot goes above-right, and
    // the layer, which keeps room for a caption on both sides, starts off the output.
    beside_click(session, &mut client, "headless-1", 0.05, above_right)?;
    clicks_pass_through(session, &mut client)?;
    settles(session, &mut client)?;
    structured(&client.call(session, "release_desktop", json!({"restore_focus": false}))?)?;
    client.stop()?;
    noctalia.stop().map(drop)
}

/// Refuses a Noctalia older than 5.4, which refuses the plugin.
fn noctalia_version(session: &mut Session<'_>) -> Result<()> {
    let output = session.run("noctalia", &[OsString::from("--version")])?;
    let text = String::from_utf8_lossy(&output.stdout);
    // Such as `noctalia v5.4.0`.
    let version = text.split_whitespace().nth(1).unwrap_or_default();
    let version = version.strip_prefix('v').unwrap_or(version);
    let mut parts = version.split(['.', '-']).map(str::parse::<u32>);
    let found = match (parts.next(), parts.next()) {
        (Some(Ok(major)), Some(Ok(minor))) => (major, minor),
        _ => {
            return Err(Failure::new(format!(
                "overlay: can't read Noctalia's version from {text:?}"
            )));
        }
    };
    if found < NOCTALIA {
        return Err(Failure::new(format!(
            "overlay: Noctalia {version} is older than 5.4, which the plugin needs"
        )));
    }
    session.log(&format!("overlay: Noctalia {version}"))
}

/// Waits until niri shows the robot's layer on `output`, and returns the output.
fn shown_on(session: &mut Session<'_>, output: &str) -> Result<String> {
    session.wait_until("overlay-layer", "the robot's layer", WAIT * 2, |session| {
        let Response::Layers(layers) = session.request(&Request::Layers)? else {
            return Err(Failure::new("niri answered Layers with another response"));
        };
        Ok(layers
            .into_iter()
            .find(|layer| layer.namespace == NAMESPACE && layer.output == output)
            .map(|layer| layer.output))
    })
}

fn logical_output(session: &mut Session<'_>, name: &str) -> Result<LogicalOutput> {
    let Response::Outputs(outputs) = session.request(&Request::Outputs)? else {
        return Err(Failure::new("niri answered Outputs with another response"));
    };
    outputs
        .get(name)
        .and_then(|output| output.logical)
        .ok_or_else(|| Failure::new(format!("overlay: niri has no {name}")))
}

/// A capture of `output` at its own scale.
fn capture(session: &Session<'_>, output: &str) -> Result<Vec<u8>> {
    let args = ["-t", "ppm", "-o", output, "-"].map(OsString::from);
    Ok(session.run("grim", &args)?.stdout)
}

/// Where the antenna's centre is in a capture of `output`, in layout coordinates.
fn antenna(bytes: &[u8], output: &LogicalOutput) -> Result<Option<(f64, f64)>> {
    let image: Rgb<'_> =
        image::ppm(bytes).ok_or_else(|| Failure::new("overlay: grim's capture isn't a PPM"))?;
    Ok(image::find(&image, ANTENNA).map(|rect| {
        let centre = |start: usize, length: usize| as_f64(start) + as_f64(length) / 2.0;
        (
            f64::from(output.x) + centre(rect.x, rect.width) / output.scale,
            f64::from(output.y) + centre(rect.y, rect.height) / output.scale,
        )
    }))
}

#[expect(
    clippy::cast_precision_loss,
    reason = "pixel positions in a capture are far below 2^52"
)]
const fn as_f64(value: usize) -> f64 {
    value as f64
}

/// Where the antenna should be with the robot above-left of `point`.
fn above_left(point: (f64, f64)) -> (f64, f64) {
    (
        point.0 - GAP - SPRITE + TIP.0,
        point.1 - GAP - SPRITE + TIP.1,
    )
}

/// Where the antenna should be with the robot above-right of `point`.
fn above_right(point: (f64, f64)) -> (f64, f64) {
    (point.0 + GAP + TIP.0, point.1 - GAP - SPRITE + TIP.1)
}

fn near(found: (f64, f64), expected: (f64, f64)) -> bool {
    (found.0 - expected.0).abs() <= PLACED && (found.1 - expected.1).abs() <= PLACED
}

/// Clicks `name` `across` its width and 0.6 down, and requires the robot where `place`
/// says, `GAP` from the click.
fn beside_click(
    session: &mut Session<'_>,
    client: &mut Client,
    name: &str,
    across: f64,
    place: fn((f64, f64)) -> (f64, f64),
) -> Result<()> {
    let output = logical_output(session, name)?;
    let point = (
        f64::from(output.width).mul_add(across, f64::from(output.x)),
        f64::from(output.height).mul_add(0.6, f64::from(output.y)),
    );
    let shot = Shot::through(session, client, &format!("output:{name}"), 4000)?;
    let pixel = shot.pixel(point);
    let aim = shot.centre(pixel);
    send(
        session,
        client,
        "click",
        json!({"screenshot_ref": shot.id, "x": pixel.0, "y": pixel.1}),
    )?;
    let expected = place(aim);
    let (found, after) = arrive(session, name, &output, expected)?;
    session.log(&format!(
        "overlay placement on {name}: antenna at {found:?} {after:?} after the click's reply, expected {expected:.1?} for a click at {aim:.1?}"
    ))?;
    match found {
        Some(found) if near(found, expected) => Ok(()),
        _ => Err(Failure::new(format!(
            "overlay: the robot isn't {GAP} px beside the click on {name}"
        ))),
    }
}

/// Where the robot is once a capture of `name` shows it near `expected`, and how long that
/// took, or where it was last seen after `ARRIVAL`. The last capture is kept in the
/// artifacts.
fn arrive(
    session: &Session<'_>,
    name: &str,
    output: &LogicalOutput,
    expected: (f64, f64),
) -> Result<(Option<(f64, f64)>, Duration)> {
    let started = Instant::now();
    loop {
        let captured = capture(session, name)?;
        let found = antenna(&captured, output)?;
        let at = started.elapsed();
        if found.is_some_and(|found| near(found, expected)) || at >= ARRIVAL {
            std::fs::write(session.artifact(&format!("overlay-{name}.ppm")), &captured)
                .map_err(|error| Failure::new(format!("overlay: save the capture: {error}")))?;
            return Ok((found, at));
        }
    }
}

/// The GTK button under the robot must count a click aimed at the robot.
fn clicks_pass_through(session: &mut Session<'_>, client: &mut Client) -> Result<()> {
    let check = "overlay click-through";
    let Some(process) = super::activation::start_fixture(session, "button", check)? else {
        return Ok(());
    };
    let output = logical_output(session, "headless-1")?;
    let layout = super::multi::move_to(session, "org.ncu.Activation", "headless-1")?;
    let (x, y) = surface_origin(&output, &layout)?;
    let (width, height) = layout.window_size;
    // Low and right on the button, so the robot lands above-left of it, still on the button.
    let first = (
        f64::from(width).mul_add(0.75, x),
        f64::from(height).mul_add(0.75, y),
    );
    let before = Shot::through(session, client, "output:headless-1", 4000)?;
    let pixel = before.pixel(first);
    send(
        session,
        client,
        "click",
        json!({"screenshot_ref": before.id, "x": pixel.0, "y": pixel.1}),
    )?;
    super::multi::counted(session, 1)?;
    let expected = above_left(before.centre(pixel));
    let tip = match arrive(session, "headless-1", &output, expected)?.0 {
        Some(tip) if near(tip, expected) => tip,
        _ => {
            return Err(Failure::new(
                "overlay: no robot above-left of the click on the button",
            ));
        }
    };
    let robot = (tip.0, tip.1 - TIP.1 + SPRITE / 2.0);
    let after = Shot::through(session, client, "output:headless-1", 4000)?;
    let under = after.pixel(robot);
    send(
        session,
        client,
        "click",
        json!({"screenshot_ref": after.id, "x": under.0, "y": under.1}),
    )?;
    super::multi::counted(session, 2)?;
    session.log(&format!(
        "{check}: a click at the robot's middle {robot:.1?} reached the button beneath"
    ))?;
    process.stop().map(drop)
}

/// `click … screenshot: true` on headless-1 with the overlay, then with the plugin
/// disabled: every call must settle, and the median with the overlay may take at most
/// `SETTLE_SLACK` longer.
fn settles(session: &mut Session<'_>, client: &mut Client) -> Result<()> {
    let with = settle_times(session, client)?;
    let args = ["msg", "plugins", "disable", PLUGIN_ID].map(OsString::from);
    session.run("noctalia", &args)?;
    session.wait_until("overlay-gone", "the robot's layer gone", WAIT, |session| {
        let Response::Layers(layers) = session.request(&Request::Layers)? else {
            return Err(Failure::new("niri answered Layers with another response"));
        };
        Ok((!layers.iter().any(|layer| layer.namespace == NAMESPACE)).then_some(()))
    })?;
    let without = settle_times(session, client)?;
    let (on, off) = (median(&with), median(&without));
    session.log(&format!(
        "overlay settle: median click with a screenshot {on:?} with the overlay, {off:?} without; each {with:?} and {without:?}"
    ))?;
    if on <= off + SETTLE_SLACK {
        Ok(())
    } else {
        Err(Failure::new(
            "overlay: clicks settle slower with the overlay",
        ))
    }
}

fn settle_times(session: &mut Session<'_>, client: &mut Client) -> Result<Vec<Duration>> {
    let output = logical_output(session, "headless-1")?;
    let mut times = Vec::with_capacity(SETTLE_CLICKS);
    for i in 0..SETTLE_CLICKS {
        let column = if i % 2 == 0 { 0.35 } else { 0.65 };
        let point = (
            f64::from(output.width) * column,
            f64::from(output.height) * 0.5,
        );
        let shot = Shot::through(session, client, "output:headless-1", 4000)?;
        let pixel = shot.pixel(point);
        let arguments =
            json!({"screenshot_ref": shot.id, "x": pixel.0, "y": pixel.1, "screenshot": true});
        let (result, took) = client.timed_call("click", arguments)?;
        let outcome = structured(&result)?;
        if !settled(&outcome) {
            return Err(Failure::new(format!(
                "overlay: a click's screenshot didn't settle: {}",
                mcp::report(&result)
            )));
        }
        times.push(took);
    }
    Ok(times)
}

fn settled(outcome: &Value) -> bool {
    field(outcome, "/screenshot/settled") == true
}

fn median(times: &[Duration]) -> Duration {
    let mut sorted = times.to_vec();
    sorted.sort();
    sorted.get(sorted.len() / 2).copied().unwrap_or_default()
}
