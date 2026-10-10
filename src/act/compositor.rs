//! `niri_action`: any niri action, in niri's own JSON form, followed by the state niri then
//! reports for the window the action is about. niri 26.04's IPC has no fullscreen or
//! maximized flag, so those show as sizes.

use std::time::Duration;

use niri_ipc::{Action, Window};
use serde::Serialize;

use super::{Niri, Observed, Outcome, no_window, send};
use crate::error::CallError;
use crate::niri;
use crate::niri::waiter::{View, Waited};

/// How long to wait for niri to report a change in the window.
const OBSERVE: Duration = Duration::from_secs(1);
/// How long to keep reading events after the first change, since a resize can come in
/// steps: niri's layout first, then the app's new size.
const SETTLE: Duration = Duration::from_millis(200);

/// What niri reports about a window's place and size.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct WindowState {
    pub(crate) id: u64,
    pub(crate) workspace_id: Option<u64>,
    pub(crate) is_focused: bool,
    pub(crate) is_floating: bool,
    pub(crate) is_urgent: bool,
    /// The window's own size in logical pixels, without niri's borders.
    pub(crate) window_size: (i32, i32),
    /// The tile's size, borders included.
    pub(crate) tile_size: (f64, f64),
    /// Column and row in the scrolling layout, from 1; null while floating.
    pub(crate) pos_in_scrolling_layout: Option<(usize, usize)>,
}

impl From<&Window> for WindowState {
    fn from(window: &Window) -> Self {
        Self {
            id: window.id,
            workspace_id: window.workspace_id,
            is_focused: window.is_focused,
            is_floating: window.is_floating,
            is_urgent: window.is_urgent,
            window_size: window.layout.window_size,
            tile_size: window.layout.tile_size,
            pos_in_scrolling_layout: window.layout.pos_in_scrolling_layout,
        }
    }
}

/// Which window an action is about.
#[derive(Debug, PartialEq, Eq)]
enum Aim {
    /// No one window, as for focusing a column or a workspace.
    None,
    /// The focused window, since the action names none.
    Focused,
    Window(u64),
}

