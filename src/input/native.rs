//! Native per-key input. A dirty marker covers every possible press until release is
//! acknowledged and, after an extended keymap, niri has sent the compositor's map back.
//! SIGKILL requires recover; destruction alone releases nothing in niri. A paste's
//! `Aftercare` is armed right before the first stroke and told the key went out once the
//! release is acknowledged, before the marker comes off; a release niri never acknowledged
//! drops it, so the keeper keeps the pasted text.

use std::time::Duration;

use tokio::time::Instant;

use super::keyboard::{Expect, Sent, Typing, check_expect, ended, partly, typed_so_far};
use super::keymap::{Key, Plan, plan};
use super::paste::Aftercare;
use super::{Input, focused_app_id};
use crate::act::{self, Observed, Outcome};
use crate::control::cleanup;
use crate::control::desk::Desk;
use crate::control::marker::{self, Marker, Native, Written};
use crate::error::{CallError, ErrorName, ToolError};
use crate::niri::{
    self, Socket,
    keyboard::Keyboard,
    waiter::{View, Waited, Waiter},
};
use crate::policy;

/// Past how long a key loop that found the stop flag set stops waiting for the desk's stop
/// watcher, which reads the flag on every change in the runtime directory and once a
/// second.
const STOP_SEEN: Duration = Duration::from_secs(2);

pub(super) async fn type_input(
    input: Input<'_>,
    typing: Typing,
    expect: Expect,
    aftercare: &mut Option<Aftercare>,
) -> Result<Outcome, CallError> {
    let waiter = niri::waiter(input.niri.events).await?;
    type_seen(input, waiter, typing, expect, aftercare).await
}

/// `type_input` with niri's events from `waiter`. Each part of the plan's text goes out
/// with its own map, uploaded right before its first key, once that key may go out.
async fn type_seen(
    input: Input<'_>,
    mut waiter: Waiter,
    typing: Typing,
    expect: Expect,
    aftercare: &mut Option<Aftercare>,
) -> Result<Outcome, CallError> {
    let focus = check_expect(&expect, waiter.view())?;
    if let Some(refused) = policy::refuse_input(input.policy, focused_app_id(waiter.view())) {
        return Err(refused.into());
    }
    let (keyboard, group, plan) = bind(input, waiter.view(), &typing).await?;
    let revision = keyboard.revision();
    let before = waiter.view().focused_window();
    let mut device = Device::new(keyboard, input, &typing, plan.keys(), group).await?;
    let call = Call {
        typing: &typing,
        budget: Budget::start(plan.keys().count()),
        marks: aftercare.is_none(),
        before,
        group,
    };
    let decisive = decisive(&typing, plan.keys().count());
    let mut sent = Sent::default();
    for (index, (extended, key)) in plan.strokes().enumerate() {
        let left = checked_before(input, &mut waiter, before, group, Some(index) == decisive)
            .await
            .map_err(|error| partly(error, &typing, sent))?;
        if let Some(outcome) = left {
            device
                .finish(waiter.view().keyboard_group())
                .await
                .map_err(|error| partly(error, &typing, sent))?;
            return Ok(ended(outcome, focus, &typing, sent));
        }
        may_press(input, waiter.view(), &call, sent).await?;
        if let Some(map) = extended {
            device
                .extend(&map)
                .map_err(|error| partly(error, &typing, sent))?;
        }
        if device.commit(aftercare, input.niri.desk).await?
            && let Some(outcome) = admitted(input, &mut waiter, &mut device, &call, sent)
                .await
                .map_err(|error| partly(error, &typing, sent))?
        {
            device
                .finish(waiter.view().keyboard_group())
                .await
                .map_err(|error| partly(error, &typing, sent))?;
            return Ok(ended(outcome, focus, &typing, sent));
        }
        // Sent to niri before it failed or ran out of time, the key may still have gone out.
        call.budget
            .bound(device.stroke(key))
            .await
            .map_err(|error| partly(error, &typing, sent.underway()))?;
        sent.add_key(key.character);
        // Counts only once `may_press` told the desk the key was going out.
        let answered = typed_so_far(waiter.view(), focus, &typing, sent);
        input.niri.desk.answered(answered);
        if device.revision()? != revision {
            device
                .finish(waiter.view().keyboard_group())
                .await
                .map_err(|error| partly(error, &typing, sent))?;
            return Ok(ended(keymap_changed(waiter.view()), focus, &typing, sent));
        }
    }
    device
        .finish(waiter.view().keyboard_group())
        .await
        .map_err(|error| partly(error, &typing, sent))?;
    let outcome = settled(input.niri.socket, &mut waiter, before, group).await;
    Ok(ended(outcome, focus, &typing, sent))
}

/// The outcome once every key went out and niri acknowledged the last. niri sends keys to a
/// window it has just focused before its event stream reports the move, so that window may
/// not be in the stream yet; niri's own answer, asked after the last key, has it. The stream
/// is read too, after that answer, up to what it has passed on, so focus that left and came
/// back is seen as far as the stream has reported it.
async fn settled(socket: &Socket, waiter: &mut Waiter, before: Option<u64>, group: u32) -> Outcome {
    let focused = niri::focused_window(socket).await;
    if let Some(outcome) = interrupted(waiter, before, group).await {
        return outcome;
    }
    match focused {
        Ok(focused) if focused == before => {
            Outcome::seen(Observed::Sent, waiter.view(), Vec::new())
        }
        Ok(focused) => moved(focused, waiter.view()),
        Err(error) => Outcome::uncertain(
            Some(true),
            Some(waiter.view()),
            format!(
                "niri didn't say which window had focus after the last key: {}",
                error.detail
            ),
        ),
    }
}

/// The stroke that commits the call: the submitting `Return`, or the last key of a `key`
/// call, such as the `Return` of `["Down", "Down", "Return"]`. niri sends keys to a window
/// it has just focused before its event stream reports the move, so before that stroke
/// niri itself is asked which window has focus.
const fn decisive(typing: &Typing, keys: usize) -> Option<usize> {
    match typing {
        Typing::Text { submit: true, .. } | Typing::Keys(_) => keys.checked_sub(1),
        Typing::Text { .. } => None,
    }
}

/// The lock state, read again if its last read is older than `LAST_GATE`, then
/// `left_before`: niri's events since the lock was read or the last key, right before this
/// one. niri's answer before the `decisive` stroke can come late enough that the lock read
/// is older than `LAST_GATE` by then; the lock is then read and niri asked once more, so
/// both checks still come right before the key. A second answer as slow leaves the read
/// stale, and `may_press` refuses the key with `lock_stale`.
async fn checked_before(
    input: Input<'_>,
    waiter: &mut Waiter,
    before: Option<u64>,
    group: u32,
    decisive: bool,
) -> Result<Option<Outcome>, ToolError> {
    act::fresh_lock(input.niri).await?;
    let left = left_before(input.niri.socket, waiter, before, group, decisive).await?;
    if left.is_some() || input.niri.desk.lock_fresh() {
        return Ok(left);
    }
    act::fresh_lock(input.niri).await?;
    left_before(input.niri.socket, waiter, before, group, decisive).await
}

/// `interrupted`, and before the `decisive` stroke also focus away from `before` in niri's
/// own answer, then `interrupted` again for what the stream reported while niri answered.
async fn left_before(
    socket: &Socket,
    waiter: &mut Waiter,
    before: Option<u64>,
    group: u32,
    decisive: bool,
) -> Result<Option<Outcome>, ToolError> {
    if let Some(outcome) = interrupted(waiter, before, group).await {
        return Ok(Some(outcome));
    }
    if !decisive {
        return Ok(None);
    }
    let focused = niri::focused_window(socket).await?;
    if focused != before {
        return Ok(Some(moved(focused, waiter.view())));
    }
    Ok(interrupted(waiter, before, group).await)
}

