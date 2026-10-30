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

use std::collections::HashSet;
use std::ops::RangeInclusive;
use std::time::Duration;

use tokio::time::Instant;

use super::{
    Input, focused_app_id,
    held::{self, Held},
};
use crate::a11y::model::{self, Unmappable};
use crate::a11y::{self, ElementRef};
use crate::act::{self, Observed, Outcome};
use crate::control::cleanup;
use crate::control::desk::partway;
use crate::control::marker::{self, Marker, Written};
use crate::control::runtime::RuntimeDir;
use crate::coords::{ImagePx, LayoutPt, ProtocolPt};
use crate::error::{CallError, ErrorName, ToolError};
use crate::niri::events::Motion;
use crate::niri::pointer::{Axis, Pointer, Step};
use crate::niri::waiter::{Moved, Shift, View, Waited};
use crate::policy;
use crate::refs::{self, Shot};
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
/// How long the looks within a gesture may spend reading the lock state again after a read
/// that came back stale, all together (`Gate::read_lock`). Meanwhile a drag holds its
/// button, and an app may autoscroll.
const GESTURE_READING: Duration = Duration::from_secs(1);
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
    fn spots(&self) -> Vec<&Spot> {
        match self {
            Self::Move(at) | Self::Click { at, .. } | Self::Scroll { at, .. } => vec![at],
            Self::Drag { from, to, .. } => vec![from, to],
        }
    }

    /// Whether one of its points is a pixel of the screenshot, which only the image says
    /// what it lands on.
    fn has_pixel(&self) -> bool {
        self.spots()
            .into_iter()
            .any(|spot| matches!(spot, Spot::Pixel(_)))
    }

    /// The element refs among its points.
    fn elements(&self) -> Vec<&str> {
        self.spots()
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

impl<P: Copy> Gesture<P> {
    /// Where a drag ends.
    const fn end(&self) -> Option<P> {
        match self {
            Self::Drag { to, .. } => Some(*to),
            Self::Move(_) | Self::Click { .. } | Self::Scroll { .. } => None,
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

/// A point of a gesture: where it is in the layout, and the motion on the output that
/// takes the pointer there.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Aim {
    at: LayoutPt,
    motion: ProtocolPt,
}

/// One thing to do: move the pointer, send another step, or wait.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Planned {
    Move(Aim),
    Send(Step),
    Pause(Duration),
}

/// The steps of `gesture`, given where each of its points is.
fn plan<P>(gesture: &Gesture<P>, aimed: &[Aim]) -> Vec<Planned> {
    let motions = aimed.iter().map(|aim| Planned::Move(*aim));
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
    let connection = input
        .niri
        .events
        .ok()
        .and_then(niri::events::EventStream::connection);
    shot.check(Instant::now(), &outputs, connection)?;
    policy::pointer_output(&outputs, &shot.output)?;
    // Checked again right before the first input (`Gate::pass`); this refuses early.
    let at_pixel = gesture.has_pixel();
    if at_pixel {
        shot.unmoved(&niri::shifted(input.niri.events, &shot.output).await?)?;
    }
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
    let end = destination.zip(located.end()).map(|(window, at)| End {
        window,
        at,
        area: shot.geometry,
        followers: HashSet::new(),
    });
    let now = Instant::now();
    let aimed = located
        .points()
        .into_iter()
        .map(|at| {
            let motion = shot.aim_at(at, now, &outputs, connection)?;
            Ok(Aim { at, motion })
        })
        .collect::<Result<Vec<_>, ToolError>>()?;
    let display = input.display.path().map_err(Clone::clone)?;
    let held = Held::prepare(input, waiter.view(), keys).await?;
    act::fresh_lock(input.niri).await?;
    let pointer = Pointer::bind(display, niri::pid(socket).await?, &shot.output).await?;
    let pressing = Pressing { tool, button };
    let device = Device::new(pointer, input.runtime, pressing, &shot.output, held).await?;
    let gate = Gate {
        waiter: &mut waiter,
        input,
        shot: &shot,
        pixels: at_pixel,
        windows: &windows,
        end,
        reading: GESTURE_READING,
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
        return Err(device.discard(error).await.into());
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
    /// The screenshot the gesture aims through, on whose output the pointer is bound.
    shot: &'a Shot,
    /// Whether the gesture aims at the screenshot's pixels.
    pixels: bool,
    /// The windows of the elements the gesture aims at, none for pixels.
    windows: &'a [u64],
    /// The element a drag ends at, if it ends at one.
    end: Option<End>,
    /// What is left of `GESTURE_READING`.
    reading: Duration,
}

/// The element a drag ends at: its window, and its centre on the output whose layout
/// area is `area`, as the drag aims at it.
#[derive(Debug)]
struct End {
    window: u64,
    at: LayoutPt,
    area: niri_ipc::LogicalOutput,
    /// The windows that opened on its workspace during the drag under the pointer and
    /// clear of its centre, as a tab the drag tears out does.
    followers: HashSet<u64>,
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
    /// are applied first (`Waiter::moved`). Until a button is pressed, a pixel aim is refused
    /// when the windows on the screenshot's output moved since the capture began, or just
    /// before it (`Shot::unmoved`), as at the call's start: preparing the gesture can take
    /// a while. An element aim is refused by any of them that
    /// can change what is shown where since the call's first look, before its elements were
    /// placed, until a button is pressed: a box may no longer be where its element is. Once
    /// something went out, any gesture is stopped by a workspace switch or the overview,
    /// which change the whole view under the pointer. A window moving doesn't stop a drag
    /// that holds a button, as dragging a window by its title bar moves it, unless the
    /// element the drag ends at is no longer where the drag goes (`End::lost`). An app can
    /// change its `app_id` at any time without moving anything, so the deny list is asked
    /// each time about the focused window and the elements' windows as niri reports them
    /// now. The lock state is read again first when the last read is too old, since a
    /// round trip in the gesture can take seconds, and must still be fresh at the end,
    /// however long the look took. A look that kept a read made before it, still fresh
    /// then, may find it too old at the end without anything having been slow: that look
    /// reads again and is made once more. Before the first input, the desk is told input
    /// is going out: a motion can open a hover menu or move focus, so a stop or a lost
    /// lease from then on leaves the call `uncertain`. `pointer` is where the last motion
    /// put the pointer, if one went out.
    async fn pass(&mut self, stage: Stage, pointer: Option<LayoutPt>) -> Result<(), ToolError> {
        let kept = self.input.niri.desk.lock_fresh();
        match self.look(stage, pointer).await {
            Err(error) if kept && error.name == ErrorName::LockStale => {
                self.look(stage, pointer).await
            }
            looked => looked,
        }
    }

    /// One look of `pass`.
    async fn look(&mut self, stage: Stage, pointer: Option<LayoutPt>) -> Result<(), ToolError> {
        self.read_lock(stage).await?;
        let moved = self.waiter.moved().await.map_err(|reason| {
            let sent = if stage == Stage::First {
                "nothing was sent"
            } else {
                "the gesture stopped"
            };
            ToolError::new(ErrorName::UpstreamError, format!("{reason}; {sent}"))
        })?;
        self.still_aimed(stage, &moved, pointer)?;
        let shot = self.shot;
        if stage != Stage::Pressed && self.pixels {
            shot.unmoved(&niri::shifted(self.input.niri.events, &shot.output).await?)?;
        }
        // The last look at niri, so what follows judges its newest answer.
        let outputs = niri::outputs(self.input.niri.socket).await?;
        shot.same_output(&outputs)?;
        policy::pointer_output(&outputs, &shot.output)?;
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
            desk.fresh().map_err(partway)
        }
    }

    /// Reads the lock state again when its last read is too old (`act::fresh_lock`), under
    /// that read's own deadlines. Once something went out, a read that itself took too
    /// long is made again while `reading` lasts, and a read still running when it runs out
    /// is dropped, leaving the lock state stale: stopping drops a drag wherever the pointer
    /// is, while another read only holds the pointer still a little longer, but not for
    /// long. Only those reads count against `reading`, so a slow desktop whose reads come
    /// back fresh doesn't use it up. Before the first input, one slow read refuses the call
    /// with nothing sent, and the agent can simply try again.
    async fn read_lock(&mut self, stage: Stage) -> Result<(), ToolError> {
        let niri = self.input.niri;
        act::fresh_lock(niri).await?;
        if stage == Stage::First || niri.desk.lock_fresh() {
            return Ok(());
        }
        let began = Instant::now();
        let until = began + self.reading;
        while !niri.desk.lock_fresh() {
            let Ok(read) = tokio::time::timeout_at(until, act::fresh_lock(niri)).await else {
                break;
            };
            read?;
        }
        self.reading = self.reading.saturating_sub(began.elapsed());
        Ok(())
    }

    /// Whether the gesture's points still stand at `stage` after niri's changes `moved`,
    /// with the pointer at `pointer`.
    fn still_aimed(
        &mut self,
        stage: Stage,
        moved: &Moved,
        pointer: Option<LayoutPt>,
    ) -> Result<(), ToolError> {
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
        self.end
            .as_mut()
            .and_then(|end| end.lost(self.waiter.view(), moved, pointer))
            .map_or(Ok(()), |why| {
                Err(ToolError::new(ErrorName::ElementUnmappable, why))
            })
    }
}

impl End {
    /// Why the element is no longer where the drag goes after niri's changes `moved`, with
    /// the pointer at `pointer`: its window moved, resized, floated, went to another
    /// workspace or closed, its workspace's active window now hides the floating windows
    /// (`model::hides_floating`), or another floating window on its workspace lies over its
    /// centre after opening or being relaid. niri reports no stacking order, so that window
    /// may be on top. Only one of `followers` that also lies under the pointer doesn't
    /// count. A window that was there before the drag counts even when the drag carries
    /// it, as a title-bar drag does.
    fn lost(
        &mut self,
        view: &View,
        moved: &Moved,
        pointer: Option<LayoutPt>,
    ) -> Option<&'static str> {
        let Some(own) = view.windows().get(&self.window) else {
            return Some(END_MOVED);
        };
        if moved.relaid.contains(&self.window) {
            return Some(END_MOVED);
        }
        let active = own
            .workspace_id
            .and_then(|id| view.workspaces().get(&id)?.active_window_id)
            .and_then(|id| view.windows().get(&id));
        if active.is_some_and(|active| model::hides_floating(&self.area, active)) {
            return Some(END_HIDDEN);
        }
        let tile_of = |id: &u64| {
            if *id == self.window {
                return None;
            }
            let other = view.windows().get(id)?;
            (other.workspace_id == own.workspace_id)
                .then(|| model::floating_tile(&self.area, other))
                .flatten()
        };
        let under_pointer = |tile: &model::LayoutBox| pointer.is_some_and(|at| tile.contains(at));
        self.followers.extend(moved.opened.iter().filter(|id| {
            tile_of(id).is_some_and(|tile| under_pointer(&tile) && !tile.contains(self.at))
        }));
        let covered = moved.relaid.iter().any(|id| {
            tile_of(id).is_some_and(|tile| {
                tile.contains(self.at) && !(self.followers.contains(id) && under_pointer(&tile))
            })
        });
        covered.then_some(END_COVERED)
    }
}

const END_MOVED: &str = "niri moved, resized or closed the window of the element the drag ends at, or moved it to another workspace, after the drag began, so the element is no longer where the drag goes";
const END_HIDDEN: &str = "a tiled window as large as the output, as a fullscreen window is, became or grew into the active window of the workspace of the element the drag ends at after the drag began, so niri hides the element's floating window";
const END_COVERED: &str = "another floating window opened or moved over the element the drag ends at after the drag began, and may be on top of it";

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
    /// Where the last motion put the pointer, if one went out.
    at: Option<LayoutPt>,
    /// The output the pointer is bound to.
    output: String,
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
            at: None,
            output: output.to_owned(),
        })
    }

    fn pointer(&mut self) -> Result<&mut Pointer, ToolError> {
        self.pointer
            .as_mut()
            .ok_or_else(|| ToolError::new(ErrorName::UpstreamError, "the pointer is gone"))
    }

    /// Sends `step`, unless niri changed the bound output since binding, as far as it has
    /// said so far. A release goes out whatever happened: it lands nowhere in particular.
    fn send(&mut self, step: Step) -> Result<(), ToolError> {
        if !matches!(step, Step::Release(_)) {
            self.unchanged()?;
        }
        self.pointer()?.send(step)
    }

    /// Whether niri left the bound output as it was when the pointer was bound to it.
    /// Checked after each `sync`, it tells whether niri handled everything sent so far on
    /// that output.
    fn unchanged(&mut self) -> Result<(), ToolError> {
        let Some(change) = self.pointer()?.output_change()? else {
            return Ok(());
        };
        Err(refs::invalid(
            "output_changed",
            &format!(
                "niri {} output {} after the pointer was bound to it; take a new screenshot",
                change.what(),
                self.output
            ),
        ))
    }

    /// Whether any of the gesture's input may have gone out.
    fn sent(&self) -> bool {
        self.keys_sent || self.pointer.as_ref().is_some_and(Pointer::sent)
    }

    /// Sends the planned steps and waits until niri has handled them. `gate` passes the
    /// first input and every part that follows a wait. After a pause, niri handles what
    /// was sent before the gate looks, so the events it wrote meanwhile are there.
    async fn run(&mut self, steps: Vec<Planned>, gate: &mut Gate<'_>) -> Result<(), ToolError> {
        gate.pass(Stage::First, None).await?;
        if let Some(held) = &mut self.held {
            self.keys_sent = true;
            held.begin().await?;
            gate.pass(Stage::Unpressed, None).await?;
        }
        for planned in steps {
            match planned {
                Planned::Move(aim) => {
                    self.send(Step::Motion(aim.motion))?;
                    self.at = Some(aim.at);
                }
                Planned::Send(step) => self.send(step)?,
                Planned::Pause(pause) => self.rest(pause, gate).await?,
            }
        }
        self.pointer()?.sync().await?;
        self.unchanged()?;
        if let Some(held) = &mut self.held {
            held.released().await?;
        }
        Ok(())
    }

    /// Waits `pause`, then until niri has handled what was sent, and has `gate` pass the
    /// part that follows.
    async fn rest(&mut self, pause: Duration, gate: &mut Gate<'_>) -> Result<(), ToolError> {
        tokio::time::sleep(pause).await;
        self.pointer()?.sync().await?;
        self.unchanged()?;
        let stage = if self.pointer()?.pressing() {
            Stage::Pressed
        } else {
            Stage::Unpressed
        };
        gate.pass(stage, self.at).await
    }

    /// Removes the marker after a gesture niri has handled whole.
    async fn finish(self) -> Result<(), ToolError> {
        self.clear("niri handled the input").await
    }

    /// Removes the marker of a gesture refused with `error` before anything of it went out,
    /// and returns `error`. A marker that can't be removed is added to its detail, since it
    /// blocks the next action, and the refusal's own reason stays.
    async fn discard(self, error: ToolError) -> ToolError {
        match self.clear(&error.detail).await {
            Ok(()) => error,
            Err(unremoved) => ToolError {
                detail: unremoved.detail,
                ..error
            },
        }
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
    /// the input. The removal waits for the marker's lock, so it runs in a cleanup task,
    /// as `abandon`'s does, and a call cancelled meanwhile can't leave the marker behind.
    async fn clear(mut self, done: &str) -> Result<(), ToolError> {
        let Some(marker) = self.marker.take() else {
            return Ok(());
        };
        let cleared = cleanup::spawn(cleanup::deadline(), async move {
            marker
                .clear()
                .await
                .map_err(|error| ToolError::new(ErrorName::UpstreamError, error.to_string()))
        });
        let unremoved = |error: ToolError| {
            ToolError::new(
                ErrorName::UpstreamError,
                format!(
                    "{done}, but the input-dirty marker couldn't be removed ({}); actions refuse until the user runs `niri-computer-use recover`",
                    error.detail
                ),
            )
        };
        cleared
            .await
            .map_err(|error| ToolError::new(ErrorName::UpstreamError, error.to_string()))
            .and_then(|cleared| cleared)
            .map_err(unremoved)
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
    use niri_ipc::{LogicalOutput, Transform};
    use proptest::prelude::*;

    use super::*;
    use crate::test_support::wayland::{self, Aimed, OutputEvent, Pointed};

    fn at(x: u32) -> Aim {
        Aim {
            at: LayoutPt {
                x: f64::from(x),
                y: 0.0,
            },
            motion: ProtocolPt {
                x,
                y: 0,
                x_extent: 1000,
                y_extent: 1000,
            },
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
                Planned::Move(at(1)),
                Planned::Send(Step::Press(273)),
                Planned::Send(Step::Release(273)),
                Planned::Send(Step::Press(273)),
                Planned::Send(Step::Release(273)),
            ]
        );
        assert_eq!(plan(&Gesture::Move(PX), &[at(1)]), [Planned::Move(at(1))]);
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
                Planned::Move(at(0)),
                Planned::Pause(HOVER),
                Planned::Send(Step::Press(272)),
                Planned::Pause(HOVER),
                Planned::Move(at(1)),
                Planned::Pause(DRAG_STEP),
                Planned::Move(at(2)),
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
                Planned::Move(at(1)),
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
                Planned::Move(at(1)),
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

    const UNLOCKED: &str = r#"{"locked": false}"#;
    const LOCKED: &str = r#"{"locked": true}"#;

    /// Serves Noctalia's status for `env`: each read gets the next of `replies`, that late,
    /// and every read after them the last one.
    fn noctalia_answers(env: &crate::Env, replies: Vec<(Duration, &'static str)>) {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

        let socket = crate::noctalia::socket(env).unwrap();
        let listener = tokio::net::UnixListener::bind(socket).unwrap();
        tokio::spawn(async move {
            let mut replies = replies.into_iter();
            let mut last = (Duration::ZERO, UNLOCKED);
            while let Ok((mut stream, _)) = listener.accept().await {
                last = replies.next().unwrap_or(last);
                let (late, reply) = last;
                let mut request = Vec::new();
                stream.read_to_end(&mut request).await.ok();
                tokio::time::sleep(late).await;
                stream.write_all(reply.as_bytes()).await.ok();
            }
        });
    }

    /// What a click with Control held did, its modifiers handled 300 ms late, past the
    /// 250 ms a read of the lock state holds for, while Noctalia gives `replies`.
    struct HeldClick {
        outcome: Outcome,
        seen: wayland::Seen,
        marker_left: bool,
    }

    /// A fake niri with its runtime directory and desk, and a Noctalia that gives
    /// `replies` (`noctalia_answers`).
    struct Bench {
        dir: std::path::PathBuf,
        niri: wayland::Compositor,
        env: crate::Env,
        runtime: RuntimeDir,
        socket: niri::Socket,
        events: niri::events::EventStream,
        desk: crate::control::desk::Desk,
    }

    impl Bench {
        fn start(name: &str, replies: Vec<(Duration, &'static str)>) -> Self {
            Self::start_with(name, replies, vec![wayland::output()])
        }

        /// As `start`, with `outputs` as niri's outputs.
        fn start_with(
            name: &str,
            replies: Vec<(Duration, &'static str)>,
            outputs: Vec<niri_ipc::Output>,
        ) -> Self {
            use crate::test_support::{fresh_dir, wayland::Compositor};

            let dir = fresh_dir(name);
            let keymap = include_str!("../../tests/fixtures/us-de.xkb");
            let niri = Compositor::start_with(&dir, keymap, outputs);
            let env = crate::Env {
                wayland_display: Some("fake-noctalia".into()),
                ..niri.env(&dir)
            };
            noctalia_answers(&env, replies);
            let runtime = RuntimeDir::of(&env).unwrap();
            runtime.create().unwrap();
            let socket = niri.niri_socket();
            let events = niri.events();
            let desk = crate::control::desk::Desk::start(&env);
            Self {
                dir,
                niri,
                env,
                runtime,
                socket,
                events,
                desk,
            }
        }

        fn input(&self) -> Input<'_> {
            Input {
                niri: act::Niri {
                    socket: &self.socket,
                    events: Ok(&self.events),
                    desk: &self.desk,
                    locks: crate::control::LockSources {
                        env: &self.env,
                        noctalia: true,
                    },
                },
                display: &self.env.display,
                runtime: &self.runtime,
                policy: &policy::Loaded::Missing,
                a11y: None,
            }
        }
    }

    /// A screenshot ref of the whole of the fake's output at scale 1, taken now and settled
    /// then, since the event stream counts its initial state as a move.
    async fn whole_output(bench: &Bench) -> Shot {
        shot_of(bench, wayland::OUTPUT, 1).await
    }

    /// As `whole_output`, of output `name`, with `scale` image pixels per logical pixel.
    async fn shot_of(bench: &Bench, name: &str, scale: u32) -> Shot {
        bench.events.desktop().await.unwrap();
        let outputs = niri::outputs(&bench.socket).await.unwrap();
        let output = &outputs[name];
        let geometry = output.logical.unwrap();
        Shot {
            output: output.name.clone(),
            geometry,
            motion_geometry: crate::coords::motion_geometry(output),
            captured: crate::observe::Rect {
                x: geometry.x,
                y: geometry.y,
                width: geometry.width,
                height: geometry.height,
            },
            scale: f64::from(scale),
            width: geometry.width * scale,
            height: geometry.height * scale,
            steady_since: Some(Instant::now()),
            ..crate::test_support::shot()
        }
    }

    async fn click_holding_ctrl(name: &str, replies: Vec<(Duration, &'static str)>) -> HeldClick {
        let bench = Bench::start(name, replies);
        let (niri, runtime) = (&bench.niri, &bench.runtime);
        let input = bench.input();
        let shot = whole_output(&bench).await;
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
        let clicked = HeldClick {
            outcome,
            seen: niri.seen(),
            marker_left: marker::read(runtime).is_some(),
        };
        std::fs::remove_dir_all(&bench.dir).unwrap();
        clicked
    }

    /// Once the pointer's connection to the display is held, has niri report window 1
    /// resized, waits until the event stream has it, and serves the connection.
    async fn resize_once_held(bench: &Bench) {
        let niri = &bench.niri;
        while !niri.seen().holding {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let layout = niri_ipc::WindowLayout {
            pos_in_scrolling_layout: Some((1, 1)),
            tile_size: (960.0, 1080.0),
            window_size: (960, 1080),
            tile_pos_in_workspace_view: None,
            window_offset_in_tile: (0.0, 0.0),
        };
        let resized = Instant::now();
        niri.event(niri_ipc::Event::WindowLayoutsChanged {
            changes: vec![(1, layout)],
        });
        let output = wayland::OUTPUT;
        let shifted = async || bench.events.shifted(output).await.unwrap().shifted;
        while shifted().await.is_none_or(|shifted| shifted.at < resized) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        niri.serve_held().await;
    }

    // The bug: a pixel aim was checked against niri's windows once, as the call began, and
    // the look right before the first input let any window change through for a gesture of
    // pixels only, so a window that moved while the call got ready, here while niri took
    // its time with the pointer's connection, didn't stop the click.
    #[tokio::test]
    async fn a_pixel_click_is_refused_when_a_window_moves_while_it_gets_ready() {
        let bench = Bench::start("pointer-moved-ready", Vec::new());
        let niri = &bench.niri;
        let shot = whole_output(&bench).await;
        niri.hold_client().await;
        let click = Gesture::Click {
            at: Spot::Pixel(PX),
            button: Button::Left,
            count: 1,
        };
        let no_element = |id: &str| Err(a11y::not_accessible(format!("no element {id}")));
        let clicking = point(bench.input(), Ok(shot), click, &[], no_element);
        let both = async { tokio::join!(clicking, resize_once_held(&bench)).0 };
        let clicked = tokio::time::timeout(Duration::from_secs(10), Box::pin(both))
            .await
            .unwrap();
        let Err(CallError::Tool(error)) = clicked else {
            panic!("the click wasn't refused: {clicked:?}");
        };
        assert_eq!(error.name, ErrorName::RefInvalid);
        let moved = "layout_changed: niri moved or resized window 1 since the screenshot";
        assert!(error.detail.starts_with(moved), "{}", error.detail);
        assert_eq!(niri.seen().buttons, []);
        assert!(marker::read(&bench.runtime).is_none());
        std::fs::remove_dir_all(&bench.dir).unwrap();
    }

    // Held modifiers go down after the look before the first input, and niri may take long
    // to handle them. A screen locked meanwhile stops the click at the look after them:
    // no pointer input goes out, the modifiers are released, the marker is removed, and
    // the outcome is uncertain, since the modifiers went out.
    #[tokio::test]
    async fn a_screen_locked_while_held_keys_go_down_stops_the_click() {
        let replies = vec![(Duration::ZERO, UNLOCKED), (Duration::ZERO, LOCKED)];
        let clicked = click_holding_ctrl("pointer-held-lock", replies).await;
        assert_eq!(clicked.outcome.observed, Observed::Uncertain);
        let detail = clicked.outcome.detail.unwrap_or_default();
        assert!(detail.starts_with("the screen is locked"), "{detail}");
        assert_eq!(clicked.seen.buttons, []);
        assert_eq!(clicked.seen.last_modifiers().map(|(held, _)| held), Some(0));
        assert!(!clicked.marker_left);
    }

    // Once input went out, stopping leaves a gesture half done, so a read of the lock state
    // that itself took longer than the 250 ms a read holds for is made again: one slow
    // read in the middle of a gesture doesn't stop it.
    #[tokio::test]
    async fn one_slow_lock_read_after_input_went_out_is_read_again() {
        let slow = Duration::from_millis(300);
        let replies = vec![
            (Duration::ZERO, UNLOCKED),
            (slow, UNLOCKED),
            (Duration::ZERO, UNLOCKED),
        ];
        let clicked = click_holding_ctrl("pointer-held-slow-read", replies).await;
        assert_eq!(
            clicked.outcome.observed,
            Observed::Sent,
            "{:?}",
            clicked.outcome.detail
        );
        assert_eq!(clicked.seen.buttons, [(272, true), (272, false)]);
        assert!(!clicked.marker_left);
    }

    // A gesture that stops partway because every read of the lock state was too slow has
    // already sent part of itself, so its detail says what happened and doesn't tell the
    // agent to try once more, as a refusal before any input does.
    #[tokio::test]
    async fn a_gesture_stopped_by_slow_lock_reads_isnt_called_safe_to_repeat() {
        let slow = Duration::from_millis(300);
        let replies = vec![(Duration::ZERO, UNLOCKED), (slow, UNLOCKED)];
        let clicked = click_holding_ctrl("pointer-held-stale", replies).await;
        assert_eq!(clicked.outcome.observed, Observed::Uncertain);
        let detail = clicked.outcome.detail.unwrap_or_default();
        assert!(
            detail.starts_with(
                "the lock state was last read over 250 ms before the call's next part"
            ),
            "{detail}"
        );
        assert!(!detail.contains("try once more"), "{detail}");
        assert!(detail.contains("may have taken effect"), "{detail}");
        assert_eq!(clicked.seen.buttons, []);
        assert!(!clicked.marker_left);
    }

    /// What a gate on `bench` needs: a waiter on its niri, and its input.
    async fn gate_on(bench: &Bench) -> (niri::waiter::Waiter, Input<'_>) {
        let waiter = niri::waiter(Ok(&bench.events)).await.unwrap();
        (waiter, bench.input())
    }

    // On a loaded desktop every look of a drag may find the lock state stale and need one
    // read, which comes back fresh but slowly. Those reads don't count against the second
    // for reading again, so a drag of many looks isn't stopped partway.
    #[tokio::test]
    async fn reads_that_come_back_fresh_never_use_up_a_gestures_second() {
        let slow = Duration::from_millis(150);
        let bench = Bench::start("pointer-gate-fresh", vec![(slow, UNLOCKED)]);
        let (mut waiter, input) = gate_on(&bench).await;
        let shot = whole_output(&bench).await;
        let mut gate = Gate {
            waiter: &mut waiter,
            input,
            shot: &shot,
            pixels: false,
            windows: &[],
            end: None,
            reading: GESTURE_READING,
        };
        for look in 0..10 {
            bench
                .desk
                .lock_read(Instant::now() - Duration::from_secs(1));
            gate.read_lock(Stage::Pressed).await.unwrap();
            assert!(bench.desk.lock_fresh(), "look {look}");
        }
        std::fs::remove_dir_all(&bench.dir).unwrap();
    }

    // The user's stop key may set the stop flag after a gesture's last look and before the
    // desk's stop watcher cancels it. Each part of the gesture reads the flag itself: the
    // first input, the pointer input after the held modifiers, and each drag step while
    // the button is held.
    #[tokio::test]
    async fn every_part_of_a_gesture_checks_the_stop_flag_itself() {
        let bench = Bench::start("pointer-gate-stop", vec![(Duration::ZERO, UNLOCKED)]);
        let (mut waiter, input) = gate_on(&bench).await;
        let shot = whole_output(&bench).await;
        let mut gate = Gate {
            waiter: &mut waiter,
            input,
            shot: &shot,
            pixels: false,
            windows: &[],
            end: None,
            reading: GESTURE_READING,
        };
        bench.runtime.stop().unwrap();
        for stage in [Stage::First, Stage::Unpressed, Stage::Pressed] {
            bench.desk.lock_read(Instant::now());
            let error = gate.pass(stage, None).await.unwrap_err();
            assert_eq!(
                error.name,
                ErrorName::Stopped,
                "{stage:?}: {}",
                error.detail
            );
        }
        std::fs::remove_dir_all(&bench.dir).unwrap();
    }

    // A look keeps a lock read under 250 ms old, which can turn 250 ms while the look
    // catches up on niri's events. That says nothing about how slow the desktop is: the
    // look reads the lock state again and looks once more, before the first input and
    // during a drag alike, rather than refusing the call or dropping the drag.
    #[tokio::test]
    async fn a_lock_read_that_ages_out_during_a_look_is_made_again() {
        let bench = Bench::start("pointer-gate-aged", vec![(Duration::ZERO, UNLOCKED)]);
        let (mut waiter, input) = gate_on(&bench).await;
        let shot = whole_output(&bench).await;
        let mut gate = Gate {
            waiter: &mut waiter,
            input,
            shot: &shot,
            pixels: false,
            windows: &[],
            end: None,
            reading: GESTURE_READING,
        };
        for stage in [Stage::First, Stage::Pressed] {
            let passed = pass_aging(&mut gate, &bench.desk, stage).await;
            assert!(passed.is_ok(), "{stage:?}: {passed:?}");
            assert!(bench.desk.lock_fresh(), "{stage:?}");
        }
        std::fs::remove_dir_all(&bench.dir).unwrap();
    }

    /// `gate`'s pass at `stage`, whose look starts with a fresh lock read on `desk` that
    /// becomes one begun a second ago once the look yields to catch up on niri's events.
    async fn pass_aging(
        gate: &mut Gate<'_>,
        desk: &crate::control::desk::Desk,
        stage: Stage,
    ) -> Result<(), ToolError> {
        desk.lock_read(Instant::now());
        let aging = desk.clone();
        tokio::spawn(async move { aging.lock_read(Instant::now() - Duration::from_secs(1)) });
        gate.pass(stage, None).await
    }

    // While a gesture reads the lock state again after a read that came back stale, a drag
    // holds its button and an app may autoscroll under it, so those reads stop after a
    // second in all, however long each one would take. The look's first read keeps its
    // own deadline.
    #[tokio::test]
    async fn reading_again_after_a_stale_read_stops_after_a_second() {
        let slow = Duration::from_millis(1500);
        let bench = Bench::start("pointer-gate-slow", vec![(slow, UNLOCKED)]);
        let (mut waiter, input) = gate_on(&bench).await;
        let shot = whole_output(&bench).await;
        let mut gate = Gate {
            waiter: &mut waiter,
            input,
            shot: &shot,
            pixels: false,
            windows: &[],
            end: None,
            reading: GESTURE_READING,
        };
        bench
            .desk
            .lock_read(Instant::now() - Duration::from_secs(1));
        let began = Instant::now();
        let read = tokio::time::timeout(Duration::from_secs(10), gate.read_lock(Stage::Pressed));
        read.await.unwrap().unwrap();
        // The first read's 1.5 s and the second of reading again, not a third read.
        let took = began.elapsed();
        assert!(took < Duration::from_millis(3500), "{took:?}");
        assert!(!bench.desk.lock_fresh());
        assert_eq!(gate.reading, Duration::ZERO);
        std::fs::remove_dir_all(&bench.dir).unwrap();
    }

    /// A device for a left click on `bench`'s output, its marker written.
    async fn click_device(bench: &Bench) -> Result<Device, ToolError> {
        let output = wayland::OUTPUT;
        let pointer = Pointer::bind(bench.niri.display(), std::process::id(), output)
            .await
            .unwrap();
        let pressing = Pressing {
            tool: "click",
            button: Some(Button::Left),
        };
        Device::new(pointer, &bench.runtime, pressing, output, None).await
    }

    // The user's stop key sets the stop flag while a click's last look reads niri's events,
    // before the desk's stop watcher has seen it. Nothing of the click goes out, and the
    // call is refused as stopped rather than cut after sending.
    #[tokio::test]
    async fn a_stop_set_while_the_last_gate_reads_events_prevents_the_click() {
        let bench = Bench::start("pointer-stop-gate", vec![(Duration::ZERO, UNLOCKED)]);
        let session = crate::test_support::session(1);
        bench
            .desk
            .acquire(&session, "review", async { None }, async { None })
            .await
            .unwrap();
        let input = bench.input();
        let window = niri::waiter::tests::window(1, Some("test"), 1, true);
        let (mut waiter, updates) =
            niri::waiter::tests::waiter(niri::waiter::tests::view(vec![window]), 0);
        let notify = std::sync::Arc::new(tokio::sync::Notify::new());
        let notified = std::sync::Arc::clone(&notify);
        let runtime = bench.runtime.clone();
        let stopper = tokio::spawn(async move {
            notified.notified().await;
            runtime.stop().unwrap();
            updates
                .send(niri::waiter::Update::Event(
                    1,
                    Box::new(niri_ipc::Event::WindowUrgencyChanged {
                        id: 1,
                        urgent: true,
                    }),
                ))
                .unwrap();
        });
        let work = async {
            let device = click_device(&bench).await?;
            bench.desk.lock_read(Instant::now());
            let shot = whole_output(&bench).await;
            let gate = Gate {
                waiter: &mut waiter,
                input,
                shot: &shot,
                pixels: false,
                windows: &[],
                end: None,
                reading: GESTURE_READING,
            };
            notify.notify_one();
            let steps = plan(
                &Gesture::Click {
                    at: PX,
                    button: Button::Left,
                    count: 1,
                },
                &[at(1)],
            );
            send(device, gate, steps).await?;
            Ok::<_, CallError>(())
        };
        let acted = bench
            .desk
            .act(&session, async { None }, work, |worked| async { worked });
        let ended = Box::pin(acted).await;
        stopper.await.unwrap();
        tokio::time::sleep(Duration::from_millis(30)).await;
        let buttons = bench.niri.seen().buttons;
        // The gate read niri's events after the stop flag was set, so it looked after.
        assert!(waiter.view().windows()[&1].is_urgent);
        assert!(
            buttons.is_empty(),
            "input after the stop flag was set: {buttons:?}; result={ended:?}"
        );
        assert!(
            matches!(&ended, Err(CallError::Tool(error)) if error.name == ErrorName::Stopped),
            "{ended:?}"
        );
        std::fs::remove_dir_all(&bench.dir).unwrap();
    }

    // A gesture refused before any input, here because niri's event stream reset, removes
    // its marker. When that fails too, the error still says why the gesture was refused,
    // besides that the marker stayed and what the user does about it.
    #[tokio::test]
    async fn a_marker_that_cant_be_removed_keeps_the_original_failure() {
        let bench = Bench::start("pointer-unremoved", vec![(Duration::ZERO, UNLOCKED)]);
        let input = bench.input();
        let device = click_device(&bench).await.unwrap();
        let lock = std::fs::File::options()
            .read(true)
            .write(true)
            .open(bench.runtime.path().join("input-dirty.lock"))
            .unwrap();
        lock.try_lock().unwrap();
        let (mut waiter, updates) = niri::waiter::tests::waiter(View::default(), 0);
        updates.send(niri::waiter::Update::Reset(2)).unwrap();
        bench.desk.lock_read(Instant::now());
        let shot = whole_output(&bench).await;
        let gate = Gate {
            waiter: &mut waiter,
            input,
            shot: &shot,
            pixels: false,
            windows: &[],
            end: None,
            reading: GESTURE_READING,
        };
        let error = send(device, gate, Vec::new()).await.unwrap_err();
        let CallError::Tool(error) = error else {
            panic!("unexpected error: {error:?}")
        };
        assert!(
            error.detail.starts_with("niri's event stream disconnected")
                && error.detail.contains("nothing was sent")
                && error
                    .detail
                    .contains("input-dirty marker couldn't be removed")
                && error.detail.contains("`niri-computer-use recover`"),
            "{}",
            error.detail
        );
        drop(lock);
        std::fs::remove_dir_all(&bench.dir).unwrap();
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

    /// An output named `name`, enabled at `logical` or turned off, whose mode is its
    /// untransformed logical size times its scale.
    fn monitor(name: &str, logical: Option<LogicalOutput>) -> niri_ipc::Output {
        let mode = logical.map(|logical| {
            let (width, height) = match logical.transform {
                Transform::_90 | Transform::_270 | Transform::Flipped90 | Transform::Flipped270 => {
                    (logical.height, logical.width)
                }
                Transform::Normal
                | Transform::_180
                | Transform::Flipped
                | Transform::Flipped180 => (logical.width, logical.height),
            };
            crate::test_support::output_mode::from_logical(width, height, logical.scale)
        });
        niri_ipc::Output {
            name: name.to_owned(),
            current_mode: mode.map(|_| 0),
            modes: mode.into_iter().collect(),
            logical,
            ..wayland::output()
        }
    }

    /// `width`×`height` logical pixels at `(x, y)`, at scale 1 and transform `Normal`.
    const fn area(x: i32, y: i32, width: u32, height: u32) -> LogicalOutput {
        LogicalOutput {
            x,
            y,
            width,
            height,
            scale: 1.0,
            transform: Transform::Normal,
        }
    }

    fn no_element(id: &str) -> Result<ElementRef, ToolError> {
        Err(a11y::not_accessible(format!("no element {id}")))
    }

    // The bug: a pointer bound to the first output, or a motion encoded for another
    // output's geometry, lands a click on a second monitor somewhere else. The expected
    // request is worked out by hand: pixel (639, 101) of an image at 2 pixels per logical
    // pixel is (319.75, 50.75) on the output, in thousandths of its 1280×720.
    #[tokio::test]
    async fn a_click_on_a_second_output_lands_where_its_screenshot_shows() {
        let left = LogicalOutput {
            scale: 2.0,
            ..area(-1280, 100, 1280, 720)
        };
        let outputs = vec![
            monitor("DP-1", Some(area(0, 0, 1920, 1080))),
            monitor("HDMI-A-1", Some(left)),
        ];
        let bench = Bench::start_with("pointer-second-output", Vec::new(), outputs);
        let shot = shot_of(&bench, "HDMI-A-1", 2).await;
        let click = Gesture::Click {
            at: Spot::Pixel(ImagePx { x: 639, y: 101 }),
            button: Button::Left,
            count: 1,
        };
        let outcome = point(bench.input(), Ok(shot), click, &[], no_element)
            .await
            .unwrap();
        assert_eq!(outcome.observed, Observed::Sent, "{:?}", outcome.detail);
        let on = |input| Aimed {
            output: Some("HDMI-A-1".to_owned()),
            generation: Some(0),
            synced_since_change: false,
            input,
        };
        assert_eq!(
            bench.niri.seen().aimed,
            [
                on(Pointed::Motion(319_750, 50_750, 1_280_000, 720_000)),
                on(Pointed::Press(272))
            ]
        );
        std::fs::remove_dir_all(&bench.dir).unwrap();
    }

    /// The output a gesture aims at, and the one beside it.
    const BOUND: &str = "DP-1";
    const OTHER: &str = "HDMI-A-1";

    fn desk_outputs() -> Vec<niri_ipc::Output> {
        vec![
            monitor(BOUND, Some(area(0, 0, 1920, 1080))),
            monitor(OTHER, Some(area(1920, 0, 1280, 720))),
        ]
    }

    /// What niri does to an output: it reports each change over IPC and to the `wl_output`s
    /// bound to it at once.
    #[derive(Debug, Clone, Copy)]
    enum Change {
        /// Gone from IPC, as a disconnected monitor is.
        Unplug,
        /// Listed without `logical`, as `niri msg output … off` leaves it.
        Off,
        /// Unplugged and plugged back as it was, which only Wayland shows.
        Replug,
        Rescale,
        Move,
        Rotate,
        /// Another output appears.
        Add,
    }

    /// niri's outputs after `change` to output `name`.
    fn changed(name: &str, change: Change) -> OutputEvent {
        let mut outputs = desk_outputs();
        let changing = outputs
            .iter_mut()
            .find(|output| output.name == name)
            .unwrap();
        let logical = changing.logical.unwrap();
        match change {
            Change::Unplug => outputs.retain(|output| output.name != name),
            Change::Off => *changing = monitor(name, None),
            Change::Replug => return OutputEvent::Replug(name.to_owned()),
            Change::Rescale => {
                let half = LogicalOutput {
                    width: logical.width / 2,
                    height: logical.height / 2,
                    scale: 2.0,
                    ..logical
                };
                *changing = monitor(name, Some(half));
            }
            Change::Move => *changing = monitor(name, Some(LogicalOutput { y: 50, ..logical })),
            Change::Rotate => {
                let turned = LogicalOutput {
                    width: logical.height,
                    height: logical.width,
                    transform: Transform::_90,
                    ..logical
                };
                *changing = monitor(name, Some(turned));
            }
            Change::Add => outputs.push(monitor("DP-2", Some(area(0, 1080, 800, 600)))),
        }
        OutputEvent::Set(outputs)
    }

    fn any_gesture() -> impl Strategy<Value = Gesture<Spot>> {
        let pixel = (0..1920_u32, 0..1080_u32).prop_map(|(x, y)| Spot::Pixel(ImagePx { x, y }));
        prop_oneof![
            pixel.clone().prop_map(Gesture::Move),
            (pixel.clone(), 1..=2_u8).prop_map(|(at, count)| Gesture::Click {
                at,
                button: Button::Left,
                count
            }),
            (pixel.clone(), pixel.clone()).prop_map(|(from, to)| Gesture::Drag {
                from,
                to,
                button: Button::Left
            }),
            (pixel, 1..=2_i32).prop_map(|(at, notches_y)| Gesture::Scroll {
                at,
                notches_x: 0,
                notches_y
            }),
        ]
    }

    fn any_change() -> impl Strategy<Value = (&'static str, Change)> {
        let bound = prop::sample::select(vec![
            Change::Unplug,
            Change::Off,
            Change::Replug,
            Change::Rescale,
            Change::Move,
            Change::Rotate,
        ]);
        let other = prop::sample::select(vec![
            Change::Unplug,
            Change::Off,
            Change::Replug,
            Change::Rescale,
            Change::Add,
        ]);
        prop_oneof![
            bound.prop_map(|change| (BOUND, change)),
            other.prop_map(|change| (OTHER, change)),
        ]
    }

    /// When niri changes an output, as a gesture goes.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum When {
        BeforeTheCall,
        /// After the call read niri's outputs, while the display holds its first
        /// connection, the held modifiers' or else the pointer's.
        WhileConnecting,
        /// Right before niri handles the virtual pointer's request with this number,
        /// counted from 1 with the one creating it.
        AtPointerRequest(usize),
    }

    /// The virtual pointer requests `gesture` makes when nothing stops it: one creating
    /// the pointer, two for each motion, press and release (a drag makes `DRAG_STEPS + 1`
    /// motions), and three for each wheel turn.
    /// The test checks the change it schedules at one of them happened.
    fn requests(gesture: &Gesture<Spot>) -> usize {
        1 + match gesture {
            Gesture::Move(_) => 2,
            Gesture::Click { count, .. } => 2 + 4 * usize::from(*count),
            Gesture::Drag { .. } => 2 * (usize::try_from(DRAG_STEPS).unwrap() + 1 + 2),
            Gesture::Scroll { .. } => 2 + 3,
        }
    }

    /// A gesture, and when an output changes as it goes: mostly at one of its requests.
    fn any_gesture_and_when() -> impl Strategy<Value = (Gesture<Spot>, When)> {
        any_gesture().prop_flat_map(|gesture| {
            let when = prop_oneof![
                1 => Just(When::BeforeTheCall),
                1 => Just(When::WhileConnecting),
                4 => (1..=requests(&gesture)).prop_map(When::AtPointerRequest),
            ];
            (Just(gesture), when)
        })
    }

    /// One gesture on a desk of two outputs, while niri makes `change` to output `name`
    /// `when` says.
    async fn gesture_while_an_output_changes(
        gesture: Gesture<Spot>,
        keys: &[String],
        (name, change): (&str, Change),
        when: When,
    ) {
        let bench = Bench::start_with("pointer-output-change", Vec::new(), desk_outputs());
        let niri = &bench.niri;
        let shot = shot_of(&bench, BOUND, 1).await;
        let event = changed(name, change);
        match when {
            When::BeforeTheCall => niri.outputs(event.clone()).await,
            When::WhileConnecting => niri.hold_client().await,
            When::AtPointerRequest(request) => {
                niri.outputs_at_pointer_request(request, event.clone())
                    .await;
            }
        }
        let changing = async {
            if when == When::WhileConnecting {
                change_once_held(niri, event).await;
            }
        };
        let pointing = point(bench.input(), Ok(shot), gesture, keys, no_element);
        let both = async { tokio::join!(pointing, changing).0 };
        let result = tokio::time::timeout(Duration::from_secs(10), Box::pin(both))
            .await
            .unwrap();
        // A gesture stopped midway releases and removes the marker in a task of its own.
        let released = Instant::now() + Duration::from_secs(2);
        while marker::read(&bench.runtime).is_some() && Instant::now() < released {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(marker::read(&bench.runtime).is_none(), "{result:?}");
        let expected = Expected {
            other: name == OTHER,
            caught_by_the_screenshot: name == BOUND
                && when == When::BeforeTheCall
                && !matches!(change, Change::Replug),
        };
        let seen = niri.seen();
        if let When::AtPointerRequest(request) = when {
            assert!(
                seen.pointer_requests >= request,
                "the change never came: {seen:?}"
            );
        }
        expected.hold(&result, &seen);
        drop(seen);
        std::fs::remove_dir_all(&bench.dir).unwrap();
    }

    /// Once the display holds a connection, makes `event`'s change and serves it.
    async fn change_once_held(niri: &wayland::Compositor, event: OutputEvent) {
        while !niri.seen().holding {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        niri.outputs(event).await;
        niri.serve_held().await;
    }

    /// What a gesture during an output change must have done, besides what any must.
    struct Expected {
        /// The change was to the other output, which the pointer isn't bound to.
        other: bool,
        /// The change came before the call to an output as niri reports it.
        caught_by_the_screenshot: bool,
    }

    impl Expected {
        /// Asserts what any gesture must have done, held modifiers and all: it may be
        /// refused, stopped or sent, but it leaves nothing pressed, refuses only with
        /// nothing sent, sends nothing more to an output no longer as captured once a round
        /// trip could have shown the change, and is never reported sent after niri handled
        /// any of it on such an output. Then what `self` adds.
        fn hold(&self, result: &Result<Outcome, CallError>, seen: &wayland::Seen) {
            let observed = result.as_ref().map(|outcome| outcome.observed);
            assert_eq!(seen.held_buttons(), Vec::<u32>::new(), "{result:?}");
            assert_eq!(seen.held_keys(), Vec::<u32>::new(), "{result:?}");
            let bound = |aimed: &Aimed| aimed.output.as_deref() == Some(BOUND);
            assert!(seen.aimed.iter().all(bound), "{:?}", seen.aimed);
            let as_captured = seen.aimed.iter().all(|aimed| aimed.generation == Some(0));
            let sent_once_seen =
                |aimed: &&Aimed| aimed.generation != Some(0) && aimed.synced_since_change;
            let late: Vec<&Aimed> = seen.aimed.iter().filter(sent_once_seen).collect();
            assert!(late.is_empty(), "{late:?}: {result:?}");
            match observed {
                Err(_) => assert!(seen.aimed.is_empty() && seen.keys.is_empty(), "{result:?}"),
                Ok(Observed::Sent) => assert!(as_captured, "{:?}: {result:?}", seen.aimed),
                Ok(observed) => assert_eq!(observed, Observed::Uncertain, "{result:?}"),
            }
            if self.other {
                assert_eq!(observed.ok(), Some(Observed::Sent), "{result:?}");
            }
            if self.caught_by_the_screenshot {
                assert!(result.is_err(), "{result:?}");
            }
        }
    }

    // A same-name replug shows only on Wayland: niri's IPC answer stays as it was, so only
    // the pointer's own connection can stop the drag's later motions, which niri would map
    // over the box around the other outputs.
    #[tokio::test]
    async fn a_drag_stops_once_niri_replugs_its_output() {
        let drag = Gesture::Drag {
            from: Spot::Pixel(ImagePx { x: 100, y: 100 }),
            to: Spot::Pixel(ImagePx { x: 900, y: 500 }),
            button: Button::Left,
        };
        let replug = (BOUND, Change::Replug);
        gesture_while_an_output_changes(drag, &[], replug, When::AtPointerRequest(8)).await;
    }

    proptest! {
        #![proptest_config(crate::test_support::properties(40))]

        #[test]
        fn an_output_change_never_leaves_input_reported_sent_or_held(
            (gesture, when) in any_gesture_and_when(),
            held in any::<bool>(),
            change in any_change(),
        ) {
            let keys = if held { vec!["ctrl".to_owned()] } else { Vec::new() };
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(gesture_while_an_output_changes(gesture, &keys, change, when));
        }
    }
}
