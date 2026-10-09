//! The input tools' work: the pointer tools in `pointer`, the keyboard tools in
//! `keyboard`. Both refuse input to an app on the policy's deny list, and both write the
//! input-dirty marker before anything that could leave input held.

mod held;
pub(crate) mod keyboard;
mod keymap;
mod native;
pub(crate) mod pointer;

use std::path::Path;

use crate::act::Niri;
use crate::control::runtime::RuntimeDir;
use crate::niri::waiter::View;
use crate::policy::Loaded;

/// What the input tools need besides their arguments.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Input<'a> {
    pub(crate) niri: Niri<'a>,
    /// The Wayland display's socket, if known.
    pub(crate) display: Option<&'a Path>,
    pub(crate) runtime: &'a RuntimeDir,
    pub(crate) policy: &'a Loaded,
    pub(crate) keyboard: Option<&'a std::ffi::OsStr>,
}

/// The `app_id` of the window with keyboard focus, if it has one.
pub(super) fn focused_app_id(view: &View) -> Option<&str> {
    view.windows()
        .get(&view.focused_window()?)?
        .app_id
        .as_deref()
}
