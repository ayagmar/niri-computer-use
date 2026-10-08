//! M0 probe: one virtual pointer, bound to one output, sends one action.
//! The hold action keeps the device alive until killed or its three-second deadline.
//!
//! The pointer is only created once the named output is found, so run against a
//! compositor without that output (the host has no `winit`), it sends no input.

mod coords;

use std::env;
use std::error::Error;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use coords::{Capture, ImagePx, LayoutPt, Output, ProtocolPt, Transform};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_output::{self, WlOutput};
use wayland_client::protocol::wl_pointer::{Axis, AxisSource, ButtonState};
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::protocol::wl_seat::WlSeat;
use wayland_client::{Connection, Dispatch, QueueHandle, delegate_noop};
use wayland_protocols_wlr::virtual_pointer::v1::client::zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1;
use wayland_protocols_wlr::virtual_pointer::v1::client::zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1;

const USAGE: &str = "usage: vpointer <output> <x>,<y>,<width>,<height> <transform> <time> <action>
actions:
  motion-layout <x> <y>                                   a layout point
  motion-image <px> <py> <origin x> <origin y> <scale>    an image pixel's centre
  click <button>                                          press, then release
  scroll                                                  one wheel notch down
  hold <button>                                           press and hold for up to 3 seconds
  release <button>                                        release only";

type Result<T> = std::result::Result<T, Box<dyn Error>>;

#[derive(Debug)]
enum Action {
    Motion(ProtocolPt),
    Click(u32),
    Hold(u32),
    Release(u32),
    Scroll,
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("vpointer: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &[String]) -> Result<()> {
    let [name, geometry, transform, time, action @ ..] = args else {
        return Err(USAGE.into());
    };
    let output = parse_output(geometry, transform)?;
    let time: u32 = time.parse()?;
    let action = parse_action(action, &output)?;
    send(name, time, &action)
}

fn parse_output(geometry: &str, transform: &str) -> Result<Output> {
    let parts: Vec<i32> = geometry
        .split(',')
        .map(str::parse)
        .collect::<std::result::Result<_, _>>()?;
    let [x, y, width, height] = parts[..] else {
        return Err(format!("geometry {geometry:?} is not x,y,width,height").into());
    };
    if width <= 0 || height <= 0 {
        return Err(format!("output size {width}x{height} is not positive").into());
    }
    let transform =
        Transform::parse(transform).ok_or_else(|| format!("unknown transform {transform:?}"))?;
    Ok(Output {
        x,
        y,
        width,
        height,
        transform,
    })
}

fn parse_action(action: &[String], output: &Output) -> Result<Action> {
    // `f64::from_str` accepts "NaN" and "inf", which the encoding can't handle.
    let numbers = |values: &[String]| -> Result<Vec<f64>> {
        values
            .iter()
            .map(|value| match value.parse::<f64>() {
                Ok(number) if number.is_finite() => Ok(number),
                _ => Err(format!("{value:?} is not a finite number").into()),
            })
            .collect()
    };
    match action {
        [kind, rest @ ..] if kind == "motion-layout" => {
            let [x, y] = numbers(rest)?[..] else {
                return Err(USAGE.into());
            };
            motion(LayoutPt { x, y }, output)
        }
        [kind, px, py, rest @ ..] if kind == "motion-image" => {
            let [x, y, scale] = numbers(rest)?[..] else {
                return Err(USAGE.into());
            };
            if scale <= 0.0 {
                return Err(format!("capture scale {scale} is not positive").into());
            }
            let pixel = ImagePx {
                x: px.parse()?,
                y: py.parse()?,
            };
            let capture = Capture {
                origin: LayoutPt { x, y },
                scale,
            };
            println!(
                "image pixel ({}, {}) at capture scale {scale}",
                pixel.x, pixel.y
            );
            motion(coords::image_to_layout(pixel, capture), output)
        }
        [kind, button] if kind == "click" => Ok(Action::Click(button.parse()?)),
        [kind, button] if kind == "hold" => Ok(Action::Hold(button.parse()?)),
        [kind, button] if kind == "release" => Ok(Action::Release(button.parse()?)),
        [kind] if kind == "scroll" => Ok(Action::Scroll),
        _ => Err(USAGE.into()),
    }
}

/// Encodes the target and refuses to send it unless niri's own mapping lands on it.
fn motion(target: LayoutPt, output: &Output) -> Result<Action> {
    let request = coords::checked_encode(target, output)
        .map_err(|error| format!("niri would land {error} px from the target"))?;
    let landed = coords::niri_forward(request, output);
    println!(
        "target ({:.4}, {:.4}) -> motion_absolute x={} y={} x_extent={} y_extent={} \
         -> niri ({:.4}, {:.4}), off by {:.4}",
        target.x,
        target.y,
        request.x,
        request.y,
        request.x_extent,
        request.y_extent,
        landed.x,
        landed.y,
        coords::forward_error(target, request, output),
    );
    Ok(Action::Motion(request))
}

#[derive(Debug, Default)]
struct State {
    names: Vec<(WlOutput, String)>,
}

fn send(output_name: &str, time: u32, action: &Action) -> Result<()> {
    let connection = Connection::connect_to_env()?;
    let (globals, mut queue) = registry_queue_init::<State>(&connection)?;
    let qh = queue.handle();
    let seat: WlSeat = globals.bind(&qh, 1..=1, ())?;
    let manager: ZwlrVirtualPointerManagerV1 = globals.bind(&qh, 2..=2, ())?;
    // `wl_output.name` needs version 4.
    for global in globals.contents().clone_list() {
        if global.interface == "wl_output" && global.version >= 4 {
            globals
                .registry()
                .bind::<WlOutput, _, _>(global.name, 4, &qh, ());
        }
    }
    let mut state = State::default();
    queue.roundtrip(&mut state)?;
    let output = state
        .names
        .iter()
        .find(|(_, name)| name == output_name)
        .map(|(output, _)| output)
        .ok_or_else(|| format!("no output named {output_name:?}"))?;

    let pointer = manager.create_virtual_pointer_with_output(Some(&seat), Some(output), &qh, ());
    dispatch(&pointer, time, action);
    if matches!(action, Action::Hold(_)) {
        queue.roundtrip(&mut state)?;
        let end = Instant::now() + Duration::from_secs(3);
        while Instant::now() < end {
            std::thread::sleep(end.saturating_duration_since(Instant::now()));
        }
        return Err("held pointer was not killed within 3 seconds".into());
    }
    pointer.destroy();
    // niri has handled every request once this round trip returns.
    queue.roundtrip(&mut state)?;
    Ok(())
}

fn dispatch(pointer: &ZwlrVirtualPointerV1, time: u32, action: &Action) {
    match *action {
        Action::Motion(request) => {
            pointer.motion_absolute(
                time,
                request.x,
                request.y,
                request.x_extent,
                request.y_extent,
            );
            pointer.frame();
            println!("sent motion_absolute, frame");
        }
        Action::Click(button) => {
            pointer.button(time, button, ButtonState::Pressed);
            pointer.frame();
            pointer.button(time, button, ButtonState::Released);
            pointer.frame();
            println!("sent button {button} pressed, frame, released, frame");
        }
        Action::Hold(button) => {
            pointer.button(time, button, ButtonState::Pressed);
            pointer.frame();
            println!("sent button {button} pressed, frame; holding device");
        }
        Action::Release(button) => {
            pointer.button(time, button, ButtonState::Released);
            pointer.frame();
            println!("sent button {button} released, frame");
        }
        Action::Scroll => {
            // niri creates the axis frame in `axis_discrete`; `axis_source` only changes an
            // existing frame, so it has to come second.
            pointer.axis_discrete(time, Axis::VerticalScroll, 15.0, 1);
            pointer.axis_source(AxisSource::Wheel);
            pointer.frame();
            println!("sent axis_discrete(vertical, 15.0, 1), axis_source(wheel), frame");
        }
    }
}

impl Dispatch<WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: <WlRegistry as wayland_client::Proxy>::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlOutput, ()> for State {
    fn event(
        state: &mut Self,
        output: &WlOutput,
        event: wl_output::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_output::Event::Name { name } = event {
            state.names.push((output.clone(), name));
        }
    }
}

delegate_noop!(State: ignore WlSeat);
delegate_noop!(State: ZwlrVirtualPointerManagerV1);
delegate_noop!(State: ZwlrVirtualPointerV1);

#[cfg(test)]
mod tests {
    use super::*;

