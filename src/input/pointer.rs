//! The pointer tools' work (plan §6, §8): aim at pixels of a screenshot ref, then send the
//! gesture through a virtual pointer bound to that screenshot's output.
//!
//! A point is a pixel of the screenshot, or an accessible element `elements` listed,
//! whose place is worked out again from niri and the accessibility bus right before the
//! gesture (`elements::aim`) and must lie inside the screenshot.
//!
//! Before anything is sent, the focused window's app must not be on the deny list, the
//! outputs, requested right then, must be a setup live tests cover, and every point must
//! pass the ref's checks. A gesture that presses a button writes the input-dirty marker
//! first, and removes it only once every button it pressed is released: when the gesture
//! ends, or, when it is dropped midway by a stop or a cancelled request, by the release
//! sent on the way out.

use std::ops::RangeInclusive;
use std::time::Duration;

use tokio::time::Instant;

use super::{
    Input, focused_app_id,
    held::{self, Held},
};
use crate::a11y::model::Unmappable;
use crate::a11y::{self, ElementRef};
use crate::act::{Observed, Outcome};
use crate::control::marker::{Marker, Written};
use crate::control::runtime::RuntimeDir;
use crate::coords::{ImagePx, LayoutPt, ProtocolPt};
use crate::error::{CallError, ErrorName, ToolError};
use crate::niri::pointer::{Axis, Pointer, Step};
use crate::niri::waiter::Waited;
use crate::policy;
use crate::refs::Shot;
use crate::{elements, niri};

/// How long the pointer rests on a drag's start before and after pressing, so the app
/// sees the hover and the press as separate moments.
const HOVER: Duration = Duration::from_millis(50);
/// A drag moves in this many steps, this far apart.
const DRAG_STEPS: u32 = 10;
const DRAG_STEP: Duration = Duration::from_millis(20);
const MAX_CLICKS: u8 = 3;
const MAX_NOTCHES: i32 = 10;
/// The wheel notches one call may turn each way.
const NOTCHES: RangeInclusive<i32> = -MAX_NOTCHES..=MAX_NOTCHES;

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

/// Where a gesture points, as the agent named it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Spot {
    Pixel(ImagePx),
    /// An element ref, `elem-<tag>-N`.
    Element(String),
}

/// A gesture at points of type `P`: `Spot`s as asked, then layout points once located.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Gesture<P> {
    Move(P),
    Click {
        at: P,
        button: Button,
        count: u8,
    },
    Drag {
        from: P,
        to: P,
        button: Button,
    },
    /// Wheel notches: positive is right or down.
    Scroll {
        at: P,
        notches_x: i32,
        notches_y: i32,
    },
}

impl<P> Gesture<P> {
    pub(crate) const fn tool(&self) -> &'static str {
        match self {
            Self::Move(_) => "pointer_move",
            Self::Click { .. } => "click",
            Self::Drag { .. } => "drag",
            Self::Scroll { .. } => "scroll",
        }
    }

    /// The button the gesture presses, if any.
    const fn button(&self) -> Option<Button> {
        match self {
            Self::Click { button, .. } | Self::Drag { button, .. } => Some(*button),
            Self::Move(_) | Self::Scroll { .. } => None,
        }
    }

    /// Checks the arguments that the schema can't.
    fn check(&self) -> Result<(), CallError> {
        let mistake = |message: String| Err(CallError::InvalidArguments(message));
        match *self {
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
            } if !NOTCHES.contains(&notches_x) || !NOTCHES.contains(&notches_y) => {
                mistake(format!(
                    "at most {MAX_NOTCHES} notches each way per call; scroll again after a screenshot"
                ))
            }
            Self::Move(_) | Self::Click { .. } | Self::Drag { .. } | Self::Scroll { .. } => Ok(()),
        }
    }

    /// The same gesture at the points `locate` gives for each of its own, in order.
    async fn located<Q, F>(self, mut locate: impl FnMut(P) -> F) -> Result<Gesture<Q>, ToolError>
    where
        F: Future<Output = Result<Q, ToolError>>,
    {
        Ok(match self {
            Self::Move(at) => Gesture::Move(locate(at).await?),
            Self::Click { at, button, count } => Gesture::Click {
                at: locate(at).await?,
                button,
                count,
            },
            Self::Drag { from, to, button } => Gesture::Drag {
                from: locate(from).await?,
                to: locate(to).await?,
                button,
            },
            Self::Scroll {
                at,
                notches_x,
                notches_y,
            } => Gesture::Scroll {
                at: locate(at).await?,
                notches_x,
                notches_y,
            },
        })
    }
}

