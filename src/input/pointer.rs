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
use crate::act::{self, Observed, Outcome};
use crate::control::cleanup;
use crate::control::marker::{self, Marker, Written};
use crate::control::runtime::RuntimeDir;
use crate::coords::{ImagePx, LayoutPt, ProtocolPt};
use crate::error::{CallError, ErrorName, ToolError};
use crate::niri::events::Motion;
use crate::niri::pointer::{Axis, Pointer, Step};
use crate::niri::waiter::{Moved, Shift, Waited};
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
/// How long after niri activates a workspace or opens or closes the overview no element
/// is aimed at on that output: niri reports the state it is heading to at once, and never
/// when its animation ends. Its default animations take about 0.3 s.
const SETTLE: Duration = Duration::from_secs(1);
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

impl Gesture<Spot> {
    /// The element refs among its points.
    fn elements(&self) -> Vec<&str> {
        let spots = match self {
            Self::Move(at) | Self::Click { at, .. } | Self::Scroll { at, .. } => vec![at],
            Self::Drag { from, to, .. } => vec![from, to],
        };
        spots
            .into_iter()
            .filter_map(|spot| match spot {
                Spot::Element(id) => Some(id.as_str()),
                Spot::Pixel(_) => None,
            })
            .collect()
    }

    /// The element ref a drag ends at, if it ends at one.
    const fn destination(&self) -> Option<&str> {
        match self {
            Self::Drag {
                to: Spot::Element(id),
                ..
            } => Some(id.as_str()),
            Self::Drag { .. } | Self::Move(_) | Self::Click { .. } | Self::Scroll { .. } => None,
        }
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
    // The windows of the elements it aims at, and of the one a drag ends at.
    let windows = gesture
        .elements()
        .into_iter()
        .map(|id| element(id).map(|kept| kept.kept.window))
        .collect::<Result<Vec<_>, _>>()?;
    let destination = gesture
        .destination()
        .map(|id| element(id).map(|kept| kept.kept.window))
        .transpose()?;
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
    act::fresh_lock(input.niri).await?;
    let pointer = Pointer::bind(display, niri::pid(socket).await?, &shot.output).await?;
    let pressing = Pressing { tool, button };
    let device = Device::new(pointer, input.runtime, pressing, &shot.output, held).await?;
    let gate = Gate {
        waiter: &mut waiter,
        input,
        windows: &windows,
        destination,
    };
    if let Some(stopped) = send(device, gate, plan(&located, &aimed)).await? {
        return Ok(stopped);
    }
    // The events the input caused so far, such as a click's focus change.
    if let Waited::Lost(reason) = waiter.until(Duration::ZERO, |_| None::<()>).await {
        return Ok(Outcome::uncertain(Some(true), Some(waiter.view()), reason));
    }
    Ok(Outcome::seen(Observed::Sent, waiter.view(), Vec::new()))
}

/// Sends `steps` through `device`, each part only once `gate` passes it. Refused or failed
/// before anything went out, the call fails like any other before input; afterwards, it
/// stops there, releases what it holds, and its outcome is `uncertain`.
async fn send(
    mut device: Device,
    mut gate: Gate<'_>,
    steps: Vec<Planned>,
) -> Result<Option<Outcome>, CallError> {
    let Err(error) = device.run(steps, &mut gate).await else {
        device.finish().await?;
        return Ok(None);
    };
    if !device.sent() {
        // Nothing was pressed, so the marker goes now, before the next call can see it.
        device.discard().await?;
        return Err(error.into());
    }
    let released = match device.abandon().await {
        Ok(()) => "every button and key it held was released".to_owned(),
        Err(failed) => format!(
            "releasing what it held failed ({}), so actions refuse until the user runs `niri-computer-use recover`",
            failed.detail
        ),
    };
    let detail = format!(
        "{}; the gesture stopped there and {released}; what went out may have taken effect",
        error.detail
    );
    Ok(Some(Outcome::uncertain(
        None,
        Some(gate.waiter.view()),
        detail,
    )))
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
    let aimed = elements::aim(input.niri.socket, a11y, input.policy, &kept).await?;
    if aimed.output != shot.output {
        return Err(elements::unmappable(Unmappable::OtherOutput));
    }
    let events = input.niri.events.map_err(Clone::clone)?;
    match events
        .motion(&aimed.output, kept.kept.window, SETTLE)
        .await?
    {
        Motion::Settled => {}
        Motion::Moving => return Err(elements::unmappable(Unmappable::Moving)),
        Motion::Restless => return Err(elements::unmappable(Unmappable::Restless)),
    }
    if shot.contains(aimed.at) {
        Ok(aimed.at)
    } else {
        Err(elements::unmappable(Unmappable::OutsideScreenshot))
    }
}

/// The checks before each part of a gesture goes out (`Gate::pass`).
struct Gate<'a> {
    waiter: &'a mut niri::waiter::Waiter,
    input: Input<'a>,
    /// The windows of the elements the gesture aims at, none for pixels.
    windows: &'a [u64],
    /// The window of the element a drag ends at, if it ends at one.
    destination: Option<u64>,
}