/// The window an action acts on and changes in a way niri reports. Screenshots, casts and
/// the opacity rule name a window but change nothing niri reports about it.
#[expect(
    clippy::too_many_lines,
    reason = "one exhaustive match over niri's actions without a wildcard arm, so an action a \
              new niri-ipc adds must be placed here"
)]
const fn aim(action: &Action) -> Aim {
    let id = match action {
        Action::FocusWindow { id }
        | Action::ToggleWindowUrgent { id }
        | Action::SetWindowUrgent { id }
        | Action::UnsetWindowUrgent { id } => Some(*id),
        Action::CloseWindow { id }
        | Action::FullscreenWindow { id }
        | Action::ToggleWindowedFullscreen { id }
        | Action::CenterWindow { id }
        | Action::ResetWindowHeight { id }
        | Action::SwitchPresetWindowWidth { id }
        | Action::SwitchPresetWindowWidthBack { id }
        | Action::SwitchPresetWindowHeight { id }
        | Action::SwitchPresetWindowHeightBack { id }
        | Action::MaximizeWindowToEdges { id }
        | Action::ToggleWindowFloating { id }
        | Action::MoveWindowToFloating { id }
        | Action::MoveWindowToTiling { id }
        | Action::ConsumeOrExpelWindowLeft { id }
        | Action::ConsumeOrExpelWindowRight { id }
        | Action::MoveWindowToMonitor { id, .. }
        | Action::SetWindowWidth { id, .. }
        | Action::SetWindowHeight { id, .. }
        | Action::MoveFloatingWindow { id, .. }
        | Action::MoveWindowToWorkspace { window_id: id, .. } => *id,
        Action::Quit { .. }
        | Action::PowerOffMonitors { .. }
        | Action::PowerOnMonitors { .. }
        | Action::Spawn { .. }
        | Action::SpawnSh { .. }
        | Action::DoScreenTransition { .. }
        | Action::Screenshot { .. }
        | Action::ScreenshotScreen { .. }
        | Action::ScreenshotWindow { .. }
        | Action::ToggleKeyboardShortcutsInhibit { .. }
        | Action::FocusWindowInColumn { .. }
        | Action::FocusWindowPrevious { .. }
        | Action::FocusColumnLeft { .. }
        | Action::FocusColumnRight { .. }
        | Action::FocusColumnFirst { .. }
        | Action::FocusColumnLast { .. }
        | Action::FocusColumnRightOrFirst { .. }
        | Action::FocusColumnLeftOrLast { .. }
        | Action::FocusColumn { .. }
        | Action::FocusWindowOrMonitorUp { .. }
        | Action::FocusWindowOrMonitorDown { .. }
        | Action::FocusColumnOrMonitorLeft { .. }
        | Action::FocusColumnOrMonitorRight { .. }
        | Action::FocusWindowDown { .. }
        | Action::FocusWindowUp { .. }
        | Action::FocusWindowDownOrColumnLeft { .. }
        | Action::FocusWindowDownOrColumnRight { .. }
        | Action::FocusWindowUpOrColumnLeft { .. }
        | Action::FocusWindowUpOrColumnRight { .. }
        | Action::FocusWindowOrWorkspaceDown { .. }
        | Action::FocusWindowOrWorkspaceUp { .. }
        | Action::FocusWindowTop { .. }
        | Action::FocusWindowBottom { .. }
        | Action::FocusWindowDownOrTop { .. }
        | Action::FocusWindowUpOrBottom { .. }
        | Action::MoveColumnLeft { .. }
        | Action::MoveColumnRight { .. }
        | Action::MoveColumnToFirst { .. }
        | Action::MoveColumnToLast { .. }
        | Action::MoveColumnLeftOrToMonitorLeft { .. }
        | Action::MoveColumnRightOrToMonitorRight { .. }
        | Action::MoveColumnToIndex { .. }
        | Action::MoveWindowDown { .. }
        | Action::MoveWindowUp { .. }
        | Action::MoveWindowDownOrToWorkspaceDown { .. }
        | Action::MoveWindowUpOrToWorkspaceUp { .. }
        | Action::ConsumeWindowIntoColumn { .. }
        | Action::ExpelWindowFromColumn { .. }
        | Action::SwapWindowRight { .. }
        | Action::SwapWindowLeft { .. }
        | Action::ToggleColumnTabbedDisplay { .. }
        | Action::SetColumnDisplay { .. }
        | Action::CenterColumn { .. }
        | Action::CenterVisibleColumns { .. }
        | Action::FocusWorkspaceDown { .. }
        | Action::FocusWorkspaceUp { .. }
        | Action::FocusWorkspace { .. }
        | Action::FocusWorkspacePrevious { .. }
        | Action::MoveWindowToWorkspaceDown { .. }
        | Action::MoveWindowToWorkspaceUp { .. }
        | Action::MoveColumnToWorkspaceDown { .. }
        | Action::MoveColumnToWorkspaceUp { .. }
        | Action::MoveColumnToWorkspace { .. }
        | Action::MoveWorkspaceDown { .. }
        | Action::MoveWorkspaceUp { .. }
        | Action::MoveWorkspaceToIndex { .. }
        | Action::SetWorkspaceName { .. }
        | Action::UnsetWorkspaceName { .. }
        | Action::FocusMonitorLeft { .. }
        | Action::FocusMonitorRight { .. }
        | Action::FocusMonitorDown { .. }
        | Action::FocusMonitorUp { .. }
        | Action::FocusMonitorPrevious { .. }
        | Action::FocusMonitorNext { .. }
        | Action::FocusMonitor { .. }
        | Action::MoveWindowToMonitorLeft { .. }
        | Action::MoveWindowToMonitorRight { .. }
        | Action::MoveWindowToMonitorDown { .. }
        | Action::MoveWindowToMonitorUp { .. }
        | Action::MoveWindowToMonitorPrevious { .. }
        | Action::MoveWindowToMonitorNext { .. }
        | Action::MoveColumnToMonitorLeft { .. }
        | Action::MoveColumnToMonitorRight { .. }
        | Action::MoveColumnToMonitorDown { .. }
        | Action::MoveColumnToMonitorUp { .. }
        | Action::MoveColumnToMonitorPrevious { .. }
        | Action::MoveColumnToMonitorNext { .. }
        | Action::MoveColumnToMonitor { .. }
        | Action::SwitchPresetColumnWidth { .. }
        | Action::SwitchPresetColumnWidthBack { .. }
        | Action::MaximizeColumn { .. }
        | Action::SetColumnWidth { .. }
        | Action::ExpandColumnToAvailableWidth { .. }
        | Action::SwitchLayout { .. }
        | Action::ShowHotkeyOverlay { .. }
        | Action::MoveWorkspaceToMonitorLeft { .. }
        | Action::MoveWorkspaceToMonitorRight { .. }
        | Action::MoveWorkspaceToMonitorDown { .. }
        | Action::MoveWorkspaceToMonitorUp { .. }
        | Action::MoveWorkspaceToMonitorPrevious { .. }
        | Action::MoveWorkspaceToMonitorNext { .. }
        | Action::MoveWorkspaceToMonitor { .. }
        | Action::ToggleDebugTint { .. }
        | Action::DebugToggleOpaqueRegions { .. }
        | Action::DebugToggleDamage { .. }
        | Action::FocusFloating { .. }
        | Action::FocusTiling { .. }
        | Action::SwitchFocusBetweenFloatingAndTiling { .. }
        | Action::ToggleWindowRuleOpacity { .. }
        | Action::SetDynamicCastWindow { .. }
        | Action::SetDynamicCastMonitor { .. }
        | Action::ClearDynamicCastTarget { .. }
        | Action::StopCast { .. }
        | Action::ToggleOverview { .. }
        | Action::OpenOverview { .. }
        | Action::CloseOverview { .. }
        | Action::LoadConfigFile { .. } => return Aim::None,
    };
    match id {
        Some(id) => Aim::Window(id),
        None => Aim::Focused,
    }
}

