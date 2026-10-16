//! What niri showed on a screenshot's output when the capture started (pure), so a pointer
//! tool can tell that the windows there moved since and the image no longer shows what a
//! pixel lands on. niri reports no place for a tiled window, only its column, size and
//! whether its workspace's view scrolls to it, as the active window; a change in any of
//! them can move what is under a pixel.

use std::collections::BTreeMap;

use niri_ipc::WindowLayout;

use crate::niri::waiter::View;

/// One output as niri described it: whether the overview was open, the workspace it
/// showed and that workspace's active window, and each window on that workspace.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Scene {
    overview: bool,
    workspace: Option<u64>,
    active: Option<u64>,
    windows: BTreeMap<u64, Placed>,
}

/// Where niri put one window.
#[derive(Debug, Clone, PartialEq)]
struct Placed {
    layout: WindowLayout,
    floating: bool,
}

impl Scene {
    /// What `view` has on `output`.
    pub(crate) fn of(view: &View, output: &str) -> Self {
        let shown = view
            .workspaces()
            .values()
            .find(|workspace| workspace.is_active && workspace.output.as_deref() == Some(output));
        let workspace = shown.map(|workspace| workspace.id);
        let windows = view
            .windows()
            .values()
            .filter(|window| workspace.is_some() && window.workspace_id == workspace)
            .map(|window| {
                let placed = Placed {
                    layout: window.layout.clone(),
                    floating: window.is_floating,
                };
                (window.id, placed)
            })
            .collect();
        Self {
            overview: view.overview_open(),
            workspace,
            active: shown.and_then(|shown| shown.active_window_id),
            windows,
        }
    }

    /// What differs in `now`, if anything, the first of: the overview, the workspace shown,
    /// a window that left the workspace or came onto it, one niri moved or resized, and
    /// the active window.
    pub(crate) fn moved_in(&self, now: &Self) -> Option<String> {
        if self.overview != now.overview {
            let what = if now.overview { "opened" } else { "closed" };
            return Some(format!("niri {what} the overview"));
        }
        if self.workspace != now.workspace {
            return Some(format!(
                "the output shows {} instead of {}",
                workspace(now.workspace),
                workspace(self.workspace)
            ));
        }
        if let Some(id) = self.windows.keys().find(|id| !now.windows.contains_key(id)) {
            return Some(format!("window {id} closed or left the workspace"));
        }
        if let Some(id) = now.windows.keys().find(|id| !self.windows.contains_key(id)) {
            return Some(format!("window {id} opened or came onto the workspace"));
        }
        if let Some((id, _)) = self
            .windows
            .iter()
            .find(|(id, placed)| now.windows.get(id) != Some(placed))
        {
            return Some(format!("niri moved or resized window {id}"));
        }
        (self.active != now.active).then(|| {
            format!(
                "the view now follows {} instead of {}, as after a focus change",
                window(now.active),
                window(self.active)
            )
        })
    }
}

fn workspace(id: Option<u64>) -> String {
    id.map_or_else(|| "no workspace".to_owned(), |id| format!("workspace {id}"))
}

fn window(id: Option<u64>) -> String {
    id.map_or_else(|| "no window".to_owned(), |id| format!("window {id}"))
}

#[cfg(test)]
mod tests {
    use niri_ipc::{Event, Window, Workspace};

    use super::*;
    use crate::niri::waiter::tests::{view_of, window as window_on, workspace as on_dp1};

    fn on(output: &str, id: u64, active: Option<u64>) -> Workspace {
        let mut workspace = on_dp1(id, true);
        workspace.output = Some(output.to_owned());
        workspace.active_window_id = active;
        workspace
    }

    fn tiled(id: u64, workspace: u64, column: usize, width: f64) -> Window {
        let mut window = window_on(id, Some("app"), workspace, false);
        window.layout.pos_in_scrolling_layout = Some((column, 1));
        window.layout.tile_size = (width, 1374.0);
        window
    }