/// Where a gesture is when its next part goes out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    /// Nothing went out yet.
    First,
    /// Something went out after a wait, the held modifiers or the motion to a drag's start,
    /// and no button is pressed.
    Unpressed,
    /// A button is pressed, during a drag.
    Pressed,
}

impl Gate<'_> {
    /// The checks right before the gesture's next part, with nothing awaited between them
    /// and it: before its first input, and after each wait in it. niri's events up to now
    /// are applied first (`Waiter::moved`). An element aim is refused by any of them that
    /// can change what is shown where since the call's first look, before its elements were
    /// placed, until a button is pressed: a box may no longer be where its element is. Once
    /// something went out, any gesture is stopped by a workspace switch or the overview,
    /// which change the whole view under the pointer. A window moving doesn't stop a drag
    /// that holds a button, as dragging a window by its title bar moves it, unless it is
    /// the window of the element the drag ends at: that element is no longer where the drag
    /// goes once its window moved, resized, floated, went to another workspace or closed.
    /// An app can
    /// change its `app_id` at any time without moving anything, so the deny list is asked
    /// each time about the focused window and the elements' windows as niri reports them
    /// now. The lock state is read again first when the last read is too old, since a
    /// round trip in the gesture can take seconds, and must still be fresh at the end,
    /// however long the look took. Before the first input, the desk is told input is going
    /// out: a motion can open a hover menu or move focus, so a stop or a lost lease from
    /// then on leaves the call `uncertain`.
    async fn pass(&mut self, stage: Stage) -> Result<(), ToolError> {
        act::fresh_lock(self.input.niri).await?;
        let moved = self.waiter.moved().await.map_err(|reason| {
            let sent = if stage == Stage::First {
                "nothing was sent"
            } else {
                "the gesture stopped"
            };
            ToolError::new(ErrorName::UpstreamError, format!("{reason}; {sent}"))
        })?;
        self.still_aimed(stage, &moved)?;
        let view = self.waiter.view();
        if let Some(refused) = policy::refuse_input(self.input.policy, focused_app_id(view)) {
            return Err(refused);
        }
        for id in self.windows {
            let app_id = view
                .windows()
                .get(id)
                .and_then(|window| window.app_id.as_deref());
            if let Some(refused) = policy::refuse_window(self.input.policy, *id, app_id) {
                return Err(refused);
            }
        }
        let desk = self.input.niri.desk;
        if stage == Stage::First {
            desk.sending_input()
        } else {
            desk.fresh()
        }
    }

    /// Whether the gesture's points still stand at `stage` after niri's changes `moved`.
    fn still_aimed(&self, stage: Stage, moved: &Moved) -> Result<(), ToolError> {
        let at_element = !self.windows.is_empty();
        let least = match stage {
            Stage::First | Stage::Unpressed if at_element => Some(Shift::Windows),
            Stage::First => None,
            Stage::Unpressed | Stage::Pressed => Some(Shift::View),
        };
        if least.is_some_and(|least| moved.shift >= least) {
            return Err(moving(stage, moved.shift));
        }
        // Before the press, an element aim already stops on any window change.
        let Some(id) = self.destination else {
            return Ok(());
        };
        if moved.relaid.contains(&id) || !self.waiter.view().windows().contains_key(&id) {
            return Err(ToolError::new(
                ErrorName::ElementUnmappable,
                "niri moved, resized or closed the window of the element the drag ends at, or moved it to another workspace, after the drag began, so the element is no longer where the drag goes",
            ));
        }
        Ok(())
    }
}

/// Why `Gate::pass` refused or stopped a gesture at `stage` when niri reported `moved`.
fn moving(stage: Stage, moved: Shift) -> ToolError {
    if stage == Stage::First {
        return elements::unmappable(Unmappable::Moving);
    }
    let what = if moved == Shift::View {
        "niri switched a workspace or opened or closed the overview after the gesture began"
    } else {
        "niri opened, closed, focused, moved or resized a window after the gesture began, so the element may no longer be under the pointer"
    };
    ToolError::new(ErrorName::ElementUnmappable, what)
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
    /// Whether the held modifiers may have gone out.
    keys_sent: bool,
}