/// After a paste's keeper admitted its key, which can take up to `ADMIT`, the decisive
/// stroke's checks again, right before the key: `checked_before`, which reads the lock
/// again if its last read has aged and asks niri itself which window has focus, since
/// niri can send the key to a window it has just focused before its stream says so, then
/// `may_press`. Nothing is awaited between those and the key. On a refusal or a failed
/// query, the keeper hears `n` and restores the clipboard.
async fn admitted(
    input: Input<'_>,
    waiter: &mut Waiter,
    device: &mut Device,
    call: &Call<'_>,
    sent: Sent,
) -> Result<Option<Outcome>, ToolError> {
    let checked = match checked_before(input, waiter, call.before, call.group, true).await {
        Ok(None) => may_press(input, waiter.view(), call, sent)
            .await
            .map(|()| None),
        other => other,
    };
    if !matches!(checked, Ok(None)) {
        device.withdraw().await;
    }
    checked
}

/// The outcome once niri says focus moved to `focused`.
fn moved(focused: Option<u64>, view: &View) -> Outcome {
    Outcome {
        focused_window: focused,
        detail: Some(moved_to(focused)),
        ..Outcome::seen(Observed::Interrupted, view, Vec::new())
    }
}

/// Binds a virtual keyboard on niri's display and plans `typing`'s keys in the layout
/// niri has active, which it returns too.
async fn bind(
    input: Input<'_>,
    view: &View,
    typing: &Typing,
) -> Result<(Keyboard, u32, Plan), CallError> {
    let display = input.display.path().map_err(Clone::clone)?;
    let keyboard = Keyboard::bind(display, niri::pid(input.niri.socket).await?).await?;
    let group = view.keyboard_group().ok_or_else(|| {
        ToolError::new(
            ErrorName::NiriUnavailable,
            "the active keyboard layout is unknown",
        )
    })?;
    let plan = plan(keyboard.map()?, group, typing)?;
    Ok((keyboard, group, plan))
}

/// The outcome once the compositor's keymap changed while keys went out.
fn keymap_changed(view: &View) -> Outcome {
    Outcome::uncertain(
        Some(true),
        Some(view),
        "the compositor keymap changed during typing".into(),
    )
}

/// What stays the same for each key of a call.
struct Call<'a> {
    typing: &'a Typing,
    budget: Budget,
    /// Whether to tell the desk each key is going out. `paste`'s keeper reports whether
    /// its key went out instead.
    marks: bool,
    /// The window that had focus when the call began.
    before: Option<u64>,
    /// The layout the keys were planned in.
    group: u32,
}

/// Whether the next key may go out: the stop flag isn't set, the focused app in `view` isn't
/// denied, the call's time isn't up, and the lock state was read at most `LAST_GATE` ago.
/// With `marks`, tells the desk the key is going out.
async fn may_press(
    input: Input<'_>,
    view: &View,
    call: &Call<'_>,
    sent: Sent,
) -> Result<(), ToolError> {
    if input
        .runtime
        .stopped()
        .map_err(|error| upstream(&format!("read stop flag: {error}")))?
    {
        if call.marks && !sent.nothing() {
            // Keys went out, so the call ends `uncertain`, as the desk's stop watcher ends
            // it once it sees the same flag.
            tokio::time::sleep(STOP_SEEN).await;
        }
        return Err(ToolError::new(
            ErrorName::Stopped,
            format!(
                "stopped after {} characters and {} keys",
                sent.chars, sent.keys
            ),
        ));
    }
    if let Some(refused) = policy::refuse_input(input.policy, focused_app_id(view)) {
        return Err(partly(refused, call.typing, sent));
    }
    call.budget
        .check()
        .map_err(|error| partly(error, call.typing, sent))?;
    let unlocked = if call.marks {
        input.niri.desk.sending_input()
    } else {
        input.niri.desk.fresh()
    };
    unlocked.map_err(|error| partly(error, call.typing, sent))
}

async fn interrupted(waiter: &mut Waiter, before: Option<u64>, group: u32) -> Option<Outcome> {
    match waiter
        .until(Duration::ZERO, |view| {
            (view.focused_window() != before || view.keyboard_group() != Some(group)).then_some(())
        })
        .await
    {
        Waited::Timeout => None,
        Waited::Done(()) => {
            let view = waiter.view();
            let detail = if view.focused_window() == before {
                "the active keyboard layout changed while they went out, so some of them may have typed other characters".to_owned()
            } else {
                moved_to(view.focused_window())
            };
            Some(Outcome {
                detail: Some(detail),
                ..Outcome::seen(Observed::Interrupted, view, Vec::new())
            })
        }
        Waited::Lost(reason) => Some(Outcome::uncertain(Some(true), Some(waiter.view()), reason)),
    }
}

/// What an `interrupted` call says of the keys that went out once focus moved to `focused`.
fn moved_to(focused: Option<u64>) -> String {
    focused.map_or_else(
        || "focus left the window while they went out, so some of them may have reached what took it".to_owned(),
        |id| format!("focus moved to window {id} while they went out, so some of them may have reached it"),
    )
}

/// A key's share of its call's time: three frames at 60 Hz. The slowest steady rate
/// measured, a nested niri drawing every frame, was two (32 ms).
const PER_KEY: Duration = Duration::from_millis(50);
/// The least time a call gets, so a short one isn't cut off by a single slow frame.
const MIN_TIME: Duration = Duration::from_secs(3);

/// The keys' time: `PER_KEY` for each key, and at least `MIN_TIME`, from
/// before the first key until niri has acknowledged the last. Each round trip has its own
/// deadline, but a niri that answers every one late could otherwise stretch a call
/// without end. Binding and planning before have their round trips' deadlines, and the
/// release after runs within the cleanup's. A call out of time stops, before its next key
/// or while one waits for niri, and the dropped device's cleanup releases the keys.
struct Budget {
    length: Duration,
    ends: Instant,
}

impl Budget {
    fn start(keys: usize) -> Self {
        let keys = u32::try_from(keys).unwrap_or(u32::MAX);
        let length = PER_KEY.saturating_mul(keys).max(MIN_TIME);
        Self {
            length,
            ends: Instant::now() + length,
        }
    }

    /// Refuses to start another key once the time is up.
    fn check(&self) -> Result<(), ToolError> {
        if Instant::now() < self.ends {
            return Ok(());
        }
        Err(self.over(""))
    }

    /// Runs a key's `stroke` until the time is up.
    async fn bound(
        &self,
        stroke: impl Future<Output = Result<(), ToolError>>,
    ) -> Result<(), ToolError> {
        tokio::time::timeout_at(self.ends, stroke)
            .await
            .unwrap_or_else(|_| Err(self.over(": niri hadn't acknowledged the key last sent")))
    }

    fn over(&self, pending: &str) -> ToolError {
        ToolError::new(
            ErrorName::DeadlineExceeded,
            format!(
                "native typing took longer than its {:?}{pending}",
                self.length
            ),
        )
    }
}

struct Device {
    keyboard: Option<Keyboard>,
    marker: Option<Written>,
    /// niri's socket, asked for the active layout before the layout is restored.
    socket: Socket,
    /// The layout niri last reported to this call.
    group: u32,
    aftercare: Option<Aftercare>,
    /// The deadline of the cleanup task the release runs in, which a cleanup started when
    /// the device drops takes over.
    ends: Option<Instant>,
}

impl Device {
    async fn new(
        keyboard: Keyboard,
        input: Input<'_>,
        typing: &Typing,
        keys: impl Iterator<Item = &Key>,
        group: u32,
    ) -> Result<Self, ToolError> {
        let mut codes: Vec<u32> = keys.map(|key| key.code.0).collect();
        codes.sort_unstable();
        codes.dedup();
        let mut marker = Marker::pending(typing.tool(), Vec::new());
        marker.keyboard = Some(Native {
            codes: codes.clone(),
            group,
        });
        let marker = Written::write(input.runtime, marker)
            .await
            .map_err(|error| marker::unwritten(&error))?;
        Ok(Self {
            keyboard: Some(keyboard),
            marker: Some(marker),
            socket: input.niri.socket.clone(),
            group,
            aftercare: None,
            ends: None,
        })
    }

