//! The pointer tools' work (plan §6, §8): aim at pixels of a screenshot ref, then send the
//! gesture through a virtual pointer bound to that screenshot's output.
//!
//! Before anything is sent, the focused window's app must not be on the deny list, the
//! outputs, requested right then, must be a setup live tests cover, and every point must
//! pass the ref's checks. A gesture that presses a button writes the input-dirty marker
//! first, and removes it only once every button it pressed is released: when the gesture
//! ends, or, when it is dropped midway by a stop or a cancelled request, by the release
//! sent on the way out.

use std::path::Path;
use std::time::Duration;

use tokio::time::Instant;

use crate::act::{Niri, Observed, Outcome};
use crate::control::marker::{Marker, Written};
use crate::control::runtime::RuntimeDir;
use crate::coords::{ImagePx, ProtocolPt};
use crate::error::{CallError, ErrorName, ToolError};
use crate::niri::pointer::{Axis, Pointer, Step};
use crate::niri::{self, waiter::Waiter};
use crate::policy::{self, Loaded};
use crate::refs::Shot;

/// How long the pointer rests on a drag's start before and after pressing, so the app
/// sees the hover and the press as separate moments.
const HOVER: Duration = Duration::from_millis(50);
/// A drag moves in this many steps, this far apart.
const DRAG_STEPS: u32 = 10;
const DRAG_STEP: Duration = Duration::from_millis(20);
const MAX_CLICKS: u8 = 3;
const MAX_NOTCHES: i32 = 10;

/// What the pointer tools need besides their arguments.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Input<'a> {
    pub(crate) niri: Niri<'a>,
    /// The Wayland display's socket, if known.
    pub(crate) display: Option<&'a Path>,
    pub(crate) runtime: &'a RuntimeDir,
    pub(crate) policy: &'a Loaded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Button {
    Left,
    Right,
    Middle,
}

impl Button {
    /// The evdev code, such as `BTN_LEFT` (272).
    const fn code(self) -> u32 {
        match self {
            Self::Left => 272,
            Self::Right => 273,
            Self::Middle => 274,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Gesture {
    Move(ImagePx),
    Click {
        at: ImagePx,
        button: Button,
        count: u8,
    },
    Drag {
        from: ImagePx,
        to: ImagePx,
        button: Button,
    },
    /// Wheel notches: positive is right or down.
    Scroll {
        at: ImagePx,
        notches_x: i32,
        notches_y: i32,
    },
}

impl Gesture {
    pub(crate) const fn tool(self) -> &'static str {
        match self {
            Self::Move(_) => "pointer_move",
            Self::Click { .. } => "click",
            Self::Drag { .. } => "drag",
            Self::Scroll { .. } => "scroll",
        }
    }

    /// The button the gesture presses, if any.
    const fn button(self) -> Option<Button> {
        match self {
            Self::Click { button, .. } | Self::Drag { button, .. } => Some(button),
            Self::Move(_) | Self::Scroll { .. } => None,
        }
    }

    /// Checks the arguments that the schema can't.
    fn check(self) -> Result<(), CallError> {
        let mistake = |message: String| Err(CallError::InvalidArguments(message));
        match self {
            Self::Click { count, .. } if !(1..=MAX_CLICKS).contains(&count) => {
                mistake(format!("`count` must be 1 to {MAX_CLICKS}, not {count}"))
            }
            Self::Scroll {
                notches_x,
                notches_y,
                ..
            } if notches_x == 0 && notches_y == 0 => {
                mistake("`notches_x` or `notches_y` must not be 0".to_owned())
            }
            Self::Scroll {
                notches_x,
                notches_y,
                ..
            } if notches_x.abs() > MAX_NOTCHES || notches_y.abs() > MAX_NOTCHES => {
                mistake(format!(
                    "at most {MAX_NOTCHES} notches each way per call; scroll again after a screenshot"
                ))
            }
            Self::Move(_) | Self::Click { .. } | Self::Drag { .. } | Self::Scroll { .. } => Ok(()),
        }
    }

    /// The image pixels the gesture goes through, in order. A drag goes from its start to
    /// its end in even steps.
    fn pixels(self) -> Vec<ImagePx> {
        match self {
            Self::Move(at) | Self::Click { at, .. } | Self::Scroll { at, .. } => vec![at],
            Self::Drag { from, to, .. } => (0..=DRAG_STEPS)
                .map(|step| ImagePx {
                    x: between(from.x, to.x, step),
                    y: between(from.y, to.y, step),
                })
                .collect(),
        }
    }
}

/// The point `step` of `DRAG_STEPS` along from `from` to `to`, rounded to a pixel.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "a point between two u32 pixels, rounded, is a u32 pixel"
)]
fn between(from: u32, to: u32, step: u32) -> u32 {
    let from = f64::from(from);
    let to = f64::from(to);
    (from + (to - from) * f64::from(step) / f64::from(DRAG_STEPS)).round() as u32
}