impl Device {
    /// Writes the marker first if `gesture` presses a button, naming the output the
    /// pointer is bound to, which `recover` binds its release to.
    async fn new(
        pointer: Pointer,
        runtime: &RuntimeDir,
        pressing: Pressing,
        output: &str,
        held: Option<Held>,
    ) -> Result<Self, ToolError> {
        let marker = if pressing.button.is_some() || held.is_some() {
            let buttons = pressing.button.into_iter().map(Button::code).collect();
            let mut marker = Marker::pending(pressing.tool, buttons);
            marker.output = Some(output.to_owned());
            marker.keyboard = held.as_ref().map(Held::marker);
            let written = Written::write(runtime, marker)
                .await
                .map_err(|error| marker::unwritten(&error))?;
            Some(written)
        } else {
            None
        };
        Ok(Self {
            pointer: Some(pointer),
            marker,
            held,
            keys_sent: false,
        })
    }

    fn pointer(&mut self) -> Result<&mut Pointer, ToolError> {
        self.pointer
            .as_mut()
            .ok_or_else(|| ToolError::new(ErrorName::UpstreamError, "the pointer is gone"))
    }

    /// Whether any of the gesture's input may have gone out.
    fn sent(&self) -> bool {
        self.keys_sent || self.pointer.as_ref().is_some_and(Pointer::sent)
    }

    /// Sends the planned steps and waits until niri has handled them. `gate` passes the
    /// first input and every part that follows a wait. After a pause, niri handles what
    /// was sent before the gate looks, so the events it wrote meanwhile are there.
    async fn run(&mut self, steps: Vec<Planned>, gate: &mut Gate<'_>) -> Result<(), ToolError> {
        gate.pass(Stage::First).await?;
        if let Some(held) = &mut self.held {
            self.keys_sent = true;
            held.begin().await?;
            gate.pass(Stage::Unpressed).await?;
        }
        for planned in steps {
            match planned {
                Planned::Send(step) => self.pointer()?.send(step)?,
                Planned::Pause(pause) => self.rest(pause, gate).await?,
            }
        }
        self.pointer()?.sync().await?;
        if let Some(held) = &mut self.held {
            held.released().await?;
        }
        Ok(())
    }

    /// Waits `pause`, then until niri has handled what was sent, and has `gate` pass the
    /// part that follows.
    async fn rest(&mut self, pause: Duration, gate: &mut Gate<'_>) -> Result<(), ToolError> {
        tokio::time::sleep(pause).await;
        let pointer = self.pointer()?;
        pointer.sync().await?;
        let stage = if pointer.pressing() {
            Stage::Pressed
        } else {
            Stage::Unpressed
        };
        gate.pass(stage).await
    }

    /// Removes the marker after a gesture niri has handled whole.
    async fn finish(self) -> Result<(), ToolError> {
        self.clear("niri handled the input").await
    }

    /// Removes the marker of a gesture refused before anything of it went out.
    async fn discard(self) -> Result<(), ToolError> {
        self.clear("nothing was sent").await
    }

    /// Releases what a gesture stopped midway still holds, and removes the marker once
    /// niri has handled that, as a drop would, but before the call returns. That runs in a
    /// cleanup task, within the cleanup's deadline, so a call cancelled meanwhile can't cut
    /// it short and leave the marker behind.
    async fn abandon(mut self) -> Result<(), ToolError> {
        let (Some(mut pointer), Some(marker)) = (self.pointer.take(), self.marker.take()) else {
            // No marker: nothing was pressed or held.
            return Ok(());
        };
        pointer.release_all();
        let released = cleanup::spawn(
            cleanup::deadline(),
            release(pointer, self.held.take(), marker),
        );
        released.await.map_err(|error| {
            ToolError::new(
                ErrorName::UpstreamError,
                format!("the release task ended: {error}"),
            )
        })?
    }

