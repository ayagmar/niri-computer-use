//! `niri_action`: any niri action, in niri's own JSON form, followed by the state niri then
//! reports for the window the action is about. niri 26.04's IPC has no fullscreen or
//! maximized flag, so those show as sizes.

use std::time::Duration;

use niri_ipc::{Action, Window};
use serde::Serialize;

use super::record::Quiet;
use super::{Niri, Observed, Outcome, no_window, refuse_close, send};
use crate::error::CallError;
use crate::niri;
use crate::niri::waiter::{View, Waited};
use crate::policy::Loaded;

/// How long to wait for niri to report a change in the window.
const OBSERVE: Duration = Duration::from_secs(1);
/// How long to keep reading events after the first change, since a resize can come in
/// steps: niri's layout first, then the app's new size.
const SETTLE: Duration = Duration::from_millis(200);

/// Whether `action`'s change report waits for quiet: not once its own wait ended with
/// `changed` or `closed`, or after `unchanged`'s full second, and never for `Spawn` and `SpawnSh`, whose window comes later than that wait
/// would cover. An action that isn't one is refused before the report.
pub(crate) const fn quiet(action: Option<&Action>) -> Quiet {
    match action {
        Some(Action::Spawn { .. } | Action::SpawnSh { .. }) => Quiet::Never,
        _ => Quiet::UnlessSeen,
    }
}

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

/// How an action names the window it acts on.
#[derive(Debug, PartialEq, Eq)]
enum Field<'a> {
    /// It acts on no one window, or on the focused one without a field to name it.
    None,
    /// It must name one.
    Window(u64),
    /// It may name one, and acts on the focused one when it doesn't. Whether niri reports
    /// what it changes there: screenshots, casts and the opacity rule change nothing it
    /// reports.
    Optional {
        id: &'a mut Option<u64>,
        reported: bool,
    },
}

/// Where `action` names its window.
#[expect(
    clippy::too_many_lines,
    reason = "one exhaustive match over niri's actions without a wildcard arm, so an action a \
              new niri-ipc adds must be placed here"
)]
const fn field(action: &mut Action) -> Field<'_> {
    match action {
        Action::FocusWindow { id }
        | Action::ToggleWindowUrgent { id }
        | Action::SetWindowUrgent { id }
        | Action::UnsetWindowUrgent { id } => Field::Window(*id),
        Action::ScreenshotWindow { id, .. }
        | Action::ToggleWindowRuleOpacity { id }
        | Action::SetDynamicCastWindow { id } => Field::Optional {
            id,
            reported: false,
        },
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
        | Action::MoveWindowToWorkspace { window_id: id, .. } => {
            Field::Optional { id, reported: true }
        }
        Action::Quit { .. }
        | Action::PowerOffMonitors { .. }
        | Action::PowerOnMonitors { .. }
        | Action::Spawn { .. }
        | Action::SpawnSh { .. }
        | Action::DoScreenTransition { .. }
        | Action::Screenshot { .. }
        | Action::ScreenshotScreen { .. }
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
        | Action::SetDynamicCastMonitor { .. }
        | Action::ClearDynamicCastTarget { .. }
        | Action::StopCast { .. }
        | Action::ToggleOverview { .. }
        | Action::OpenOverview { .. }
        | Action::CloseOverview { .. }
        | Action::LoadConfigFile { .. } => Field::None,
    }
}

/// The window an action acts on and changes in a way niri reports.
const fn aim(action: &mut Action) -> Aim {
    match field(action) {
        Field::None
        | Field::Optional {
            reported: false, ..
        } => Aim::None,
        Field::Window(id) => Aim::Window(id),
        Field::Optional { id: Some(id), .. } => Aim::Window(*id),
        Field::Optional { id: None, .. } => Aim::Focused,
    }
}

/// Names `focused`, the window focused at the check, in an action that may name a window
/// and names none, so focus moving before niri handles it can't redirect it. With no
/// window focused, such an action is refused.
fn pin(action: &mut Action, focused: Option<u64>) -> Result<(), CallError> {
    let Field::Optional { id: id @ None, .. } = field(action) else {
        return Ok(());
    };
    *id = Some(focused.ok_or_else(|| {
        CallError::InvalidArguments(
            "no window has focus; pass the id of the window to act on".to_owned(),
        )
    })?);
    Ok(())
}