/// One thing to do: send a step, or wait.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Planned {
    Send(Step),
    Pause(Duration),
}

/// The steps of `gesture`, given where each of its pixels is on the output.
fn plan(gesture: Gesture, aimed: &[ProtocolPt]) -> Vec<Planned> {
    let motions = aimed
        .iter()
        .map(|point| Planned::Send(Step::Motion(*point)));
    match gesture {
        Gesture::Move(_) => motions.collect(),
        Gesture::Click { button, count, .. } => {
            let click = [
                Planned::Send(Step::Press(button.code())),
                Planned::Send(Step::Release(button.code())),
            ];
            motions.chain((0..count).flat_map(|_| click)).collect()
        }
        Gesture::Drag { button, .. } => {
            let mut planned = Vec::new();
            let mut motions = motions;
            planned.extend(motions.next());
            planned.extend([
                Planned::Pause(HOVER),
                Planned::Send(Step::Press(button.code())),
                Planned::Pause(HOVER),
            ]);
            for motion in motions {
                planned.extend([motion, Planned::Pause(DRAG_STEP)]);
            }
            planned.push(Planned::Send(Step::Release(button.code())));
            planned
        }
        Gesture::Scroll {
            notches_x,
            notches_y,
            ..
        } => motions
            .chain(
                [(Axis::Vertical, notches_y), (Axis::Horizontal, notches_x)]
                    .into_iter()
                    .filter(|(_, notches)| *notches != 0)
                    .map(|(axis, notches)| Planned::Send(Step::Wheel(axis, notches))),
            )
            .collect(),
    }
}

/// Aims `gesture` through `shot` and sends it. `observed` is `sent`: niri handled the
/// input, and its effect is for the next screenshot to show.
pub(crate) async fn point(
    input: Input<'_>,
    shot: Result<Shot, ToolError>,
    gesture: Gesture,
) -> Result<Outcome, CallError> {
    gesture.check()?;
    let shot = shot?;
    let mut waiter = niri::waiter(input.niri.events).await?;
    let focused_app = focused_app_id(&waiter);
    if let Some(refused) = policy::refuse_input(input.policy, focused_app.as_deref()) {
        return Err(refused.into());
    }
    let socket = input.niri.socket;
    let outputs = niri::outputs(socket).await?;
    policy::pointer_support(outputs.values())?;
    let now = Instant::now();
    let connection = input
        .niri
        .events
        .and_then(niri::events::EventStream::connection);
    let aimed = gesture
        .pixels()
        .into_iter()
        .map(|pixel| shot.aim(pixel, now, &outputs, connection))
        .collect::<Result<Vec<_>, _>>()?;
    let display = input.display.ok_or_else(|| {
        ToolError::new(
            ErrorName::UpstreamError,
            "WAYLAND_DISPLAY or XDG_RUNTIME_DIR is not set",
        )
    })?;
    let pointer = Pointer::bind(display, niri::pid(socket).await?, &shot.output).await?;
    let device = Device::new(pointer, input.runtime, gesture)?;
    if let Err(lost) = device.run(plan(gesture, &aimed)).await {
        return Ok(Outcome::uncertain(None, Some(waiter.view()), lost.detail));
    }
    // The events the input caused so far, such as a click's focus change.
    waiter.until(Duration::ZERO, |_| None::<()>).await;
    Ok(Outcome::seen(Observed::Sent, waiter.view(), Vec::new()))
}

fn focused_app_id(waiter: &Waiter) -> Option<String> {
    let view = waiter.view();
    view.windows().get(&view.focused_window()?)?.app_id.clone()
}

/// The pointer, with the input-dirty marker while a gesture may press a button. Dropping
/// it releases whatever is still pressed and removes the marker once those releases are
/// sent.
#[derive(Debug)]
struct Device {
    pointer: Pointer,
    marker: Option<Written>,
}

impl Device {
    /// Writes the marker first if `gesture` presses a button.
    fn new(pointer: Pointer, runtime: &RuntimeDir, gesture: Gesture) -> Result<Self, ToolError> {
        let marker = gesture
            .button()
            .map(|button| {
                Written::write(
                    runtime,
                    Marker::pending(gesture.tool(), vec![button.code()]),
                )
            })
            .transpose()
            .map_err(|error| {
                ToolError::new(
                    ErrorName::UpstreamError,
                    format!("write the input-dirty marker: {error}"),
                )
            })?;
        Ok(Self { pointer, marker })
    }

