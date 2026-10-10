//! Native per-key input. A dirty marker covers every possible press until release is
//! acknowledged and, after an extended keymap, niri has sent the compositor's map back.
//! SIGKILL requires recover; destruction alone releases nothing in niri.

use std::time::Duration;

use super::keyboard::{Expect, Sent, Typing, check_expect, ended};
use super::keymap::{Key, plan};
use super::{Input, focused_app_id};
use crate::act::{Observed, Outcome};
use crate::control::marker::{Marker, Native, Written};
use crate::error::{CallError, ErrorName, ToolError};
use crate::niri::{
    self,
    keyboard::Keyboard,
    waiter::{Waited, Waiter},
};
use crate::policy;

pub(super) async fn type_input(
    input: Input<'_>,
    typing: Typing,
    expect: Expect,
) -> Result<Outcome, CallError> {
    let mut waiter = niri::waiter(input.niri.events).await?;
    let focus = check_expect(&expect, waiter.view())?;
    if let Some(refused) = policy::refuse_input(input.policy, focused_app_id(waiter.view())) {
        return Err(refused.into());
    }
    let display = input.display.ok_or_else(|| {
        ToolError::new(
            ErrorName::UpstreamError,
            "WAYLAND_DISPLAY or XDG_RUNTIME_DIR is not set",
        )
    })?;
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
    let mut device = Device::new(keyboard, input, &typing, &keys, group)?;
    if let Some(map) = &plan.extended {
        device.keyboard()?.extend(map)?;
    }
    let mut sent = Sent::default();
    for key in keys {
        if let Some(outcome) = interrupted(&mut waiter, before, group).await {
            device.finish().await?;
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
        device.stroke(key).await?;
        if key.character {
            sent.chars += 1;
        } else {
            sent.keys += 1;
        }
        if device.revision()? != revision {
            device.finish().await?;
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
    device.finish().await?;
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

#[derive(Debug)]
struct Device {
    keyboard: Option<Keyboard>,
    marker: Option<Written>,
    group: u32,
}

impl Device {
    fn new(
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
            .map_err(|error| upstream(&format!("write native input-dirty marker: {error}")))?;
        Ok(Self {
            keyboard: Some(keyboard),
            marker: Some(marker),
            group,
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

    async fn stroke(&mut self, key: Key) -> Result<(), ToolError> {
        let keyboard = self.keyboard()?;
        keyboard.modifiers(key.modifiers, key.group)?;
        keyboard.key(key.code.0, true)?;
        keyboard.key(key.code.0, false)?;
        keyboard.modifiers(0, key.group)?;
        keyboard.sync().await
    }

    async fn finish(mut self) -> Result<(), ToolError> {
        let group = self.group;
        let keyboard = self.keyboard()?;
        keyboard.release(&[], group).await?;
        keyboard.restore(group).await?;
        if let Some(marker) = self.marker.take() {
            marker
                .clear()
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
        if keyboard.release_now(&[], self.group).is_err()
            || keyboard.restore_now(self.group).is_err()
        {
            return;
        }
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(acknowledge_release(keyboard, marker));
        }
    }
}

async fn acknowledge_release(mut keyboard: Keyboard, marker: Written) {
    if keyboard.sync().await.is_ok() && keyboard.restored() {
        marker.clear().ok();
    }
}

fn upstream(detail: &str) -> ToolError {
    ToolError::new(ErrorName::UpstreamError, detail)
}
