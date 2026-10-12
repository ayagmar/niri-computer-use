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
    let mut waiter = niri::waiter(input.niri.events).await?;
    let focus = check_expect(&expect, waiter.view())?;
    if let Some(refused) = policy::refuse_input(input.policy, focused_app_id(waiter.view())) {
        return Err(refused.into());
    }
    let (keyboard, group, plan) = bind(input, waiter.view(), &typing).await?;
    let keys = plan.keys;
    let revision = keyboard.revision();
    let before = waiter.view().focused_window();
    let mut device = Device::new(keyboard, input, &typing, &keys, group).await?;
    if let Some(map) = &plan.extended {
        device.keyboard()?.extend(map)?;
    }
    // `paste`'s keeper reports whether its key went out; the keyboard tools tell the desk.
    let marks = aftercare.is_none();
    let mut sent = Sent::default();
    for key in keys {
        act::fresh_lock(input.niri)
            .await
            .map_err(|error| partly(error, &typing, sent))?;
        // niri's events since the lock was read or the last key, right before this one.
        if let Some(outcome) = interrupted(&mut waiter, before, group).await {
            device.finish(waiter.view().keyboard_group()).await?;
            return Ok(ended(outcome, focus, &typing, sent));
        }
        may_press(input, waiter.view(), &typing, sent, marks).await?;
        device.commit(aftercare).await?;
        device.stroke(key).await?;
        sent.add_key(key.character);
        // Counts only once `may_press` told the desk the key was going out.
        let answered = typed_so_far(waiter.view(), focus, &typing, sent);
        input.niri.desk.answered(answered);
        if device.revision()? != revision {
            device.finish(waiter.view().keyboard_group()).await?;
            return Ok(ended(keymap_changed(waiter.view()), focus, &typing, sent));
        }
    }
    device.finish(waiter.view().keyboard_group()).await?;
    let outcome = interrupted(&mut waiter, before, group)
        .await
        .unwrap_or_else(|| Outcome::seen(Observed::Sent, waiter.view(), Vec::new()));
    Ok(ended(outcome, focus, &typing, sent))
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

/// Whether the next key may go out: the stop flag isn't set, the focused app in `view` isn't
/// denied, and the lock state was read at most `LAST_GATE` ago. With `marks`, tells the
/// desk the key is going out.
async fn may_press(
    input: Input<'_>,
    view: &View,
    typing: &Typing,
    sent: Sent,
    marks: bool,
) -> Result<(), ToolError> {
    if input
        .runtime
        .stopped()
        .map_err(|error| upstream(&format!("read stop flag: {error}")))?
    {
        if marks && !sent.nothing() {
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
        return Err(partly(refused, typing, sent));
    }
    let unlocked = if marks {
        input.niri.desk.sending_input()
    } else {
        input.niri.desk.fresh()
    };
    unlocked.map_err(|error| partly(error, typing, sent))
}

async fn interrupted(waiter: &mut Waiter, before: Option<u64>, group: u32) -> Option<Outcome> {
    match waiter
        .until(Duration::ZERO, |view| {
            (view.focused_window() != before || view.keyboard_group() != Some(group)).then_some(())
        })
        .await
    {
        Waited::Timeout => None,
        Waited::Done(()) => Some(Outcome::seen(
            Observed::Interrupted,
            waiter.view(),
            Vec::new(),
        )),
        Waited::Lost(reason) => Some(Outcome::uncertain(Some(true), Some(waiter.view()), reason)),
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
        keys: &[Key],
        group: u32,
    ) -> Result<Self, ToolError> {
        let mut codes: Vec<u32> = keys.iter().map(|key| key.code.0).collect();
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

    /// Takes `aftercare`, if it is still there, and arms it.
    async fn commit(&mut self, aftercare: &mut Option<Aftercare>) -> Result<(), ToolError> {
        let Some(taken) = aftercare.take() else {
            return Ok(());
        };
        self.aftercare = Some(taken.arm().await?);
        Ok(())
    }

    async fn stroke(&mut self, key: Key) -> Result<(), ToolError> {
        let keyboard = self.keyboard()?;
        keyboard.modifiers(key.modifiers, key.group)?;
        keyboard.key(key.code.0, true)?;
        keyboard.key(key.code.0, false)?;
        keyboard.modifiers(0, key.group)?;
        keyboard.sync().await
    }

    /// Releases everything and puts the latest compositor keymap back in the layout niri
    /// has active, or without niri's answer in the one its event stream last reported,
    /// `group`, if it reported one: a layout the user switched to meanwhile stays. Then
    /// clears the marker. With a paste's aftercare, that runs in a task of its own, so a
    /// dropped call can't leave the keeper without its `p`.
    async fn finish(mut self, group: Option<u32>) -> Result<(), ToolError> {
        if let Some(group) = group {
            self.group = group;
        }
        let Some(aftercare) = self.aftercare.take() else {
            self.release().await?;
            return self.clear().await;
        };
        let ends = cleanup::deadline();
        self.ends = Some(ends);
        let finished = cleanup::spawn(ends, async move {
            self.release().await?;
            aftercare.sent().await;
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
    use crate::input::paste::{STOPPED_WAITING, fake_aftercare};

    #[tokio::test]
    async fn a_paste_key_its_keeper_doesnt_admit_is_never_committed() {
        let mut device = Device {
            keyboard: None,
            marker: None,
            group: 0,
            socket: Socket::unknown("no niri in this test"),
            aftercare: None,
            ends: None,
        };
        let (aftercare, _done) = fake_aftercare(STOPPED_WAITING).await;
        let mut aftercare = Some(aftercare);
        let error = device.commit(&mut aftercare).await.unwrap_err();
        assert!(
            error.detail.contains("nothing was pasted"),
            "{}",
            error.detail
        );
        // The stroke after the commit doesn't run, and nothing says the key went out.
        assert!(aftercare.is_none() && device.aftercare.is_none());
    }
}