    fn keyboard(&mut self) -> Result<&mut Keyboard, ToolError> {
        self.keyboard
            .as_mut()
            .ok_or_else(|| upstream("native keyboard is gone"))
    }

    fn revision(&mut self) -> Result<u64, ToolError> {
        Ok(self.keyboard()?.revision())
    }

    /// Uploads `map` for the keys that follow.
    fn extend(&mut self, map: &str) -> Result<(), ToolError> {
        self.keyboard()?.extend(map)
    }

    /// Takes `aftercare`, if it is still there, and arms it. Says whether it did.
    async fn commit(
        &mut self,
        aftercare: &mut Option<Aftercare>,
        desk: &Desk,
    ) -> Result<bool, ToolError> {
        let Some(taken) = aftercare.take() else {
            return Ok(false);
        };
        self.aftercare = Some(taken.arm(desk).await?);
        Ok(true)
    }

    /// Tells a paste's keeper, which admitted the key, that it won't go out after all.
    async fn withdraw(&mut self) {
        if let Some(aftercare) = self.aftercare.take() {
            aftercare.withdraw().await;
        }
    }

    /// Sends one key pair with the key's modifiers in the key's layout, then zero modifiers
    /// in the active layout, so a key found in another layout leaves the client in the one
    /// niri has active.
    async fn stroke(&mut self, key: Key) -> Result<(), ToolError> {
        let active = self.group;
        let keyboard = self.keyboard()?;
        keyboard.modifiers(key.modifiers, key.group)?;
        keyboard.key(key.code.0, true)?;
        keyboard.key(key.code.0, false)?;
        keyboard.modifiers(0, active)?;
        keyboard.sync().await
    }

    /// Releases everything and puts the latest compositor keymap back in the layout niri
    /// has active, or without niri's answer in the one its event stream last reported,
    /// `group`, if it reported one: a layout the user switched to meanwhile stays. Then
    /// tells a paste's keeper the key went out and clears the marker. That runs in a
    /// cleanup task, within the cleanup's deadline, so a dropped call can't cut it short or
    /// leave the keeper without its `p`.
    async fn finish(mut self, group: Option<u32>) -> Result<(), ToolError> {
        if let Some(group) = group {
            self.group = group;
        }
        let aftercare = self.aftercare.take();
        let ends = cleanup::deadline();
        self.ends = Some(ends);
        let finished = cleanup::spawn(ends, async move {
            self.release().await?;
            if let Some(aftercare) = aftercare {
                aftercare.sent().await;
            }
            self.clear().await
        });
        finished
            .await
            .map_err(|error| upstream(&format!("the release task ended: {error}")))?
    }

    async fn release(&mut self) -> Result<(), ToolError> {
        if let Ok(group) = niri::keyboard_group(&self.socket).await {
            self.group = group;
        }
        let group = self.group;
        let keyboard = self.keyboard()?;
        keyboard.release(&[], group).await?;
        keyboard.restore(group).await
    }

    async fn clear(&mut self) -> Result<(), ToolError> {
        if let Some(marker) = self.marker.take() {
            marker
                .clear()
                .await
                .map_err(|error| upstream(&format!("clear native input-dirty marker: {error}")))?;
        }
        Ok(())
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        let (Some(mut keyboard), Some(marker)) = (self.keyboard.take(), self.marker.take()) else {
            return;
        };
        if keyboard.release_now(&[], self.group).is_err() {
            return;
        }
        if tokio::runtime::Handle::try_current().is_ok() {
            let aftercare = self.aftercare.take();
            cleanup::spawn(
                self.ends.unwrap_or_else(cleanup::deadline),
                acknowledge_release(
                    keyboard,
                    marker,
                    aftercare,
                    (self.socket.clone(), self.group),
                ),
            );
        }
    }
}

/// Once niri has acknowledged the release, which went out at once in the layout the call
/// last knew, `layout`'s group, puts the latest compositor keymap back in the layout niri
/// has active by now, and then, after telling a paste's keeper the key went out, clears
/// the marker. Unacknowledged or not restored, the marker stays, and the keeper, dropped,
/// keeps the pasted text.
async fn acknowledge_release(
    mut keyboard: Keyboard,
    marker: Written,
    aftercare: Option<Aftercare>,
    layout: (Socket, u32),
) -> Result<(), ToolError> {
    keyboard.sync().await?;
    restore_active(&mut keyboard, &layout.0, layout.1).await?;
    if let Some(aftercare) = aftercare {
        aftercare.sent().await;
    }
    marker
        .clear()
        .await
        .map_err(|error| upstream(&format!("clear native input-dirty marker: {error}")))
}

/// Puts the latest base map back with zero modifiers in the layout niri has active, or
/// without niri's answer in `sent`, and sends the modifiers once more after niri has taken
/// it: in the nested trials, this device saw the map a dropped call restored come back
/// only after a further input, and the client sometimes got no modifiers event for one of
/// the two. Then proves clients and the device hold the latest base map; an error leaves
/// the caller's marker for `recover`. A held pointer modifier's release ends the same way.
pub(super) async fn restore_active(
    keyboard: &mut Keyboard,
    socket: &Socket,
    sent: u32,
) -> Result<(), ToolError> {
    let group = niri::keyboard_group(socket).await.unwrap_or(sent);
    keyboard.put_back(group).await?;
    keyboard.modifiers(0, group)?;
    keyboard.sync().await?;
    if keyboard.restored() {
        return Ok(());
    }
    Err(upstream(
        "the last keymap niri sent clients isn't its latest compositor keymap",
    ))
}

