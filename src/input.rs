//! The input tools' work: the pointer tools in `pointer`, the keyboard tools in
//! `keyboard`, and `paste`, with its clipboard keeper in `keeper`, which presses its key
//! through `keyboard`. All refuse input to an app on the policy's deny list, and all write
//! the input-dirty marker before anything that could leave input held.

mod held;
pub(crate) mod keeper;
pub(crate) mod keyboard;
pub(crate) mod keymap;
mod native;
pub(crate) mod paste;
pub(crate) mod pointer;

use crate::act::Niri;
use crate::control::runtime::RuntimeDir;
use crate::error::{ErrorName, ToolError};
use crate::niri::Display;
use crate::niri::waiter::View;
use crate::policy::Loaded;

/// What the input tools need besides their arguments.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Input<'a> {
    pub(crate) niri: Niri<'a>,
    /// The Wayland display, checked against niri at each use.
    pub(crate) display: &'a Display,
    pub(crate) runtime: &'a RuntimeDir,
    pub(crate) policy: &'a Loaded,
    pub(crate) keyboard: Option<&'a std::ffi::OsStr>,
    /// Where the session's `wtype` is found.
    pub(crate) place: crate::runner::Place<'a>,
    /// The accessibility bus, when this session has one.
    pub(crate) a11y: Option<&'a crate::a11y::A11y>,
}

/// Whether keys go out through the native keyboard, as they do unless `keyboard`, the
/// session's `NIRI_COMPUTER_USE_KEYBOARD`, is `wtype`.
fn native_keyboard(keyboard: Option<&std::ffi::OsStr>) -> Result<bool, ToolError> {
    match keyboard.map(std::ffi::OsStr::to_str) {
        None | Some(Some("native")) => Ok(true),
        Some(Some("wtype")) => Ok(false),
        Some(_) => Err(ToolError::new(
            ErrorName::Refused,
            "NIRI_COMPUTER_USE_KEYBOARD must be native or wtype",
        )),
    }
}

/// The `app_id` of the window with keyboard focus, if it has one.
pub(super) fn focused_app_id(view: &View) -> Option<&str> {
    view.windows()
        .get(&view.focused_window()?)?
        .app_id
        .as_deref()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_native_keyboard_is_the_default_and_wtype_is_chosen_by_name() {
        assert!(native_keyboard(None).unwrap());
        assert!(native_keyboard(Some("native".as_ref())).unwrap());
        assert!(!native_keyboard(Some("wtype".as_ref())).unwrap());
        let error = native_keyboard(Some("Native".as_ref())).unwrap_err();
        assert_eq!(error.name, ErrorName::Refused);
    }
}
