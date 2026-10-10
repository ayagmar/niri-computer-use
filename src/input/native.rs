//! Native per-key input. A dirty marker covers every possible press until release is
//! acknowledged and, after an extended keymap, niri has sent the compositor's map back.
//! SIGKILL requires recover; destruction alone releases nothing in niri. A paste's
//! `Aftercare` is armed right before the first stroke and told the key went out once the
//! release is acknowledged, before the marker comes off; a release niri never acknowledged
//! drops it, so the keeper keeps the pasted text.

use std::time::Duration;

use super::keyboard::{Expect, Sent, Typing, check_expect, ended};
use super::keymap::{Key, plan};
use super::paste::Aftercare;
use super::{Input, focused_app_id};
use crate::act::{Observed, Outcome};
use crate::control::cleanup::Pending;
use crate::control::marker::{Marker, Native, Written};
use crate::error::{CallError, ErrorName, ToolError};
use crate::niri::{
    self, Socket,
    keyboard::Keyboard,
    waiter::{Waited, Waiter},
};
use crate::policy;

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
    let display = input.display.path().map_err(Clone::clone)?;
    let keyboard = Keyboard::bind(display, niri::pid(input.niri.socket).await?).await?;
    let group = waiter.view().keyboard_group().ok_or_else(|| {
        ToolError::new(
            ErrorName::NiriUnavailable,
            "the active keyboard layout is unknown",
        )
    })?;
    let plan = plan(keyboard.map()?, group, &typing)?;
    let keys = plan.keys;
    let revision = keyboard.revision();
    let before = waiter.view().focused_window();
    let mut device = Device::new(keyboard, input, &typing, &keys, group).await?;
    if let Some(map) = &plan.extended {
        device.keyboard()?.extend(map)?;
    }
    let mut sent = Sent::default();
    for key in keys {
        if let Some(outcome) = interrupted(&mut waiter, before, group).await {
            device.finish(waiter.view().keyboard_group()).await?;
            return Ok(ended(outcome, focus, &typing, sent));
        }
        if input
            .runtime
            .stopped()
            .map_err(|error| upstream(&format!("read stop flag: {error}")))?
        {
            return Err(ToolError::new(
                ErrorName::Stopped,
                format!(
                    "stopped after {} characters and {} keys",
                    sent.chars, sent.keys
                ),
            )
            .into());
        }
        device.commit(aftercare).await?;
        device.stroke(key).await?;
        if key.character {
            sent.chars += 1;
        } else {
            sent.keys += 1;
        }
        if device.revision()? != revision {
            device.finish(waiter.view().keyboard_group()).await?;
            return Ok(ended(
                Outcome::uncertain(
                    Some(true),
                    Some(waiter.view()),
                    "the compositor keymap changed during typing".into(),
                ),
                focus,
                &typing,
                sent,
            ));
        }
    }
    device.finish(waiter.view().keyboard_group()).await?;
    let outcome = interrupted(&mut waiter, before, group)
        .await
        .unwrap_or_else(|| Outcome::seen(Observed::Sent, waiter.view(), Vec::new()));
    Ok(ended(outcome, focus, &typing, sent))
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
            .map_err(|error| upstream(&format!("write native input-dirty marker: {error}")))?;
        Ok(Self {
            keyboard: Some(keyboard),
            marker: Some(marker),
            socket: input.niri.socket.clone(),
            group,
            aftercare: None,
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
        let Some(mut taken) = aftercare.take() else {
            return Ok(());
        };
        taken.arm().await?;
        self.aftercare = Some(taken);
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
        let cleanup = Pending::start();
        let finished = tokio::spawn(async move {
            let released = self.release().await;
            if released.is_ok() {
                aftercare.sent().await;
            }
            let finished = match released {
                Ok(()) => self.clear().await,
                Err(error) => Err(error),
            };
            drop(cleanup);
            finished
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
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            let aftercare = self.aftercare.take();
            runtime.spawn(acknowledge_release(
                keyboard,
                marker,
                aftercare,
                (self.socket.clone(), self.group),
                Pending::start(),
            ));
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
    _cleanup: Pending,
) {
    if keyboard.sync().await.is_err()
        || restore_active(&mut keyboard, &layout.0, layout.1)
            .await
            .is_err()
        || !keyboard.restored()
    {
        return;
    }
    if let Some(aftercare) = aftercare {
        aftercare.sent().await;
    }
    marker.clear().await.ok();
}

/// Restores the base map with zero modifiers in the layout niri has active, or without
/// niri's answer in `sent`, and sends the modifiers once more after niri has taken it:
/// in the nested trials, this device saw the map a dropped call restored come back only
/// after a further input, and the client sometimes got no modifiers event for one of the
/// two.
async fn restore_active(
    keyboard: &mut Keyboard,
    socket: &Socket,
    sent: u32,
) -> Result<(), ToolError> {
    let group = niri::keyboard_group(socket).await.unwrap_or(sent);
    keyboard.restore_now(group)?;
    keyboard.sync().await?;
    keyboard.modifiers(0, group)?;
    keyboard.sync().await
}

fn upstream(detail: &str) -> ToolError {
    ToolError::new(ErrorName::UpstreamError, detail)
}
