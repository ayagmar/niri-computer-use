//! Native modifiers shared with a pointer gesture's single dirty marker.

use super::{Input, keyboard::parse_combo, keymap::held_mask, native};
use crate::control::marker::Native;
use crate::error::{CallError, ErrorName, ToolError};
use crate::niri::{self, Socket, keyboard::Keyboard, waiter::View};

#[derive(Debug)]
pub(super) struct Held {
    keyboard: Keyboard,
    /// niri's socket, asked for the active layout before the layout is restored.
    socket: Socket,
    mask: u32,
    /// The layout niri reported when the gesture began.
    group: u32,
    active: bool,
}

pub(super) fn check(keys: &[String]) -> Result<(), CallError> {
    if keys.is_empty() {
        return Ok(());
    }
    if keys.len() > 5 {
        return Err(CallError::InvalidArguments(
            "keys takes at most five held modifiers".into(),
        ));
    }
    parse_combo(&format!("{}+a", keys.join("+"))).map_err(CallError::InvalidArguments)?;
    Ok(())
}

impl Held {
    pub(super) async fn prepare(
        input: Input<'_>,
        view: &View,
        keys: &[String],
    ) -> Result<Option<Self>, CallError> {
        if keys.is_empty() {
            return Ok(None);
        }
        if input.keyboard.and_then(std::ffi::OsStr::to_str) != Some("native") {
            return Err(ToolError::new(
                ErrorName::Refused,
                "held pointer keys require NIRI_COMPUTER_USE_KEYBOARD=native",
            )
            .into());
        }
        let display = input.display.path().map_err(Clone::clone)?;
        let keyboard = Keyboard::bind(display, niri::pid(input.niri.socket).await?).await?;
        let group = view.keyboard_group().ok_or_else(|| {
            ToolError::new(
                ErrorName::NiriUnavailable,
                "the active keyboard layout is unknown",
            )
        })?;
        let combo = format!("{}+a", keys.join("+"));
        let names = parse_combo(&combo).map_err(CallError::InvalidArguments)?.0;
        let mask = held_mask(keyboard.map()?, &names)?;
        Ok(Some(Self {
            keyboard,
            socket: input.niri.socket.clone(),
            mask,
            group,
            active: false,
        }))
    }

    pub(super) const fn marker(&self) -> Native {
        Native {
            codes: Vec::new(),
            group: self.group,
        }
    }

    pub(super) async fn begin(&mut self) -> Result<(), ToolError> {
        self.active = true;
        self.keyboard.modifiers(self.mask, self.group)?;
        self.keyboard.sync().await
    }

    pub(super) fn release_now(&mut self) -> bool {
        if self.keyboard.release_now(&[], self.group).is_err() {
            return false;
        }
        self.active = false;
        true
    }

    /// Releases the modifiers, then puts the latest compositor keymap back in the layout
    /// niri has active by now, as native typing's cleanup does. An error, the restore
    /// unproved included, leaves the gesture's marker for `recover`.
    pub(super) async fn released(&mut self) -> Result<(), ToolError> {
        self.keyboard.release(&[], self.group).await?;
        self.active = false;
        native::restore_active(&mut self.keyboard, &self.socket, self.group).await
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        if self.active {
            self.release_now();
        }
    }
}