/// Sends `action`, then, for an action about one window, reports that window as niri
/// shows it once it changed, or after a second: `changed`, `unchanged` or `closed`, with
/// the window's state. Any other action is `sent`.
pub(crate) async fn run(niri: Niri<'_>, action: Action) -> Result<Outcome, CallError> {
    let mut waiter = niri::waiter(niri.events).await?;
    let target = match aim(&action) {
        Aim::None => None,
        Aim::Focused => waiter.view().focused_window(),
        Aim::Window(id) if waiter.view().windows().contains_key(&id) => Some(id),
        Aim::Window(id) => return Err(no_window(id)),
    };
    let before = target.and_then(|id| state(waiter.view(), id));
    if let Some(lost) = send(niri.socket, action).await? {
        return Ok(lost);
    }
    let Some(before) = before else {
        return Ok(Outcome::seen(Observed::Sent, waiter.view(), Vec::new()));
    };
    let id = before.id;
    let first = waiter
        .until(OBSERVE, |view| {
            (state(view, id).as_ref() != Some(&before)).then_some(())
        })
        .await;
    let ended = match first {
        Waited::Done(()) => waiter.until(SETTLE, |_| None::<()>).await,
        Waited::Timeout | Waited::Lost(_) => first,
    };
    if let Waited::Lost(reason) = ended {
        return Ok(Outcome::uncertain(Some(true), Some(waiter.view()), reason));
    }
    let now = state(waiter.view(), id);
    Ok(Outcome {
        window: now.clone(),
        ..Outcome::seen(compare(&before, now.as_ref()), waiter.view(), vec![id])
    })
}

fn state(view: &View, id: u64) -> Option<WindowState> {
    view.windows().get(&id).map(WindowState::from)
}

fn compare(before: &WindowState, now: Option<&WindowState>) -> Observed {
    match now {
        None => Observed::Closed,
        Some(now) if now == before => Observed::Unchanged,
        Some(_) => Observed::Changed,
    }
}

#[cfg(test)]
mod tests {
    use niri_ipc::{SizeChange, WorkspaceReferenceArg};

    use super::*;

    #[test]
    fn an_action_is_about_the_window_it_names_or_the_focused_one() {
        assert_eq!(
            aim(&Action::FullscreenWindow { id: Some(12) }),
            Aim::Window(12)
        );
        assert_eq!(
            aim(&Action::SetWindowWidth {
                id: None,
                change: SizeChange::SetFixed(1600)
            }),
            Aim::Focused
        );
        assert_eq!(
            aim(&Action::MoveWindowToWorkspace {
                window_id: Some(4),
                reference: WorkspaceReferenceArg::Index(2),
                focus: false
            }),
            Aim::Window(4)
        );
        assert_eq!(aim(&Action::FocusWindow { id: 3 }), Aim::Window(3));
        assert_eq!(aim(&Action::MaximizeColumn {}), Aim::None);
        assert_eq!(
            aim(&Action::ScreenshotWindow {
                id: Some(3),
                write_to_disk: false,
                show_pointer: false,
                path: None
            }),
            Aim::None
        );
    }
}