    /// Sends the planned steps, waits until niri has handled them, then removes the
    /// marker.
    async fn run(mut self, steps: Vec<Planned>) -> Result<(), ToolError> {
        for planned in steps {
            match planned {
                Planned::Send(step) => self.pointer.send(step)?,
                Planned::Pause(pause) => tokio::time::sleep(pause).await,
            }
        }
        self.pointer.sync().await?;
        if let Some(marker) = self.marker.take() {
            marker.clear().map_err(|error| {
                ToolError::new(
                    ErrorName::UpstreamError,
                    format!("remove the input-dirty marker: {error}"),
                )
            })?;
        }
        Ok(())
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        if let Some(marker) = self.marker.take()
            && self.pointer.release_all()
        {
            // If it can't be removed, it stays, and blocks input until `recover`.
            marker.clear().ok();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn at(x: u32) -> ProtocolPt {
        ProtocolPt {
            x,
            y: 0,
            x_extent: 1000,
            y_extent: 1000,
        }
    }

    const PX: ImagePx = ImagePx { x: 5, y: 5 };

    #[test]
    fn a_click_moves_first_then_presses_and_releases_count_times() {
        let double = Gesture::Click {
            at: PX,
            button: Button::Right,
            count: 2,
        };
        assert_eq!(
            plan(double, &[at(1)]),
            [
                Planned::Send(Step::Motion(at(1))),
                Planned::Send(Step::Press(273)),
                Planned::Send(Step::Release(273)),
                Planned::Send(Step::Press(273)),
                Planned::Send(Step::Release(273)),
            ]
        );
        assert_eq!(
            plan(Gesture::Move(PX), &[at(1)]),
            [Planned::Send(Step::Motion(at(1)))]
        );
    }

    #[test]
    fn a_drag_presses_at_its_start_moves_in_steps_and_releases_at_its_end() {
        let drag = Gesture::Drag {
            from: ImagePx { x: 0, y: 100 },
            to: ImagePx { x: 15, y: 0 },
            button: Button::Left,
        };
        let pixels = drag.pixels();
        assert_eq!(pixels.len(), 11);
        assert_eq!(pixels.first(), Some(&ImagePx { x: 0, y: 100 }));
        assert_eq!(pixels.get(1), Some(&ImagePx { x: 2, y: 90 }));
        assert_eq!(pixels.last(), Some(&ImagePx { x: 15, y: 0 }));
        let planned = plan(drag, &[at(0), at(1), at(2)]);
        assert_eq!(
            planned,
            [
                Planned::Send(Step::Motion(at(0))),
                Planned::Pause(HOVER),
                Planned::Send(Step::Press(272)),
                Planned::Pause(HOVER),
                Planned::Send(Step::Motion(at(1))),
                Planned::Pause(DRAG_STEP),
                Planned::Send(Step::Motion(at(2))),
                Planned::Pause(DRAG_STEP),
                Planned::Send(Step::Release(272)),
            ]
        );
    }

    #[test]
    fn a_scroll_moves_then_turns_the_wheels_it_was_given() {
        let both = Gesture::Scroll {
            at: PX,
            notches_x: -1,
            notches_y: 3,
        };
        assert_eq!(
            plan(both, &[at(1)]),
            [
                Planned::Send(Step::Motion(at(1))),
                Planned::Send(Step::Wheel(Axis::Vertical, 3)),
                Planned::Send(Step::Wheel(Axis::Horizontal, -1)),
            ]
        );
        let down = Gesture::Scroll {
            at: PX,
            notches_x: 0,
            notches_y: 1,
        };
        assert_eq!(
            plan(down, &[at(1)]),
            [
                Planned::Send(Step::Motion(at(1))),
                Planned::Send(Step::Wheel(Axis::Vertical, 1)),
            ]
        );
    }

    #[test]
    fn counts_and_notches_are_bounded() {
        let click = |count| Gesture::Click {
            at: PX,
            button: Button::Left,
            count,
        };
        let scroll = |notches_x, notches_y| Gesture::Scroll {
            at: PX,
            notches_x,
            notches_y,
        };
        assert!(click(1).check().is_ok());
        assert!(click(3).check().is_ok());
        assert!(click(0).check().is_err());
        assert!(click(4).check().is_err());
        assert!(scroll(0, -10).check().is_ok());
        assert!(scroll(0, 0).check().is_err());
        assert!(scroll(11, 0).check().is_err());
        assert!(scroll(0, -11).check().is_err());
    }

    #[test]
    fn only_gestures_that_press_a_button_name_one() {
        assert_eq!(Gesture::Move(PX).button(), None);
        let drag = Gesture::Drag {
            from: PX,
            to: PX,
            button: Button::Middle,
        };
        assert_eq!(drag.button().map(Button::code), Some(274));
    }
}