    /// Removes the marker; if that fails, the error starts with `done`, what happened to
    /// the input.
    async fn clear(mut self, done: &str) -> Result<(), ToolError> {
        let Some(marker) = self.marker.take() else {
            return Ok(());
        };
        marker.clear().await.map_err(|error| {
            ToolError::new(
                ErrorName::UpstreamError,
                format!(
                    "{done}, but the input-dirty marker couldn't be removed ({error}); actions refuse until the user runs `niri-computer-use recover`"
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
        if keyboard_released && pointer_released && tokio::runtime::Handle::try_current().is_ok() {
            cleanup::spawn(cleanup::deadline(), release(pointer, held, marker));
        }
    }
}

/// Releases what `pointer` and `held` still hold and removes `marker` once niri has
/// handled it and the held modifiers' keymap is proved restored. Otherwise, or if the
/// marker can't be removed, it stays, and blocks input until `recover`.
async fn release(
    mut pointer: Pointer,
    mut held: Option<Held>,
    marker: Written,
) -> Result<(), ToolError> {
    pointer.sync().await?;
    if let Some(held) = &mut held {
        held.released().await?;
    }
    marker.clear().await.map_err(|error| {
        ToolError::new(
            ErrorName::UpstreamError,
            format!("remove the input-dirty marker: {error}"),
        )
    })
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

    /// Serves Noctalia's status for `env`: unlocked to the first read, locked to every
    /// later one.
    fn locks_after_the_first_read(env: &crate::Env) {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

        let socket = crate::noctalia::socket(env).unwrap();
        let listener = tokio::net::UnixListener::bind(socket).unwrap();
        tokio::spawn(async move {
            let mut reply: &[u8] = br#"{"locked": false}"#;
            while let Ok((mut stream, _)) = listener.accept().await {
                let mut request = Vec::new();
                stream.read_to_end(&mut request).await.ok();
                stream.write_all(reply).await.ok();
                reply = br#"{"locked": true}"#;
            }
        });
    }

    // Held modifiers go down after the look before the first input, and niri may take long
    // to handle them. A screen locked meanwhile stops the click at the look after them:
    // no pointer input goes out, the modifiers are released, the marker is removed, and
    // the outcome is uncertain, since the modifiers went out.
    #[tokio::test]
    async fn a_screen_locked_while_held_keys_go_down_stops_the_click() {
        use crate::act::Niri;
        use crate::control::{LockSources, desk::Desk};
        use crate::test_support::{fresh_dir, wayland::Compositor};

        let dir = fresh_dir("pointer-held-lock");
        let niri = Compositor::start(&dir, include_str!("../../tests/fixtures/us-de.xkb"));
        let env = crate::Env {
            wayland_display: Some("fake-noctalia".into()),
            ..niri.env(&dir)
        };
        locks_after_the_first_read(&env);
        let runtime = RuntimeDir::of(&env).unwrap();
        runtime.create().unwrap();
        let socket = niri.niri_socket();
        let events = niri.events();
        let desk = Desk::start(&env);
        let input = Input {
            niri: Niri {
                socket: &socket,
                events: Ok(&events),
                desk: &desk,
                locks: LockSources {
                    env: &env,
                    noctalia: true,
                },
            },
            display: &env.display,
            runtime: &runtime,
            policy: &policy::Loaded::Missing,
            keyboard: Some("native".as_ref()),
            place: crate::test_support::no_programs(),
            a11y: None,
        };
        let outputs = niri::outputs(&socket).await.unwrap();
        let output = &outputs[crate::test_support::wayland::OUTPUT];
        let geometry = output.logical.unwrap();
        let shot = Shot {
            output: output.name.clone(),
            geometry,
            motion_geometry: crate::coords::motion_geometry(output),
            captured: crate::observe::Rect {
                x: 0,
                y: 0,
                width: geometry.width,
                height: geometry.height,
            },
            scale: 1.0,
            width: geometry.width,
            height: geometry.height,
            ..crate::test_support::shot()
        };
        // Past the 250 ms a read of the lock state holds for.
        niri.slow_modifiers(Duration::from_millis(300)).await;
        let click = Gesture::Click {
            at: Spot::Pixel(PX),
            button: Button::Left,
            count: 1,
        };
        let no_element = |id: &str| Err(a11y::not_accessible(format!("no element {id}")));
        let outcome = point(input, Ok(shot), click, &["ctrl".to_owned()], no_element)
            .await
            .unwrap();
        assert_eq!(outcome.observed, Observed::Uncertain);
        let detail = outcome.detail.unwrap_or_default();
        assert!(detail.starts_with("the screen is locked"), "{detail}");
        let seen = niri.seen();
        assert_eq!(seen.buttons, []);
        assert_eq!(seen.last_modifiers().map(|(held, _)| held), Some(0));
        assert!(marker::read(&runtime).is_none());
        std::fs::remove_dir_all(dir).unwrap();
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