impl Gesture<LayoutPt> {
    /// The points the gesture goes through, in order. A drag goes from its start to its
    /// end in even steps.
    fn points(self) -> Vec<LayoutPt> {
        match self {
            Self::Move(at) | Self::Click { at, .. } | Self::Scroll { at, .. } => vec![at],
            Self::Drag { from, to, .. } => (0..=DRAG_STEPS)
                .map(|step| {
                    let along = f64::from(step) / f64::from(DRAG_STEPS);
                    LayoutPt {
                        x: (to.x - from.x).mul_add(along, from.x),
                        y: (to.y - from.y).mul_add(along, from.y),
                    }
                })
                .collect(),
        }
    }
}

/// One thing to do: send a step, or wait.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Planned {
    Send(Step),
    Pause(Duration),
}

/// The steps of `gesture`, given where each of its points is on the output.
fn plan<P>(gesture: &Gesture<P>, aimed: &[ProtocolPt]) -> Vec<Planned> {
    let motions = aimed
        .iter()
        .map(|point| Planned::Send(Step::Motion(*point)));
    match gesture {
        Gesture::Move(_) => motions.collect(),
        &Gesture::Click { button, count, .. } => {
            let click = [
                Planned::Send(Step::Press(button.code())),
                Planned::Send(Step::Release(button.code())),
            ];
            motions.chain((0..count).flat_map(|_| click)).collect()
        }
        &Gesture::Drag { button, .. } => {
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
        &Gesture::Scroll {
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
/// input, and its effect is for the next screenshot to show. `element` looks an element
/// ref up.
pub(crate) async fn point(
    input: Input<'_>,
    shot: Result<Shot, ToolError>,
    gesture: Gesture<Spot>,
    keys: &[String],
    element: impl Fn(&str) -> Result<ElementRef, ToolError> + Sync,
) -> Result<Outcome, CallError> {
    gesture.check()?;
    held::check(keys)?;
    let shot = shot?;
    let mut waiter = niri::waiter(input.niri.events).await?;
    if let Some(refused) = policy::refuse_input(input.policy, focused_app_id(waiter.view())) {
        return Err(refused.into());
    }
    let socket = input.niri.socket;
    let outputs = niri::outputs(socket).await?;
    policy::pointer_support(outputs.values())?;
    let connection = input
        .niri
        .events
        .ok()
        .and_then(niri::events::EventStream::connection);
    shot.check(Instant::now(), &outputs, connection)?;
    let tool = gesture.tool();
    let button = gesture.button();
    let located = gesture
        .located(|spot| locate(input, &shot, spot, &element))
        .await?;
    let now = Instant::now();
    let aimed = located
        .points()
        .into_iter()
        .map(|point| shot.aim_at(point, now, &outputs, connection))
        .collect::<Result<Vec<_>, _>>()?;
    let display = input.display.path().map_err(Clone::clone)?;
    let held = Held::prepare(input, waiter.view(), keys).await?;
    let pointer = Pointer::bind(display, niri::pid(socket).await?, &shot.output).await?;
    let pressing = Pressing { tool, button };
    let mut device = Device::new(pointer, input.runtime, pressing, &shot.output, held)?;
    if let Err(error) = device.run(plan(&located, &aimed)).await {
        // Nothing reached niri: an error like any other before input.
        if !device.sent() {
            return Err(error.into());
        }
        return Ok(Outcome::uncertain(None, Some(waiter.view()), error.detail));
    }
    device.finish()?;
    // The events the input caused so far, such as a click's focus change.
    if let Waited::Lost(reason) = waiter.until(Duration::ZERO, |_| None::<()>).await {
        return Ok(Outcome::uncertain(Some(true), Some(waiter.view()), reason));
    }
    Ok(Outcome::seen(Observed::Sent, waiter.view(), Vec::new()))
}

/// Where `spot` is in the layout: a pixel's centre, or an element's, which must lie in
/// the screenshot.
async fn locate(
    input: Input<'_>,
    shot: &Shot,
    asked: Spot,
    element: &(impl Fn(&str) -> Result<ElementRef, ToolError> + Sync),
) -> Result<LayoutPt, ToolError> {
    let id = match asked {
        Spot::Pixel(pixel) => return shot.layout_of(pixel),
        Spot::Element(id) => id,
    };
    let kept = element(&id)?;
    let a11y = input
        .a11y
        .ok_or_else(|| a11y::not_accessible("this session has no accessibility bus".to_owned()))?;
    let point = elements::aim(input.niri.socket, a11y, input.policy, &kept).await?;
    if shot.contains(point) {
        Ok(point)
    } else {
        Err(elements::unmappable(Unmappable::OutsideScreenshot))
    }
}

/// The tool a gesture belongs to and the button it presses, for the marker.
#[derive(Debug, Clone, Copy)]
struct Pressing {
    tool: &'static str,
    button: Option<Button>,
}

/// The pointer, with the input-dirty marker while a gesture may press a button. Dropping
/// it midway, as a stop or a cancelled request does, releases whatever is still pressed
/// and removes the marker only once niri has handled those releases.
#[derive(Debug)]
struct Device {
    /// Always there until the device is dropped.
    pointer: Option<Pointer>,
    marker: Option<Written>,
    held: Option<Held>,
}

impl Device {
    /// Writes the marker first if `gesture` presses a button, naming the output the
    /// pointer is bound to, which `recover` binds its release to.
    fn new(
        pointer: Pointer,
        runtime: &RuntimeDir,
        pressing: Pressing,
        output: &str,
        held: Option<Held>,
    ) -> Result<Self, ToolError> {
        let marker = (pressing.button.is_some() || held.is_some())
            .then(|| {
                let buttons = pressing.button.into_iter().map(Button::code).collect();
                let mut marker = Marker::pending(pressing.tool, buttons);
                marker.output = Some(output.to_owned());
                marker.keyboard = held.as_ref().map(Held::marker);
                Written::write(runtime, marker)
            })
            .transpose()
            .map_err(|error| {
                ToolError::new(
                    ErrorName::UpstreamError,
                    format!("write the input-dirty marker: {error}"),
                )
            })?;
        Ok(Self {
            pointer: Some(pointer),
            marker,
            held,
        })
    }

    fn pointer(&mut self) -> Result<&mut Pointer, ToolError> {
        self.pointer
            .as_mut()
            .ok_or_else(|| ToolError::new(ErrorName::UpstreamError, "the pointer is gone"))
    }

    fn sent(&self) -> bool {
        self.pointer.as_ref().is_some_and(Pointer::sent)
    }

    /// Sends the planned steps and waits until niri has handled them.
    async fn run(&mut self, steps: Vec<Planned>) -> Result<(), ToolError> {
        if let Some(held) = &mut self.held {
            held.begin().await?;
        }
        for planned in steps {
            match planned {
                Planned::Send(step) => self.pointer()?.send(step)?,
                Planned::Pause(pause) => tokio::time::sleep(pause).await,
            }
        }
        self.pointer()?.sync().await?;
        if let Some(held) = &mut self.held {
            held.released().await?;
        }
        Ok(())
    }

    /// Removes the marker after a gesture niri has handled whole.
    fn finish(mut self) -> Result<(), ToolError> {
        let Some(marker) = self.marker.take() else {
            return Ok(());
        };
        marker.clear().map_err(|error| {
            ToolError::new(
                ErrorName::UpstreamError,
                format!(
                    "niri handled the input, but the input-dirty marker couldn't be removed ({error}); actions refuse until the user runs `niri-computer-use recover`"
                ),
            )
        })
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        let (Some(marker), Some(mut pointer)) = (self.marker.take(), self.pointer.take()) else {
            return;
        };
        // The release is sent now; waiting for niri to handle it needs the runtime, so a
        // task of its own does that and then removes the marker. Without a runtime, the
        // pointer's own drop still sends the release, and the marker stays.
        let mut held = self.held.take();
        let keyboard_released = held.as_mut().is_none_or(Held::release_now);
        let pointer_released = pointer.release_all();
        if keyboard_released
            && pointer_released
            && let Ok(runtime) = tokio::runtime::Handle::try_current()
        {
            runtime.spawn(release(pointer, held, marker));
        }
    }
}

/// Releases what `pointer` still holds and removes `marker` once niri has handled it. If
/// the marker can't be removed, it stays, and blocks input until `recover`.
async fn release(mut pointer: Pointer, mut held: Option<Held>, marker: Written) {
    if pointer.sync().await.is_err() {
        return;
    }
    if let Some(held) = &mut held
        && held.released().await.is_err()
    {
        return;
    }
    marker.clear().ok();
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
            plan(&double, &[at(1)]),
            [
                Planned::Send(Step::Motion(at(1))),
                Planned::Send(Step::Press(273)),
                Planned::Send(Step::Release(273)),
                Planned::Send(Step::Press(273)),
                Planned::Send(Step::Release(273)),
            ]
        );
        assert_eq!(
            plan(&Gesture::Move(PX), &[at(1)]),
            [Planned::Send(Step::Motion(at(1)))]
        );
    }

    #[test]
    fn a_drag_presses_at_its_start_moves_in_steps_and_releases_at_its_end() {
        let drag = Gesture::Drag {
            from: LayoutPt { x: 0.0, y: 100.0 },
            to: LayoutPt { x: 15.0, y: -20.0 },
            button: Button::Left,
        };
        let points = drag.points();
        assert_eq!(points.len(), 11);
        assert_eq!(points.first(), Some(&LayoutPt { x: 0.0, y: 100.0 }));
        assert_eq!(points.get(1), Some(&LayoutPt { x: 1.5, y: 88.0 }));
        assert_eq!(points.last(), Some(&LayoutPt { x: 15.0, y: -20.0 }));
        let planned = plan(&drag, &[at(0), at(1), at(2)]);
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
            plan(&both, &[at(1)]),
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
            plan(&down, &[at(1)]),
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
