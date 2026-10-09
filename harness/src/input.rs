//! M4's nested acceptance for the input tools: one `niri-computer-use` server holds the
//! lease on the nested niri, with the nested Noctalia as the lock source, and sends input
//! to `wev`, which logs what it receives. Every expected position is worked out here from
//! the screenshot's metadata and `wev`'s place in niri's layout, independently of the
//! server's mapping. Everything the run creates lives under `TEST_DIR`.

mod crash;
mod scrolling;
mod stop;

use std::ffi::OsString;
use std::fs;
use std::path::Path;
use std::time::Duration;

use niri_ipc::{LogicalOutput, WindowLayout};
use serde_json::{Value, json};

use crate::failure::{Context as _, Failure, Result};
use crate::keyboard;
use crate::mcp::{self, Client, field, structured};
use crate::session::Session;
use crate::wev::{self, Pointer};

/// Within what is left of the run's deadline.
const NOCTALIA_DEADLINE: Duration = Duration::from_secs(160);
const SERVER_DEADLINE: Duration = Duration::from_secs(150);
const WEV_DEADLINE: Duration = Duration::from_secs(150);
const READY: Duration = Duration::from_secs(20);
const WAIT: Duration = Duration::from_secs(5);
/// C4's pass rule, in logical pixels, including `wev`'s `wl_fixed` rounding.
const TOLERANCE: f64 = 0.05;
/// C4's points on `wev`'s 400x300 surface.
const POINTS: [(f64, f64); 5] = [
    (10.0, 10.0),
    (390.0, 10.0),
    (10.0, 290.0),
    (390.0, 290.0),
    (200.0, 150.0),
];

pub(crate) fn run(session: &mut Session<'_>, output: &LogicalOutput, server: &str) -> Result<()> {
    let noctalia = session.start(
        "noctalia",
        &[],
        session.artifact("noctalia.log"),
        NOCTALIA_DEADLINE,
    )?;
    let mut client = Client::start(session, server, "harness-m4", SERVER_DEADLINE)?;
    let status = mcp::ready(session, &mut client, "m4-ready", READY)?;
    session.log(&format!(
        "M4 status: niri {}, lock {}, outputs {}",
        field(&status, "/niri/compat"),
        field(&status, "/lock"),
        field(&status, "/outputs")
    ))?;
    structured(&client.call(session, "acquire_desktop", json!({}))?)?;
    // Started after Noctalia, whose bar moves floating windows down when it appears.
    let wev_log = session.artifact("wev.log");
    let args = ["-oL", "wev"].map(OsString::from);
    let process = session.start("stdbuf", &args, wev_log.clone(), WEV_DEADLINE)?;
    let window = crate::supervise::wait_for_wev(session)?;
    let wev = Wev {
        log: &wev_log,
        surface: surface_origin(output, &window)?,
    };
    accuracy(session, &mut client, &wev)?;
    clicks(session, &mut client, &wev)?;
    drag(session, &mut client, &wev)?;
    scroll(session, &mut client, &wev)?;
    keys(session, &mut client, &wev)?;
    stop::run(session, &mut client, &wev, server)?;
    structured(&client.call(session, "release_desktop", json!({"restore_focus": false}))?)?;
    crash::run(session, &wev, server)?;
    // wev floats over the tiled kitty, so it goes first.
    process.stop()?;
    scrolling::run(session, &mut client)?;
    client.stop()?;
    noctalia.stop().map(drop)
}

/// `wev`'s log and its surface's top-left corner in the layout.
#[derive(Debug, Clone, Copy)]
struct Wev<'a> {
    log: &'a Path,
    surface: (f64, f64),
}

impl Wev<'_> {
    /// The log's length now: what a call adds comes after it.
    fn offset(&self) -> Result<usize> {
        Ok(self.read()?.len())
    }

    fn read(&self) -> Result<String> {
        fs::read_to_string(self.log).context(format!("read {}", self.log.display()))
    }

    /// The pointer trace logged since `offset`.
    fn since(&self, offset: usize) -> Result<Vec<Pointer>> {
        let log = self.read()?;
        wev::pointer_trace(log.get(offset..).unwrap_or_default())
    }

    /// A surface point in layout coordinates.
    fn in_layout(&self, (x, y): (f64, f64)) -> (f64, f64) {
        (self.surface.0 + x, self.surface.1 + y)
    }

    fn on_surface(&self, (x, y): (f64, f64)) -> (f64, f64) {
        (x - self.surface.0, y - self.surface.1)
    }
}