fn upstream(detail: &str) -> ToolError {
    ToolError::new(ErrorName::UpstreamError, detail)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::runtime::RuntimeDir;
    use crate::input::keyboard::Typing;
    use crate::input::keymap::Part;
    use std::path::Path;

    use crate::act::Niri;
    use crate::control::LockSources;
    use crate::error::CallError;
    use crate::input::paste::{
        Aftercare, Clipboard, Report, SLOW_ADMISSION, STOPPED_WAITING, desk_read, fake_aftercare,
    };
    use crate::niri::waiter::{Update, tests as events};
    use crate::noctalia;
    use crate::policy::Loaded;
    use crate::test_support::{
        fresh_dir,
        wayland::{APP_ID, Compositor, initial_events},
    };
    use niri_ipc::Event;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    use tokio::sync::broadcast;

    const BASE: &str = include_str!("../../tests/fixtures/us-de.xkb");
    /// evdev codes in the fixture's first layout: `a`, `z` and Shift.
    const A: u32 = 40;
    const Z: u32 = 44;
    const HELD: u32 = 42;
    /// The fixture's `<ENTER>`, Return.
    const RETURN: u32 = 36 - 8;
    /// evdev codes of the fixture's spare keys, `<I60>` and `<I61>`.
    const I60: u32 = 60 - 8;
    const I61: u32 = 61 - 8;

    /// A character's key on `code`, with no modifiers, in the first layout.
    const fn character(code: u32) -> Key {
        Key {
            code: crate::input::keymap::Evdev(code),
            modifiers: 0,
            group: 0,
            character: true,
        }
    }

    /// Each key's press, then its release.
    fn presses(keys: &[Key]) -> Vec<(u32, bool)> {
        keys.iter()
            .flat_map(|key| [(key.code.0, true), (key.code.0, false)])
            .collect()
    }

    /// A device on the fake niri in `dir`, with `text` planned in layout 0 in one part: its
    /// extended keymap uploaded, if the layout lacks some of the text's symbols.
    async fn typing(niri: &Compositor, text: &str) -> (Device, Vec<Key>) {
        let keyboard = Keyboard::bind(niri.display(), std::process::id())
            .await
            .unwrap();
        let typing = Typing::Text {
            text: text.to_owned(),
            submit: false,
        };
        let [part] =
            <[Part; 1]>::try_from(plan(keyboard.map().unwrap(), 0, &typing).unwrap().parts)
                .unwrap();
        let mut device = Device {
            keyboard: Some(keyboard),
            marker: None,
            socket: niri.niri_socket(),
            group: 0,
            aftercare: None,
            ends: None,
        };
        if let Some(map) = &part.extended {
            device.extend(map).unwrap();
        }
        (device, part.keys)
    }

    /// Text beyond the layout goes out through an extended keymap, which clients then hold;
    /// the call ends with every key released, zero modifiers and the compositor's keymap back
    /// in clients and active in niri, so the user's next key sends no keymap.
    #[tokio::test]
    async fn a_call_with_an_extended_keymap_ends_with_the_compositors_back() {
        let dir = fresh_dir("native-extension");
        let niri = Compositor::start(&dir, BASE);
        let (mut device, keys) = typing(&niri, "é").await;
        let pairs = presses(&keys);
        assert_eq!(pairs.len(), 2);
        for key in keys {
            device.stroke(key).await.unwrap();
        }
        let during = niri.seen();
        assert_eq!(during.keys, pairs, "each key pressed and released once");
        assert_ne!(
            during.active, during.compositor,
            "the extension reached clients"
        );
        device.finish(Some(0)).await.unwrap();
        let seen = niri.seen();
        assert_eq!(seen.active, seen.compositor);
        assert!(seen.held_keys().is_empty(), "{:?}", seen.keys);
        assert_eq!(seen.last_modifiers(), Some((0, 0)));
        niri.physical(38).await;
        assert_eq!(
            niri.seen().keymaps,
            seen.keymaps,
            "a physical key sent a keymap"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A call dropped mid-text, as a cancellation drops it, releases at once and, once niri
    /// has acknowledged that, puts the compositor's keymap back in the layout niri has active
    /// before its marker comes off.
    #[tokio::test]
    async fn a_dropped_call_restores_the_compositors_keymap_before_its_marker_goes() {
        let dir = fresh_dir("native-dropped");
        let niri = Compositor::start(&dir, BASE);
        let runtime = RuntimeDir::of(&niri.env(&dir)).unwrap();
        runtime.create().unwrap();
        let (mut device, keys) = typing(&niri, "éa").await;
        device.marker = Some(
            Written::write(&runtime, Marker::pending("type_text", Vec::new()))
                .await
                .unwrap(),
        );
        device.stroke(keys[0]).await.unwrap();
        // A key still down when the call drops, as between a press and its release.
        let keyboard = device.keyboard().unwrap();
        keyboard.key(HELD, true).unwrap();
        keyboard.sync().await.unwrap();
        assert_eq!(niri.seen().held_keys(), [HELD]);
        niri.switch(1).await;
        drop(device);
        let until = Instant::now() + cleanup::LIMIT;
        while marker::read(&runtime).is_some() {
            assert!(Instant::now() < until, "the marker stayed");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let seen = niri.seen();
        assert_eq!(seen.active, seen.compositor);
        assert_eq!(seen.keys.last(), Some(&(HELD, false)), "{:?}", seen.keys);
        assert!(seen.held_keys().is_empty(), "{:?}", seen.keys);
        assert_eq!(
            seen.last_modifiers(),
            Some((0, 1)),
            "not in niri's active layout"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Serves Noctalia's status for `env`: the screen unlocked, and from the `locked_from`th
    /// read on, counted from 1, locked.
    fn noctalia_status(env: &crate::Env, locked_from: Option<usize>) {
        let listener = tokio::net::UnixListener::bind(noctalia::socket(env).unwrap()).unwrap();
        tokio::spawn(async move {
            let mut reads = 0;
            while let Ok((mut stream, _)) = listener.accept().await {
                let mut request = Vec::new();
                stream.read_to_end(&mut request).await.ok();
                reads += 1;
                let locked = locked_from.is_some_and(|from| reads >= from);
                let status = format!(r#"{{"locked": {locked}}}"#);
                stream.write_all(status.as_bytes()).await.ok();
            }
        });
    }

    /// What a test's call types against besides the fake: the policy, from which read on
    /// Noctalia says the screen is locked, and a paste's keeper, if the call pastes.
    struct Setup {
        policy: Loaded,
        locked_from: Option<usize>,
        aftercare: Option<Aftercare>,
    }

    const PLAIN: Setup = Setup {
        policy: Loaded::Missing,
        locked_from: None,
        aftercare: None,
    };

    fn text(text: &str) -> Typing {
        Typing::Text {
            text: text.to_owned(),
            submit: false,
        }
    }

    /// Types `typing` through the entry point with `expect` on the fake's focused window,
    /// with the screen unlocked, and niri's events from `waiter` when given.
    async fn type_through(
        niri: &Compositor,
        dir: &Path,
        typing: Typing,
        waiter: Option<Waiter>,
    ) -> Result<Outcome, CallError> {
        type_with(niri, dir, typing, waiter, PLAIN).await
    }

    /// `type_through` with `setup`'s policy, lock state and paste keeper.
    async fn type_with(
        niri: &Compositor,
        dir: &Path,
        typing: Typing,
        waiter: Option<Waiter>,
        mut setup: Setup,
    ) -> Result<Outcome, CallError> {
        let env = crate::Env {
            wayland_display: Some("fake-noctalia".into()),
            ..niri.env(dir)
        };
        noctalia_status(&env, setup.locked_from);
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
                live: &crate::test_support::NO_LIVE,
            },
            display: &env.display,
            runtime: &runtime,
            policy: &setup.policy,
            a11y: None,
        };
        let expect = Expect::App(APP_ID.to_owned());
        match waiter {
            Some(waiter) => type_seen(input, waiter, typing, expect, &mut setup.aftercare).await,
            None => type_input(input, typing, expect, &mut setup.aftercare).await,
        }
    }

    /// Waits until the call's marker in `dir` is gone, as the cleanup's deadline allows.
    async fn marker_cleared(niri: &Compositor, dir: &Path) {
        let runtime = RuntimeDir::of(&niri.env(dir)).unwrap();
        let until = Instant::now() + cleanup::LIMIT;
        while marker::read(&runtime).is_some() {
            assert!(Instant::now() < until, "the marker stayed");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    #[tokio::test]
    async fn a_call_types_each_key_once_and_clears_its_marker() {
        let dir = fresh_dir("native-entry");
        let niri = Compositor::start(&dir, BASE);
        let outcome = type_through(&niri, &dir, text("az"), None).await.unwrap();
        assert_eq!(outcome.observed, Observed::Sent);
        assert_eq!(
            niri.seen().keys,
            [(A, true), (A, false), (Z, true), (Z, false)]
        );
        marker_cleared(&niri, &dir).await;
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A waiter on the fake's events that sees focus move to window 2 while niri handles the
    /// `press`th key press, before it acknowledges it.
    async fn focus_moves_at(niri: &Compositor, press: usize) -> Waiter {
        let view = events::view_of(initial_events(0));
        let (waiter, updates) = events::waiter(view, 0);
        let moved = Event::WindowFocusChanged { id: Some(2) };
        niri.on_press(press, move || {
            updates.send(Update::Event(1, Box::new(moved))).ok();
        })
        .await;
        waiter
    }

    #[tokio::test]
    async fn submit_presses_return_once_all_of_the_text_went_out() {
        let dir = fresh_dir("native-submit");
        let niri = Compositor::start(&dir, BASE);
        let typing = Typing::Text {
            text: "az".to_owned(),
            submit: true,
        };
        let outcome = type_through(&niri, &dir, typing, None).await.unwrap();
        assert_eq!(outcome.observed, Observed::Sent);
        assert_eq!((outcome.typed, outcome.submitted), (None, Some(true)));
        assert_eq!(
            niri.seen().keys,
            [
                (A, true),
                (A, false),
                (Z, true),
                (Z, false),
                (RETURN, true),
                (RETURN, false)
            ]
        );
        marker_cleared(&niri, &dir).await;
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Focus moves while the first character goes out: the rest of the text and the
    /// submitting `Return` aren't typed, and the outcome says so.
    #[tokio::test]
    async fn focus_moving_mid_text_stops_the_rest_and_its_return() {
        let dir = fresh_dir("native-submit-moved");
        let niri = Compositor::start(&dir, BASE);
        let waiter = focus_moves_at(&niri, 1).await;
        let typing = Typing::Text {
            text: "az".to_owned(),
            submit: true,
        };
        let outcome = type_through(&niri, &dir, typing, Some(waiter))
            .await
            .unwrap();
        assert_eq!(outcome.observed, Observed::Interrupted);
        assert_eq!((outcome.typed, outcome.submitted), (Some(1), Some(false)));
        assert_eq!(
            outcome.detail.as_deref(),
            Some(
                "typed 1 of 2 characters, not yet the submitting Return; focus left the window while they went out, so some of them may have reached what took it"
            )
        );
        assert_eq!(niri.seen().keys, [(A, true), (A, false)]);
        marker_cleared(&niri, &dir).await;
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// niri gives another window focus while it handles the last key, and its event stream
    /// hasn't said so when the call ends: niri's own answer makes the call `interrupted`,
    /// with all of the text counted and where some of it may have gone.
    #[tokio::test]
    async fn focus_the_stream_reports_only_after_the_last_key_interrupts_the_call() {
        let dir = fresh_dir("native-quiet-focus");
        let niri = Compositor::start(&dir, BASE);
        niri.focus_quietly_at(2, Some(2)).await;
        let outcome = type_through(&niri, &dir, text("az"), None).await.unwrap();
        assert_eq!(outcome.observed, Observed::Interrupted);
        assert_eq!((outcome.typed, outcome.focused_window), (None, Some(2)));
        assert_eq!(
            outcome.detail.as_deref(),
            Some(
                "typed 2 of 2 characters; focus moved to window 2 while they went out, so some of them may have reached it"
            )
        );
        assert_eq!(
            niri.seen().keys,
            [(A, true), (A, false), (Z, true), (Z, false)]
        );
        marker_cleared(&niri, &dir).await;
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// niri gives another window focus while it handles the last character, and its event
    /// stream hasn't said so before the submitting `Return`: niri's own answer, asked before
    /// `Return`, stops the call, so `Return` doesn't confirm whatever took focus.
    #[tokio::test]
    async fn focus_the_stream_hasnt_reported_before_the_return_stops_it() {
        let dir = fresh_dir("native-quiet-submit");
        let niri = Compositor::start(&dir, BASE);
        niri.focus_quietly_at(2, Some(2)).await;
        let typing = Typing::Text {
            text: "az".to_owned(),
            submit: true,
        };
        let outcome = type_through(&niri, &dir, typing, None).await.unwrap();
        assert_eq!(niri.seen().keys, presses(&[A, Z].map(character)));
        assert_eq!(outcome.observed, Observed::Interrupted);
        assert_eq!(
            (outcome.submitted, outcome.focused_window),
            (Some(false), Some(2))
        );
        marker_cleared(&niri, &dir).await;
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A one-key `key` call goes nowhere once niri has given another window focus that its
    /// event stream hasn't reported yet.
    #[tokio::test]
    async fn a_single_key_waits_for_niri_to_confirm_focus() {
        let dir = fresh_dir("native-quiet-key");
        let niri = Compositor::start(&dir, BASE);
        niri.focus_quietly(Some(2));
        let typing = Typing::Keys(vec!["Return".to_owned()]);
        let outcome = type_through(&niri, &dir, typing, None).await.unwrap();
        assert_eq!(niri.seen().keys, []);
        assert_eq!(
            (outcome.observed, outcome.pressed),
            (Observed::Interrupted, Some(0))
        );
        marker_cleared(&niri, &dir).await;
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// The event stream reports focus moving while niri answers the request before the
    /// decisive key, and niri's answer, from just before, still names the window: the key
    /// doesn't go out.
    #[tokio::test]
    async fn focus_the_stream_reports_during_the_check_before_the_last_key_stops_it() {
        let dir = fresh_dir("native-focus-asked");
        let niri = Compositor::start(&dir, BASE);
        let view = events::view_of(initial_events(0));
        let (waiter, updates) = events::waiter(view, 0);
        let moved = Event::WindowFocusChanged { id: Some(2) };
        niri.on_focus_request(move || {
            updates.send(Update::Event(1, Box::new(moved))).ok();
        });
        let typing = Typing::Keys(vec!["Return".to_owned()]);
        let outcome = type_through(&niri, &dir, typing, Some(waiter))
            .await
            .unwrap();
        assert_eq!(niri.seen().keys, []);
        assert_eq!(outcome.observed, Observed::Interrupted);
        marker_cleared(&niri, &dir).await;
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// niri answers the request before the decisive key slowly enough that the lock read
    /// checked before it is older than `LAST_GATE` by the answer: the lock is read again and
    /// niri asked again, and the key goes out rather than failing `lock_stale`.
    #[tokio::test]
    async fn a_slow_focus_answer_before_the_last_key_doesnt_leave_the_lock_read_stale() {
        let dir = fresh_dir("native-slow-focus-answer");
        let niri = Compositor::start(&dir, BASE);
        niri.answer_focus_late(crate::control::desk::LAST_GATE + Duration::from_millis(50));
        let typing = Typing::Keys(vec!["Return".to_owned()]);
        let outcome = type_through(&niri, &dir, typing, None).await.unwrap();
        assert_eq!(outcome.observed, Observed::Sent);
        assert_eq!(niri.seen().keys, [(RETURN, true), (RETURN, false)]);
        marker_cleared(&niri, &dir).await;
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// The last key of a `key` call with several, such as the `Return` of
    /// `["Down", "Down", "Return"]`, doesn't go out once niri has given another window focus
    /// that its event stream hasn't reported.
    #[tokio::test]
    async fn the_last_of_several_keys_waits_for_niri_to_confirm_focus() {
        let dir = fresh_dir("native-quiet-last-key");
        let niri = Compositor::start(&dir, BASE);
        niri.focus_quietly_at(2, Some(2)).await;
        let outcome = type_through(&niri, &dir, three_keys(), None).await.unwrap();
        assert_eq!(niri.seen().keys, presses(&[Z, A].map(character)));
        assert_eq!(
            (outcome.observed, outcome.pressed),
            (Observed::Interrupted, Some(2))
        );
        marker_cleared(&niri, &dir).await;
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// niri can't say where focus is once the keys went out: the call isn't `sent` but
    /// `uncertain`, with niri's message.
    #[tokio::test]
    async fn no_answer_on_focus_after_the_last_key_leaves_the_call_uncertain() {
        let dir = fresh_dir("native-focus-unanswered");
        let niri = Compositor::start(&dir, BASE);
        niri.fail_focus_at(1, "niri is shutting down").await;
        let outcome = type_through(&niri, &dir, text("zaz"), None).await.unwrap();
        assert_eq!(
            (outcome.observed, outcome.accepted),
            (Observed::Uncertain, Some(true))
        );
        let detail = outcome.detail.unwrap_or_default();
        assert!(detail.contains("niri is shutting down"), "{detail}");
        assert_eq!(niri.seen().keys, presses(&[Z, A, Z].map(character)));
        marker_cleared(&niri, &dir).await;
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// niri can't say where focus is before a `key` call's last key: that key doesn't go
    /// out, and the failure counts the keys that did.
    #[tokio::test]
    async fn no_answer_on_focus_before_the_last_key_keeps_it_back() {
        let dir = fresh_dir("native-focus-unanswered-key");
        let niri = Compositor::start(&dir, BASE);
        niri.fail_focus_at(1, "niri is shutting down").await;
        let Err(CallError::Tool(error)) = type_through(&niri, &dir, three_keys(), None).await
        else {
            panic!("the last key wasn't refused");
        };
        assert!(error.may_have_sent);
        assert!(
            error.detail.starts_with("pressed 2 of 3"),
            "{}",
            error.detail
        );
        assert!(
            error.detail.contains("niri is shutting down"),
            "{}",
            error.detail
        );
        assert_eq!(niri.seen().keys, presses(&[Z, A].map(character)));
        marker_cleared(&niri, &dir).await;
        std::fs::remove_dir_all(dir).unwrap();
    }

    fn three_keys() -> Typing {
        Typing::Keys(vec!["z".to_owned(), "a".to_owned(), "z".to_owned()])
    }

    /// `key` presses its combinations in order.
    #[tokio::test]
    async fn key_presses_each_combination_in_order() {
        let dir = fresh_dir("native-keys");
        let niri = Compositor::start(&dir, BASE);
        let outcome = type_through(&niri, &dir, three_keys(), None).await.unwrap();
        assert_eq!((outcome.observed, outcome.pressed), (Observed::Sent, None));
        assert_eq!(niri.seen().keys, presses(&[Z, A, Z].map(character)));
        marker_cleared(&niri, &dir).await;
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// `key` presses no combination after focus moved, and says how many went out.
    #[tokio::test]
    async fn key_stops_once_focus_moves() {
        let dir = fresh_dir("native-keys-moved");
        let niri = Compositor::start(&dir, BASE);
        let waiter = focus_moves_at(&niri, 2).await;
        let outcome = type_through(&niri, &dir, three_keys(), Some(waiter))
            .await
            .unwrap();
        assert_eq!(
            (outcome.observed, outcome.pressed),
            (Observed::Interrupted, Some(2))
        );
        assert_eq!(niri.seen().keys, presses(&[Z, A].map(character)));
        marker_cleared(&niri, &dir).await;
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// The focused window's app becomes one the policy denies while the second key goes
    /// out: the third isn't pressed, and the refusal counts the two that were.
    #[tokio::test]
    async fn a_window_whose_app_becomes_denied_gets_no_more_keys() {
        let dir = fresh_dir("native-keys-denied");
        let niri = Compositor::start(&dir, BASE);
        let view = events::view_of(initial_events(0));
        let mut window = view.windows()[&1].clone();
        let (waiter, updates) = events::waiter(view, 0);
        window.app_id = Some("b".to_owned());
        let denied = Event::WindowOpenedOrChanged { window };
        niri.on_press(2, move || {
            updates.send(Update::Event(1, Box::new(denied))).ok();
        })
        .await;
        let setup = Setup {
            policy: Loaded::Valid(policy::parse("deny_input_app_ids = [\"b\"]\n", false).unwrap()),
            locked_from: None,
            aftercare: None,
        };
        let Err(CallError::Tool(error)) =
            type_with(&niri, &dir, three_keys(), Some(waiter), setup).await
        else {
            panic!("the third key wasn't refused");
        };
        assert_eq!(error.name, ErrorName::AppDenied);
        assert!(error.may_have_sent);
        assert!(
            error.detail.starts_with("pressed 2 of 3"),
            "{}",
            error.detail
        );
        assert_eq!(niri.seen().keys, presses(&[Z, A].map(character)));
        marker_cleared(&niri, &dir).await;
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// The screen locks while the first key goes out, slowly enough that the lock state is
    /// read again before the second: the second isn't typed, and the refusal counts the
    /// first.
    #[tokio::test]
    async fn a_screen_locked_before_the_next_key_gets_no_more_keys() {
        let dir = fresh_dir("native-keys-locked");
        let niri = Compositor::start(&dir, BASE);
        niri.slow_keys(Duration::from_millis(300)).await;
        let setup = Setup {
            policy: Loaded::Missing,
            locked_from: Some(2),
            aftercare: None,
        };
        let Err(CallError::Tool(error)) = type_with(&niri, &dir, text("az"), None, setup).await
        else {
            panic!("the second key wasn't refused");
        };
        assert_eq!(error.name, ErrorName::ScreenLocked);
        assert!(error.may_have_sent);
        assert!(
            error.detail.starts_with("typed 1 of 2 characters"),
            "{}",
            error.detail
        );
        niri.slow_keys(Duration::ZERO).await;
        marker_cleared(&niri, &dir).await;
        assert_eq!(niri.seen().keys, [(A, true), (A, false)]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Text needing more spare keys than the map has goes out in parts: clients get each
    /// part's map before its keys, `é` and `ß` the same spare key in turn, and then the
    /// compositor's map back.
    #[tokio::test]
    async fn a_call_with_more_missing_symbols_than_spare_keys_types_them_all() {
        let dir = fresh_dir("native-parts");
        let niri = Compositor::start(&dir, BASE);
        let outcome = type_through(&niri, &dir, text("é→ß"), None).await.unwrap();
        assert_eq!(outcome.observed, Observed::Sent);
        assert_eq!(outcome.typed, None, "all of it went out");
        marker_cleared(&niri, &dir).await;
        let seen = niri.seen();
        assert_eq!(seen.keys, presses(&[I60, I61, I60].map(character)));
        assert_eq!(seen.keymaps.len(), 3, "two maps, then the compositor's");
        assert_ne!(seen.keymaps[0], seen.keymaps[1]);
        assert!(!seen.keymaps[..2].contains(&seen.compositor));
        assert_eq!(seen.active, seen.compositor);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Focus moves once the first part went out: the call ends `interrupted` with what it
    /// typed, and never uploads the second part's map: the device holds the compositor's
    /// map, then the first part's, then the compositor's again.
    #[tokio::test]
    async fn a_focus_change_between_parts_ends_the_call_with_the_first_part() {
        let dir = fresh_dir("native-parts-moved");
        let niri = Compositor::start(&dir, BASE);
        let waiter = focus_moves_at(&niri, 2).await;
        let outcome = type_through(&niri, &dir, text("é→ß"), Some(waiter))
            .await
            .unwrap();
        assert_eq!(outcome.observed, Observed::Interrupted);
        assert_eq!(outcome.typed, Some(2));
        marker_cleared(&niri, &dir).await;
        let seen = niri.seen();
        assert_eq!(seen.keys, presses(&[I60, I61].map(character)));
        assert_eq!(
            seen.uploads,
            [
                seen.compositor.clone(),
                seen.keymaps[0].clone(),
                seen.compositor.clone()
            ],
            "bound with the compositor's map, the first part's, then the compositor's back"
        );
        assert_ne!(seen.keymaps[0], seen.compositor);
        assert_eq!(seen.active, seen.compositor);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// The stop flag set once the first part went out: the call ends `stopped`, and never
    /// uploads the second part's map.
    #[tokio::test]
    async fn a_stop_between_parts_ends_the_call_before_the_next_map() {
        let dir = fresh_dir("native-parts-stopped");
        let niri = Compositor::start(&dir, BASE);
        let runtime = RuntimeDir::of(&niri.env(&dir)).unwrap();
        niri.on_press(2, move || runtime.stop().unwrap()).await;
        let Err(CallError::Tool(error)) = type_through(&niri, &dir, text("é→ß"), None).await
        else {
            panic!("the call went on past the stop");
        };
        assert_eq!(error.name, ErrorName::Stopped, "{}", error.detail);
        assert_eq!(error.detail, "stopped after 2 characters and 0 keys");
        marker_cleared(&niri, &dir).await;
        let seen = niri.seen();
        assert_eq!(seen.keys, presses(&[I60, I61].map(character)));
        assert_eq!(
            seen.keymaps.len(),
            2,
            "the first part's map, then the compositor's"
        );
        assert_eq!(
            seen.uploads.len(),
            3,
            "bound, the first part's, the compositor's"
        );
        assert_eq!(seen.active, seen.compositor);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// With the second layout active, which lacks `a`, `ctrl+a` goes out in the first
    /// layout, and the call leaves the focused client in the second.
    #[tokio::test]
    async fn a_key_from_another_layout_leaves_the_active_layout_in_place() {
        let dir = fresh_dir("native-other-layout");
        let niri = Compositor::start(&dir, BASE);
        niri.switch(1).await;
        let keys = Typing::Keys(vec!["ctrl+a".to_owned()]);
        let outcome = type_through(&niri, &dir, keys, None).await.unwrap();
        assert_eq!(outcome.observed, Observed::Sent);
        marker_cleared(&niri, &dir).await;
        let seen = niri.seen();
        assert_eq!(seen.keys, [(A, true), (A, false)]);
        let ctrl = crate::input::keymap::held_mask(BASE, &["ctrl"]).unwrap();
        let sent = seen.modifiers.iter().position(|&mods| mods == (ctrl, 0));
        let after = sent.and_then(|at| seen.modifiers.get(at + 1));
        assert_eq!(after, Some(&(0, 1)), "{:?}", seen.modifiers);
        assert_eq!(seen.last_modifiers(), Some((0, 1)), "{:?}", seen.modifiers);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Each key acknowledged in 1.9 s, inside a round trip's two seconds, but the second
    /// overruns the two keys' three: the call ends `deadline_exceeded` while niri still
    /// holds it, says what it typed, and the cleanup releases everything.
    #[tokio::test]
    async fn a_call_whose_last_key_overruns_its_time_ends_and_is_released() {
        let dir = fresh_dir("native-overrun");
        let niri = Compositor::start(&dir, BASE);
        niri.slow_keys(Duration::from_millis(1900)).await;
        let started = Instant::now();
        let Err(CallError::Tool(error)) = type_through(&niri, &dir, text("az"), None).await else {
            panic!("the call ended in time");
        };
        assert!(
            started.elapsed() < Duration::from_millis(3500),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(error.name, ErrorName::DeadlineExceeded);
        assert!(
            error
                .detail
                .contains("typed 1 of 2 characters, and more may have gone out; then native typing took longer than its 3s: niri hadn't acknowledged the key last sent"),
            "{}",
            error.detail
        );
        niri.slow_keys(Duration::ZERO).await;
        marker_cleared(&niri, &dir).await;
        let seen = niri.seen();
        assert!(seen.held_keys().is_empty(), "{:?}", seen.keys);
        assert_eq!(
            seen.last_modifiers().map(|(depressed, _)| depressed),
            Some(0)
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// The first key sent to niri but not acknowledged within the round trip's two seconds
    /// may still go out, and does here: the error says so though nothing was counted.
    #[tokio::test]
    async fn a_first_key_niri_didnt_acknowledge_may_have_gone_out() {
        let dir = fresh_dir("native-first-late");
        let niri = Compositor::start(&dir, BASE);
        niri.slow_keys(Duration::from_millis(2100)).await;
        let Err(CallError::Tool(error)) = type_through(&niri, &dir, text("a"), None).await else {
            panic!("the call ended in time");
        };
        assert_eq!(error.name, ErrorName::DeadlineExceeded);
        assert!(
            error
                .detail
                .starts_with("typed 0 of 1 characters, and more may have gone out; then "),
            "{}",
            error.detail
        );
        assert!(error.may_have_sent);
        niri.slow_keys(Duration::ZERO).await;
        marker_cleared(&niri, &dir).await;
        assert_eq!(niri.seen().keys, [(A, true), (A, false)]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// The refusal of the second key of "az" after the first went out, with the lock state
    /// last read `ago` and the call's time ending at `ends`.
    async fn second_key_refused(test: &str, ago: Duration, ends: Instant) -> ToolError {
        let dir = fresh_dir(test);
        let env = crate::test_support::niri_env(&dir);
        let runtime = RuntimeDir::of(&env).unwrap();
        runtime.create().unwrap();
        let desk = desk_read(&dir, ago);
        let unread = ToolError::new(ErrorName::NiriUnavailable, "no event stream in this test");
        let input = Input {
            niri: Niri {
                socket: &env.niri_socket,
                events: Err(&unread),
                desk: &desk,
                locks: LockSources {
                    env: &env,
                    noctalia: false,
                },
                live: &crate::test_support::NO_LIVE,
            },
            display: &env.display,
            runtime: &runtime,
            policy: &Loaded::Missing,
            a11y: None,
        };
        let typing = Typing::Text {
            text: "az".to_owned(),
            submit: false,
        };
        let call = Call {
            typing: &typing,
            budget: Budget {
                length: MIN_TIME,
                ends,
            },
            marks: true,
            before: None,
            group: 0,
        };
        let mut sent = Sent::default();
        sent.add_key(true);
        let error = may_press(input, &View::default(), &call, sent)
            .await
            .unwrap_err();
        std::fs::remove_dir_all(dir).unwrap();
        error
    }

    /// Once the call's time is up, the next key doesn't start, though the stop flag, the
    /// deny list and the lock state would let it, and the refusal counts what was typed.
    #[tokio::test]
    async fn no_key_starts_once_the_time_is_up() {
        let error = second_key_refused("native-time-up", Duration::ZERO, Instant::now()).await;
        assert_eq!(error.name, ErrorName::DeadlineExceeded);
        assert_eq!(
            error.detail,
            "typed 1 of 2 characters; then native typing took longer than its 3s"
        );
    }

    // A key refused because the lock state's last read is too old, once another key went
    // out, must not tell the agent to try once more: the call would type that key again.
    #[tokio::test]
    async fn a_stale_lock_read_after_a_key_went_out_isnt_called_safe_to_repeat() {
        let ends = Instant::now() + MIN_TIME;
        let error = second_key_refused("native-stale", Duration::from_secs(1), ends).await;
        assert_eq!(error.name, ErrorName::LockStale);
        assert!(error.may_have_sent);
        assert!(
            error
                .detail
                .starts_with("typed 1 of 2 characters; then the lock state was last read over"),
            "{}",
            error.detail
        );
        assert!(!error.detail.contains("try once more"), "{}", error.detail);
    }

    /// A call gets 50 ms a key, here 7.5 s, and a short one 3 s; past them, the
    /// next key is refused with `deadline_exceeded`.
    #[tokio::test(start_paused = true)]
    async fn a_call_gets_its_keys_time_and_at_least_three_seconds() {
        let budget = Budget::start(150);
        let short = Budget::start(2);
        tokio::time::advance(Duration::from_millis(2900)).await;
        short.check().unwrap();
        tokio::time::advance(Duration::from_millis(4500)).await;
        budget.check().unwrap();
        short.check().unwrap_err();
        tokio::time::advance(Duration::from_millis(100)).await;
        let error = budget.check().unwrap_err();
        assert_eq!(error.name, ErrorName::DeadlineExceeded);
    }

    fn device() -> Device {
        Device {
            keyboard: None,
            marker: None,
            group: 0,
            socket: Socket::unknown("no niri in this test"),
            aftercare: None,
            ends: None,
        }
    }

    #[tokio::test]
    async fn a_paste_key_its_keeper_doesnt_admit_is_never_committed() {
        let dir = fresh_dir("commit-unadmitted");
        let mut device = device();
        let (aftercare, _done) = fake_aftercare(STOPPED_WAITING).await;
        let mut aftercare = Some(aftercare);
        let desk = desk_read(&dir, Duration::ZERO);
        let error = device.commit(&mut aftercare, &desk).await.unwrap_err();
        assert!(
            error.detail.contains("nothing was pasted"),
            "{}",
            error.detail
        );
        // The stroke after the commit doesn't run, and nothing says the key went out.
        assert!(aftercare.is_none() && device.aftercare.is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// The user's stop key sets the stop flag while the keeper admits the paste key, after
    /// the stop was last checked: the key is never committed, and the keeper hears `n` and
    /// restores the clipboard.
    #[tokio::test]
    async fn a_paste_key_stopped_while_its_keeper_admits_it_is_never_committed() {
        let dir = fresh_dir("commit-stopped");
        let mut device = device();
        let (aftercare, done) = fake_aftercare(SLOW_ADMISSION).await;
        let mut aftercare = Some(aftercare);
        let desk = desk_read(&dir, Duration::ZERO);
        let runtime = RuntimeDir::of(&crate::test_support::niri_env(&dir)).unwrap();
        let stopper = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            runtime.stop().unwrap();
        });
        let error = device.commit(&mut aftercare, &desk).await.unwrap_err();
        stopper.await.unwrap();
        assert_eq!(error.name, ErrorName::Stopped, "{}", error.detail);
        assert!(aftercare.is_none() && device.aftercare.is_none());
        assert_eq!(
            done.await.unwrap().unwrap(),
            Report::Done {
                read: false,
                clipboard: Clipboard::Restored,
                detail: Some("after n".to_owned())
            }
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// The keeper's admission may outlast the lock read: the key is then never committed,
    /// and the keeper hears `n` and restores the clipboard.
    #[tokio::test]
    async fn a_paste_key_admitted_after_the_lock_read_aged_is_never_committed() {
        let dir = fresh_dir("commit-aged-lock");
        let mut device = device();
        let (aftercare, done) = fake_aftercare(SLOW_ADMISSION).await;
        let mut aftercare = Some(aftercare);
        // Fresh when the `k` goes out, older than `LAST_GATE` once the keeper admits.
        let desk = desk_read(&dir, Duration::from_millis(200));
        let error = device.commit(&mut aftercare, &desk).await.unwrap_err();
        assert_eq!(error.name, ErrorName::LockStale, "{}", error.detail);
        assert!(aftercare.is_none() && device.aftercare.is_none());
        assert_eq!(
            done.await.unwrap().unwrap(),
            Report::Done {
                read: false,
                clipboard: Clipboard::Restored,
                detail: Some("after n".to_owned())
            }
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A paste's call on the fake, with `setup`'s policy and lock state, whose keeper
    /// admits the key only once `meanwhile` has run with the fake and the sender of niri's
    /// event stream, and the keeper's last report.
    async fn pasted_while_admitting(
        test: &str,
        setup: Setup,
        meanwhile: impl FnOnce(&Compositor, &broadcast::Sender<Update>),
    ) -> (Result<Outcome, CallError>, Report, Vec<(u32, bool)>) {
        let dir = fresh_dir(test);
        let niri = Compositor::start(&dir, BASE);
        let (waiter, updates) = events::waiter(events::view_of(initial_events(0)), 0);
        let (k, go) = (dir.join("k"), dir.join("go"));
        let script = format!(
            r#"dd bs=1 count=1 status=none >/dev/null; touch '{}'; while [ ! -e '{}' ]; do sleep 0.01; done; echo '{{"report":"armed"}}'; c=$(dd bs=1 count=1 status=none); echo "{{\"report\":\"done\",\"read\":false,\"clipboard\":\"restored\",\"detail\":\"after $c\"}}""#,
            k.display(),
            go.display()
        );
        let (aftercare, done) = fake_aftercare(&script).await;
        let admitting = async {
            while !k.exists() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            meanwhile(&niri, &updates);
            std::fs::write(&go, "").unwrap();
        };
        let setup = Setup {
            aftercare: Some(aftercare),
            ..setup
        };
        // The fixture's layouts have no `v`; any combination stands in for the paste key.
        let paste = Typing::Keys(vec!["ctrl+a".to_owned()]);
        let (typed, ()) = tokio::join!(
            type_with(&niri, &dir, paste, Some(waiter), setup),
            admitting
        );
        let report = done.await.unwrap().unwrap();
        marker_cleared(&niri, &dir).await;
        let keys = niri.seen().keys;
        std::fs::remove_dir_all(dir).unwrap();
        (typed, report, keys)
    }

    /// `meanwhile` for `pasted_while_admitting`: niri's stream reports `event`.
    fn reported(event: Event) -> impl FnOnce(&Compositor, &broadcast::Sender<Update>) {
        move |_, updates| {
            updates.send(Update::Event(1, Box::new(event))).ok();
        }
    }

    /// The keeper's last report once it heard `n` after admitting the key.
    fn withdrawn() -> Report {
        Report::Done {
            read: false,
            clipboard: Clipboard::Restored,
            detail: Some("after n".to_owned()),
        }
    }

    /// The user focuses another window while the keeper admits the paste key, after niri
    /// last confirmed focus: the key doesn't go out, the keeper hears `n` and restores the
    /// clipboard, and the call ends `interrupted` with nothing pressed.
    #[tokio::test]
    async fn focus_moving_while_the_keeper_admits_the_paste_key_keeps_it_back() {
        let moved = Event::WindowFocusChanged { id: Some(2) };
        let (typed, report, keys) =
            pasted_while_admitting("paste-admit-moved", PLAIN, reported(moved)).await;
        assert_eq!(keys, []);
        assert_eq!(report, withdrawn());
        let outcome = typed.unwrap();
        assert_eq!(
            (outcome.observed, outcome.pressed),
            (Observed::Interrupted, Some(0))
        );
    }

    /// The focused window's app becomes one the policy denies while the keeper admits the
    /// paste key: the key doesn't go out, the keeper hears `n`, and the call is refused
    /// with `app_denied`, nothing sent.
    #[tokio::test]
    async fn an_app_denied_while_the_keeper_admits_the_paste_key_gets_none() {
        let view = events::view_of(initial_events(0));
        let mut window = view.windows()[&1].clone();
        window.app_id = Some("b".to_owned());
        let denied = Event::WindowOpenedOrChanged { window };
        let policy = Loaded::Valid(policy::parse("deny_input_app_ids = [\"b\"]\n", false).unwrap());
        let setup = Setup { policy, ..PLAIN };
        let (typed, report, keys) =
            pasted_while_admitting("paste-admit-denied", setup, reported(denied)).await;
        assert_eq!(keys, []);
        assert_eq!(report, withdrawn());
        let Err(CallError::Tool(error)) = typed else {
            panic!("the paste key wasn't refused");
        };
        assert_eq!(error.name, ErrorName::AppDenied);
        assert!(!error.may_have_sent);
    }

    /// niri gives another window focus while the keeper admits the paste key, and its
    /// event stream hasn't said so when the key would go out: niri's own answer, asked
    /// after the admission, keeps the key back, the keeper hears `n`, and the call ends
    /// `interrupted` with nothing pressed.
    #[tokio::test]
    async fn focus_only_niri_reports_while_the_keeper_admits_the_paste_key_keeps_it_back() {
        let (typed, report, keys) =
            pasted_while_admitting("paste-admit-quiet", PLAIN, |niri, _| {
                niri.focus_quietly(Some(2));
            })
            .await;
        assert_eq!(keys, []);
        assert_eq!(report, withdrawn());
        let outcome = typed.unwrap();
        assert_eq!(
            (outcome.observed, outcome.pressed, outcome.focused_window),
            (Observed::Interrupted, Some(0), Some(2))
        );
    }

    /// niri's answer after the keeper admitted the paste key comes once the lock read is
    /// older than `LAST_GATE`, and the screen locks meanwhile: the lock is read again before
    /// the key, which then doesn't go out, the keeper hears `n`, and the call is refused
    /// with `screen_locked`, nothing sent. A refusal for the aged read alone would be
    /// `lock_stale`.
    #[tokio::test]
    async fn a_lock_read_aged_after_the_paste_key_was_admitted_is_read_again() {
        // The first read is the call's own; the second, the one after admission.
        let setup = Setup {
            locked_from: Some(2),
            ..PLAIN
        };
        let (typed, report, keys) = pasted_while_admitting("paste-admit-aged", setup, |niri, _| {
            niri.answer_focus_late(crate::control::desk::LAST_GATE + Duration::from_millis(50));
        })
        .await;
        assert_eq!(keys, []);
        assert_eq!(report, withdrawn());
        let Err(CallError::Tool(error)) = typed else {
            panic!("the paste key went out on an aged lock read");
        };
        assert_eq!(error.name, ErrorName::ScreenLocked, "{}", error.detail);
        assert!(!error.may_have_sent);
    }
}
