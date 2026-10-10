//! The input tools' work: the pointer tools in `pointer`, the keyboard tools in
//! `keyboard`, and `paste`, with its clipboard keeper in `keeper`, which presses its key
//! through `keyboard`. All refuse input to an app on the policy's deny list, and all write
//! the input-dirty marker before anything that could leave input held.

mod held;
pub(crate) mod keeper;
pub(crate) mod keyboard;
mod keymap;
mod native;
pub(crate) mod paste;
pub(crate) mod pointer;

use crate::act::Niri;
use crate::control::runtime::RuntimeDir;
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
    /// The accessibility bus, when this session has one.
    pub(crate) a11y: Option<&'a crate::a11y::A11y>,
}

/// The `app_id` of the window with keyboard focus, if it has one.
pub(super) fn focused_app_id(view: &View) -> Option<&str> {
    view.windows()
        .get(&view.focused_window()?)?
        .app_id
        .as_deref()
}