/// The layout position of `wev`'s surface. Borders are off, so it is the tile position
/// plus a zero offset.
fn surface_origin(output: &LogicalOutput, window: &WindowLayout) -> Result<(f64, f64)> {
    let (tile_x, tile_y) = window
        .tile_pos_in_workspace_view
        .ok_or_else(|| Failure::new("wev has no position in the workspace view"))?;
    let (offset_x, offset_y) = window.window_offset_in_tile;
    Ok((
        f64::from(output.x) + tile_x + offset_x,
        f64::from(output.y) + tile_y + offset_y,
    ))
}

/// A screenshot's ref and what maps its pixels back to the layout.
#[derive(Debug, Clone)]
struct Shot {
    id: String,
    origin: (f64, f64),
    scale: f64,
}

impl Shot {
    fn take(session: &mut Session<'_>, client: &mut Client, max_width: u32) -> Result<Self> {
        let arguments = json!({"target": "focused_output", "max_width": max_width});
        let metadata = structured(&client.call(session, "screenshot", arguments)?)?;
        let number = |pointer| {
            field(&metadata, pointer)
                .as_f64()
                .ok_or_else(|| Failure::new(format!("M4: no {pointer} in {metadata}")))
        };
        let id = field(&metadata, "/screenshot_ref")
            .as_str()
            .ok_or_else(|| Failure::new(format!("M4: no screenshot_ref in {metadata}")))?
            .to_owned();
        Ok(Self {
            origin: (number("/captured/x")?, number("/captured/y")?),
            scale: number("/scale")?,
            id,
        })
    }

    /// The image pixel that holds a layout point.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the points are inside the captured output, so the pixels are small and positive"
    )]
    fn pixel(&self, (x, y): (f64, f64)) -> (u32, u32) {
        (
            ((x - self.origin.0) * self.scale).floor() as u32,
            ((y - self.origin.1) * self.scale).floor() as u32,
        )
    }

    /// Where the server should put the pointer for a pixel: its centre.
    fn centre(&self, (px, py): (u32, u32)) -> (f64, f64) {
        (
            self.origin.0 + (f64::from(px) + 0.5) / self.scale,
            self.origin.1 + (f64::from(py) + 0.5) / self.scale,
        )
    }
}

fn distance(a: (f64, f64), b: (f64, f64)) -> f64 {
    (a.0 - b.0).abs().max((a.1 - b.1).abs())
}

/// Calls a pointer tool and requires `observed: sent`.
fn send(
    session: &mut Session<'_>,
    client: &mut Client,
    tool: &str,
    arguments: Value,
) -> Result<Value> {
    let outcome = structured(&client.call(session, tool, arguments)?)?;
    if field(&outcome, "/observed") != "sent" {
        return Err(Failure::new(format!("M4: {tool} wasn't sent: {outcome}")));
    }
    Ok(outcome)
}

/// C4's points through `pointer_move`, from a screenshot at the output's own scale and
/// from one at a lowered scale, must each land within ±0.05 px of the pixel's centre.
fn accuracy(session: &mut Session<'_>, client: &mut Client, wev: &Wev<'_>) -> Result<()> {
    let mut failed = 0;
    for max_width in [4000, 700] {
        let shot = Shot::take(session, client, max_width)?;
        session.log(&format!(
            "M4 accuracy: {} at capture scale {}",
            shot.id, shot.scale
        ))?;
        for point in POINTS {
            let pixel = shot.pixel(wev.in_layout(point));
            let expected = wev.on_surface(shot.centre(pixel));
            let offset = wev.offset()?;
            let arguments = json!({"screenshot_ref": shot.id, "x": pixel.0, "y": pixel.1});
            send(session, client, "pointer_move", arguments)?;
            let seen = session.wait_until("m4-accuracy", "the motion in wev", WAIT, |_| {
                Ok(wev
                    .since(offset)?
                    .into_iter()
                    .find_map(|event| match event {
                        Pointer::At(x, y) => Some((x, y)),
                        Pointer::Button { .. } => None,
                    }))
            })?;
            let off = distance(seen, expected);
            let pass = off <= TOLERANCE;
            failed += usize::from(!pass);
            session.log(&format!(
                "M4 accuracy: pixel {pixel:?} -> ({:.4}, {:.4}); wev ({:.6}, {:.6}), off by {off:.4}: {}",
                expected.0,
                expected.1,
                seen.0,
                seen.1,
                if pass { "pass" } else { "fail" }
            ))?;
        }
    }
    if failed > 0 {
        return Err(Failure::new(format!(
            "M4 accuracy: {failed} points landed outside ±{TOLERANCE} px"
        )));
    }
    Ok(())
}