    /// DP-1 shows workspace 1 with windows 7 and 9 side by side, 7 active, and has an
    /// empty workspace 3; HDMI-A-1 shows workspace 2 with window 4.
    fn desk() -> Vec<Event> {
        vec![
            Event::WorkspacesChanged {
                workspaces: vec![
                    on("DP-1", 1, Some(7)),
                    on("HDMI-A-1", 2, Some(4)),
                    on_dp1(3, false),
                ],
            },
            Event::WindowsChanged {
                windows: vec![
                    tiled(7, 1, 2, 1265.0),
                    tiled(9, 1, 3, 1265.0),
                    tiled(4, 2, 1, 1900.0),
                ],
            },
            Event::OverviewOpenedOrClosed { is_open: false },
        ]
    }

    /// What DP-1 shows after `event`, compared with before.
    fn after(event: Event) -> Option<String> {
        let before = Scene::of(&view_of(desk()), "DP-1");
        let now = Scene::of(&view_of([desk(), vec![event]].concat()), "DP-1");
        before.moved_in(&now)
    }

    #[test]
    fn a_window_resized_or_moved_on_the_output_moves_its_scene() {
        // The real session's first misclick: the browser went from the whole output to
        // half of it between the screenshot and the click.
        let resized = Event::WindowLayoutsChanged {
            changes: vec![(7, tiled(7, 1, 2, 2540.0).layout)],
        };
        assert_eq!(after(resized).unwrap(), "niri moved or resized window 7");
        let swapped = Event::WindowLayoutsChanged {
            changes: vec![(9, tiled(9, 1, 1, 1265.0).layout)],
        };
        assert_eq!(after(swapped).unwrap(), "niri moved or resized window 9");
    }

    #[test]
    fn focus_moving_to_another_window_there_moves_its_scene() {
        // The second: the user focused their terminal beside the browser.
        let focused = Event::WorkspaceActiveWindowChanged {
            workspace_id: 1,
            active_window_id: Some(9),
        };
        assert_eq!(
            after(focused).unwrap(),
            "the view now follows window 9 instead of window 7, as after a focus change"
        );
    }

    #[test]
    fn windows_coming_and_going_the_workspace_and_the_overview_move_it() {
        let mut opened = tiled(12, 1, 4, 800.0);
        opened.is_focused = true;
        let cases = [
            (
                Event::WindowOpenedOrChanged { window: opened },
                "window 12 opened or came onto the workspace",
            ),
            (
                Event::WindowClosed { id: 9 },
                "window 9 closed or left the workspace",
            ),
            (
                Event::WorkspaceActivated {
                    id: 3,
                    focused: true,
                },
                "the output shows workspace 3 instead of workspace 1",
            ),
            (
                Event::OverviewOpenedOrClosed { is_open: true },
                "niri opened the overview",
            ),
        ];
        for (event, expected) in cases {
            assert_eq!(after(event).as_deref(), Some(expected));
        }
    }

    #[test]
    fn what_doesnt_move_the_output_leaves_it() {
        let mut retitled = tiled(7, 1, 2, 1265.0);
        retitled.title = Some("Loaded - Helium".to_owned());
        let elsewhere = Event::WindowLayoutsChanged {
            changes: vec![(4, tiled(4, 2, 1, 900.0).layout)],
        };
        let other_output = Event::WorkspaceActiveWindowChanged {
            workspace_id: 2,
            active_window_id: None,
        };
        let focus_away = Event::WindowFocusChanged { id: Some(4) };
        for event in [
            Event::WindowOpenedOrChanged { window: retitled },
            elsewhere,
            other_output,
            focus_away,
            Event::WindowUrgencyChanged {
                id: 9,
                urgent: true,
            },
        ] {
            assert_eq!(after(event.clone()), None, "{event:?}");
        }
    }
}