/// Sends `action`, then, for an action about one window, reports that window as niri
/// shows it once it changed, or after a second: `changed`, `unchanged` or `closed`, with
/// the window's state. Any other action is `sent`. An action that may name a window and
/// names none goes out naming the window focused at the check, so focus moving meanwhile
/// can't redirect it. `CloseWindow` on a window of an app on the deny list is refused.
pub(crate) async fn run(
    niri: Niri<'_>,
    policy: &Loaded,
    mut action: Action,
) -> Result<Outcome, CallError> {
    let mut waiter = niri::waiter(niri.events).await?;
    let focused = waiter.view().focused_window();
    let target = match aim(&mut action) {
        Aim::None => None,
        Aim::Focused => focused,
        Aim::Window(id) if waiter.view().windows().contains_key(&id) => Some(id),
        Aim::Window(id) => return Err(no_window(id)),
    };
    pin(&mut action, focused)?;
    if let Action::CloseWindow { id: Some(id) } = action {
        refuse_close(policy, waiter.view(), id)?;
    }
    let before = target.and_then(|id| state(waiter.view(), id));
    if let Some(lost) = send(niri, action).await? {
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
    fn only_the_spawns_skip_the_quiet_whatever_niri_reports() {
        let spawn = Action::Spawn {
            command: vec!["foot".to_owned()],
        };
        let shell = Action::SpawnSh {
            command: "foot".to_owned(),
        };
        assert_eq!(quiet(Some(&spawn)), Quiet::Never);
        assert_eq!(quiet(Some(&shell)), Quiet::Never);
        let window = Action::FullscreenWindow { id: Some(2) };
        let workspace = Action::FocusWorkspaceDown {};
        assert_eq!(quiet(Some(&window)), Quiet::UnlessSeen);
        assert_eq!(quiet(Some(&workspace)), Quiet::UnlessSeen);
    }

    #[test]
    fn an_action_that_names_no_window_is_pinned_to_the_focused_one() {
        let json = |action: &Action| serde_json::to_value(action).unwrap();
        let pinned =
            |mut action: Action, focused| pin(&mut action, focused).map(|()| json(&action));
        assert_eq!(
            pinned(Action::FullscreenWindow { id: None }, Some(7)),
            Ok(json(&Action::FullscreenWindow { id: Some(7) }))
        );
        // Screenshots change nothing niri reports, but act on a window all the same.
        let screenshot = |id| Action::ScreenshotWindow {
            id,
            write_to_disk: false,
            show_pointer: false,
            path: None,
        };
        assert_eq!(
            pinned(screenshot(None), Some(7)),
            Ok(json(&screenshot(Some(7))))
        );
        assert_eq!(
            pinned(Action::CenterWindow { id: Some(3) }, Some(7)),
            Ok(json(&Action::CenterWindow { id: Some(3) }))
        );
        assert_eq!(
            pinned(Action::MaximizeColumn {}, None),
            Ok(json(&Action::MaximizeColumn {}))
        );
        assert!(matches!(
            pinned(Action::FullscreenWindow { id: None }, None),
            Err(CallError::InvalidArguments(_))
        ));
    }

    #[test]
    fn an_action_is_about_the_window_it_names_or_the_focused_one() {
        assert_eq!(
            aim(&mut Action::FullscreenWindow { id: Some(12) }),
            Aim::Window(12)
        );
        assert_eq!(
            aim(&mut Action::SetWindowWidth {
                id: None,
                change: SizeChange::SetFixed(1600)
            }),
            Aim::Focused
        );
        assert_eq!(
            aim(&mut Action::MoveWindowToWorkspace {
                window_id: Some(4),
                reference: WorkspaceReferenceArg::Index(2),
                focus: false
            }),
            Aim::Window(4)
        );
        assert_eq!(aim(&mut Action::FocusWindow { id: 3 }), Aim::Window(3));
        assert_eq!(aim(&mut Action::MaximizeColumn {}), Aim::None);
        assert_eq!(
            aim(&mut Action::ScreenshotWindow {
                id: Some(3),
                write_to_disk: false,
                show_pointer: false,
                path: None
            }),
            Aim::None
        );
    }
}