/// Waits until `wev` has logged `count` button events since `offset`, and returns them.
fn buttons(
    session: &mut Session<'_>,
    wev: &Wev<'_>,
    offset: usize,
    count: usize,
) -> Result<Vec<(u32, bool)>> {
    session.wait_until("m4-buttons", "the buttons in wev", WAIT, |_| {
        let pressed: Vec<(u32, bool)> = wev
            .since(offset)?
            .into_iter()
            .filter_map(|event| match event {
                Pointer::Button { code, pressed } => Some((code, pressed)),
                Pointer::At(..) => None,
            })
            .collect();
        Ok((pressed.len() >= count).then_some(pressed))
    })
}

/// A left click, then a right double click, each a press and a release per click.
fn clicks(session: &mut Session<'_>, client: &mut Client, wev: &Wev<'_>) -> Result<()> {
    let shot = Shot::take(session, client, 4000)?;
    let pixel = shot.pixel(wev.in_layout((200.0, 150.0)));
    for (button, code, count) in [("left", 272, 1), ("right", 273, 2)] {
        let offset = wev.offset()?;
        let arguments = json!({
            "screenshot_ref": shot.id, "x": pixel.0, "y": pixel.1,
            "button": button, "count": count
        });
        send(session, client, "click", arguments)?;
        let seen = buttons(session, wev, offset, 2 * count)?;
        let expected: Vec<(u32, bool)> = (0..count)
            .flat_map(|_| [(code, true), (code, false)])
            .collect();
        session.log(&format!("M4 click {button} x{count}: wev buttons {seen:?}"))?;
        if seen != expected {
            return Err(Failure::new(format!(
                "M4 click: expected {expected:?}, saw {seen:?}"
            )));
        }
    }
    // Each click wrote the input-dirty marker and removed it after its release.
    stop::marker_gone(session)
}

/// A drag presses at its start, moves, and releases at its end.
fn drag(session: &mut Session<'_>, client: &mut Client, wev: &Wev<'_>) -> Result<()> {
    let shot = Shot::take(session, client, 4000)?;
    let from = shot.pixel(wev.in_layout((50.0, 50.0)));
    let to = shot.pixel(wev.in_layout((350.0, 250.0)));
    let offset = wev.offset()?;
    let arguments = json!({
        "screenshot_ref": shot.id,
        "from": {"x": from.0, "y": from.1},
        "to": {"x": to.0, "y": to.1}
    });
    send(session, client, "drag", arguments)?;
    buttons(session, wev, offset, 2)?;
    let trace = wev.since(offset)?;
    let (pressed_at, released_at, moves) = drag_points(&trace)?;
    let start = wev.on_surface(shot.centre(from));
    let end = wev.on_surface(shot.centre(to));
    session.log(&format!(
        "M4 drag: pressed at {pressed_at:?}, released at {released_at:?}, {moves} motions while held"
    ))?;
    if distance(pressed_at, start) > TOLERANCE || distance(released_at, end) > TOLERANCE {
        return Err(Failure::new(format!(
            "M4 drag: expected a press at {start:?} and a release at {end:?}"
        )));
    }
    if moves < 2 {
        return Err(Failure::new("M4 drag: the pointer didn't move while held"));
    }
    stop::marker_gone(session)
}

/// A surface-local point.
type At = (f64, f64);