    fn args(text: &str) -> Vec<String> {
        text.split(' ').map(str::to_owned).collect()
    }

    const NESTED: Output = Output {
        x: 0,
        y: 0,
        width: 640,
        height: 480,
        transform: Transform::Flipped180,
    };

    #[test]
    fn parses_the_geometry_and_transform_the_harness_sends() {
        assert_eq!(parse_output("0,0,640,480", "Flipped180").unwrap(), NESTED);
        assert!(parse_output("0,0,640", "Flipped180").is_err());
        assert!(parse_output("0,0,640,480", "flipped").is_err());
        assert!(parse_output("0,0,0,480", "Flipped180").is_err());
        assert!(parse_output("0,0,640,-1", "Flipped180").is_err());
    }

    #[test]
    fn parses_each_action_the_harness_sends() {
        let motion = parse_action(&args("motion-layout 10 10"), &NESTED).unwrap();
        assert!(matches!(motion, Action::Motion(request) if request.y == 470_000));
        let image = parse_action(&args("motion-image 300 225 0 0 1.5"), &NESTED).unwrap();
        assert!(matches!(image, Action::Motion(request) if request.x == 200_333));
        let click = parse_action(&args("click 272"), &NESTED).unwrap();
        assert!(matches!(click, Action::Click(272)));
        assert!(matches!(
            parse_action(&args("scroll"), &NESTED).unwrap(),
            Action::Scroll
        ));
    }

    #[test]
    fn hold_and_release_parse_without_an_implicit_click() {
        assert!(matches!(
            parse_action(&args("hold 272"), &NESTED).unwrap(),
            Action::Hold(272)
        ));
        assert!(matches!(
            parse_action(&args("release 272"), &NESTED).unwrap(),
            Action::Release(272)
        ));
        for bad in [
            "hold",
            "hold -1",
            "hold 272 extra",
            "release",
            "release bad",
        ] {
            assert!(parse_action(&args(bad), &NESTED).is_err(), "{bad}");
        }
    }

    #[test]
    fn refuses_bad_actions_and_targets_off_the_output() {
        for bad in [
            "motion-layout 10",
            "click",
            "scroll 1",
            "jump",
            "motion-layout 700 10",
            "motion-layout NaN 10",
            "motion-layout 10 inf",
            "motion-image 300 225 0 0 0",
            "motion-image 300 225 NaN 0 1.5",
        ] {
            assert!(parse_action(&args(bad), &NESTED).is_err(), "{bad}");
        }
    }
}