/// Where the left button was pressed and released, and how many motions came between.
fn drag_points(trace: &[Pointer]) -> Result<(At, At, usize)> {
    let mut at = None;
    let mut pressed = None;
    let mut moves = 0;
    for event in trace {
        match *event {
            Pointer::At(x, y) => {
                at = Some((x, y));
                moves += usize::from(pressed.is_some());
            }
            Pointer::Button {
                code: 272,
                pressed: true,
            } => pressed = at,
            Pointer::Button {
                code: 272,
                pressed: false,
            } => {
                let start = pressed.ok_or_else(|| Failure::new("M4 drag: release before press"))?;
                let end = at.ok_or_else(|| Failure::new("M4 drag: no position"))?;
                return Ok((start, end, moves));
            }
            Pointer::Button { code, .. } => {
                return Err(Failure::new(format!("M4 drag: unexpected button {code}")));
            }
        }
    }
    Err(Failure::new("M4 drag: no press and release in wev"))
}

/// Two notches down, then one to the left: each one wheel frame with niri's 15 per notch.
fn scroll(session: &mut Session<'_>, client: &mut Client, wev: &Wev<'_>) -> Result<()> {
    let shot = Shot::take(session, client, 4000)?;
    let pixel = shot.pixel(wev.in_layout((200.0, 150.0)));
    let cases = [
        ("notches_y", 2, "0 (vertical)", 240, "30.000000"),
        ("notches_x", -1, "1 (horizontal)", -120, "-15.000000"),
    ];
    for (key, notches, axis, value120, value) in cases {
        let offset = wev.offset()?;
        let mut arguments = json!({"screenshot_ref": shot.id, "x": pixel.0, "y": pixel.1});
        if let Some(fields) = arguments.as_object_mut() {
            fields.insert(key.to_owned(), json!(notches));
        }
        send(session, client, "scroll", arguments)?;
        let frame = session.wait_until("m4-scroll", "a wheel frame in wev", WAIT, |_| {
            let log = wev.read()?;
            let frames = wev::axis_frames(log.get(offset..).unwrap_or_default());
            Ok(frames.first().map(|frame| {
                frame
                    .iter()
                    .map(|event| format!("{}: {}", event.name, event.detail))
                    .collect::<Vec<String>>()
            }))
        })?;
        session.log(&format!("M4 scroll {key} {notches}: wev frame {frame:?}"))?;
        if !wheel_frame(&frame, axis, value120, value) {
            return Err(Failure::new(format!(
                "M4 scroll: expected a wheel frame on {axis} with value120 {value120} and value {value}"
            )));
        }
    }
    Ok(())
}

/// Text and a chord into `wev` through the keyboard tools, a refused `expect` that types
/// nothing, and key routing: Ctrl+Shift+F12 reaches `wev` while niri's bind for it, which
/// touches `bind-fired`, doesn't fire.
fn keys(session: &mut Session<'_>, client: &mut Client, wev: &Wev<'_>) -> Result<()> {
    let id = wev_window(session, client)?;
    type_into_wev(session, client, wev)?;
    mismatch(session, client, wev, id)?;
    routing(session, client, wev, id)
}

fn type_into_wev(session: &mut Session<'_>, client: &mut Client, wev: &Wev<'_>) -> Result<()> {
    let text = "Hello, wörld →";
    let start = wev.offset()?;
    let typed = send_keys(
        session,
        client,
        "type_text",
        json!({"text": text, "expect": {"app_id": "wev"}}),
    )?;
    let seen = keyboard::observed(session, wev.log, start, text.chars().count(), false)?;
    keyboard::text(&wev::keyboard::trace(&seen)?, text)?;
    session.log(&format!("M4 type_text: wev decoded {text:?}; {typed}"))
}

fn mismatch(session: &mut Session<'_>, client: &mut Client, wev: &Wev<'_>, id: u64) -> Result<()> {
    let refused = client.call(
        session,
        "type_text",
        json!({"text": "x", "expect": {"window_id": id + 1000}}),
    )?;
    if field(&refused, "/structuredContent/error") != "focus_mismatch" {
        return Err(Failure::new(format!(
            "M4: expected focus_mismatch, saw {refused}"
        )));
    }
    let after = wev.offset()?;
    session.still_absent("m4-mismatch", Duration::from_secs(1), || {
        Ok(wev::keyboard::has_input(&keyboard::since(wev.log, after)?))
    })?;
    session.log("M4 focus_mismatch: refused, nothing typed for 1 s")
}

fn routing(session: &mut Session<'_>, client: &mut Client, wev: &Wev<'_>, id: u64) -> Result<()> {
    let marker = session.bind_marker();
    if marker.try_exists().context("check bind-fired")? {
        return Err(Failure::new("M4: bind-fired existed before the key"));
    }
    let start = wev.offset()?;
    let routed = send_keys(
        session,
        client,
        "key",
        json!({"keys": ["ctrl+shift+F12"], "expect": {"window_id": id}}),
    )?;
    let seen = keyboard::observed(session, wev.log, start, 1, true)?;
    keyboard::chord(&wev::keyboard::trace(&seen)?, "F12", 5, 1)?;
    session.still_absent("m4-routing", Duration::from_secs(1), || {
        marker.try_exists().context("check bind-fired")
    })?;
    session.log(&format!(
        "M4 key routing: one F12 with Control+Shift in wev, niri's bind didn't fire; {routed}"
    ))
}

/// Calls a keyboard tool and requires `observed: sent` with `focus: matched`.
fn send_keys(
    session: &mut Session<'_>,
    client: &mut Client,
    tool: &str,
    arguments: Value,
) -> Result<Value> {
    let outcome = send(session, client, tool, arguments)?;
    if field(&outcome, "/focus") != "matched" {
        return Err(Failure::new(format!(
            "M4: {tool} didn't match focus: {outcome}"
        )));
    }
    Ok(outcome)
}

/// `wev`'s window id, once it has keyboard focus.
fn wev_window(session: &mut Session<'_>, client: &mut Client) -> Result<u64> {
    session.wait_until("m4-focus", "wev focused", WAIT, |session| {
        let desktop = structured(&client.call(session, "desktop_state", json!({}))?)?;
        let focused = field(&desktop, "/focused_window").as_u64();
        let wev = field(&desktop, "/windows")
            .as_array()
            .into_iter()
            .flatten()
            .find(|window| field(window, "/app_id") == "wev")
            .and_then(|window| field(window, "/id").as_u64());
        Ok(wev.filter(|id| Some(*id) == focused))
    })
}

fn wheel_frame(frame: &[String], axis: &str, value120: i32, value: &str) -> bool {
    let has = |line: &str| frame.iter().any(|event| event == line);
    let axis_event = frame.iter().any(|event| {
        event.starts_with("axis: time: ")
            && event.ends_with(&format!("; axis: {axis}, value: {value}"))
    });
    has("axis_source: 0 (wheel)")
        && has(&format!(
            "axis_value120: axis: {axis}, value120: {value120}"
        ))
        && axis_event
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixels_target_their_centres() {
        let shot = Shot {
            id: "shot-1".to_owned(),
            origin: (0.0, 0.0),
            scale: 1.5,
        };
        assert_eq!(shot.pixel((10.0, 10.0)), (15, 15));
        assert_eq!(shot.centre((15, 15)), (15.5 / 1.5, 15.5 / 1.5));
    }

    #[test]
    fn a_drag_is_read_from_press_to_release() {
        let trace = [
            Pointer::At(1.0, 1.0),
            Pointer::Button {
                code: 272,
                pressed: true,
            },
            Pointer::At(2.0, 2.0),
            Pointer::At(3.0, 3.0),
            Pointer::Button {
                code: 272,
                pressed: false,
            },
        ];
        assert_eq!(drag_points(&trace).unwrap(), ((1.0, 1.0), (3.0, 3.0), 2));
        assert!(drag_points(&trace[..2]).is_err());
    }

    #[test]
    fn a_wheel_frame_needs_source_value120_and_value() {
        let frame = [
            "axis_source: 0 (wheel)",
            "axis_value120: axis: 0 (vertical), value120: 240",
            "axis: time: 5; axis: 0 (vertical), value: 30.000000",
        ]
        .map(str::to_owned);
        assert!(wheel_frame(&frame, "0 (vertical)", 240, "30.000000"));
        assert!(!wheel_frame(&frame, "0 (vertical)", 120, "30.000000"));
        assert!(!wheel_frame(&frame, "1 (horizontal)", 240, "30.000000"));
        assert!(!wheel_frame(&frame[1..], "0 (vertical)", 240, "30.000000"));
    }
}
