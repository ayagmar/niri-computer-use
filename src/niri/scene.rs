//! What niri shows on each output (pure), so the event stream can stamp the last time the
//! windows there moved, and a pointer tool can tell that a screenshot's pixels may no
//! longer land on what its image shows. niri reports no place for a tiled window, only its
//! column, size and whether its workspace's view scrolls to it, as the active window; a
//! change in any of them can move what is under a pixel.

use std::collections::{BTreeMap, BTreeSet};

use niri_ipc::WindowLayout;
use niri_ipc::state::EventStreamState;
use tokio::time::Instant;

/// One output as niri describes it: whether the overview is open, the workspace it shows
/// and that workspace's active window, and each window on that workspace.
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

/// The last time the windows on an output moved, as far as niri reported, and how.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Shifted {
    pub(crate) at: Instant,
    pub(crate) what: String,
}

/// When the windows on an output last moved (`EventStream::shifted`), and which connection
/// of the event stream says so: a replica of another connection than a screenshot's knows
/// nothing of what moved before it.
#[derive(Debug)]
pub(crate) struct Moves {
    pub(crate) connection: u64,
    pub(crate) shifted: Option<Shifted>,
}

/// Each output's scene in `state`, by output name.
pub(crate) fn scenes(state: &EventStreamState) -> BTreeMap<String, Scene> {
    let outputs: BTreeSet<&str> = state
        .workspaces
        .workspaces
        .values()
        .filter_map(|workspace| workspace.output.as_deref())
        .collect();
    outputs
        .into_iter()
        .map(|output| (output.to_owned(), Scene::of(state, output)))
        .collect()
}

/// How each output's scene differs from `before` to `after`, by output name.
pub(crate) fn moved(
    before: &BTreeMap<String, Scene>,
    after: &BTreeMap<String, Scene>,
) -> Vec<(String, String)> {
    let outputs: BTreeSet<&String> = before.keys().chain(after.keys()).collect();
    let none = Scene::default();
    outputs
        .into_iter()
        .filter_map(|output| {
            let was = before.get(output).unwrap_or(&none);
            let is = after.get(output).unwrap_or(&none);
            Some((output.clone(), was.moved_in(is)?))
        })
        .collect()
}

impl Scene {
    /// What `state` has on `output`.
    fn of(state: &EventStreamState, output: &str) -> Self {
        let shown =
            state.workspaces.workspaces.values().find(|workspace| {
                workspace.is_active && workspace.output.as_deref() == Some(output)
            });
        let workspace = shown.map(|workspace| workspace.id);
        let windows = state
            .windows
            .windows
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
            overview: state.overview.is_open,
            workspace,
            active: shown.and_then(|shown| shown.active_window_id),
            windows,
        }
    }

    /// What differs in `now`, if anything, the first of: the overview, the workspace shown,
    /// a window that left the workspace or came onto it, one niri moved or resized, and
    /// the active window.
    fn moved_in(&self, now: &Self) -> Option<String> {
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
    use niri_ipc::state::EventStreamStatePart as _;
    use niri_ipc::{Event, Window, Workspace};

    use super::*;
    use crate::niri::waiter::tests::{window as window_on, workspace as on_dp1};

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

    /// How DP-1's scene differs after `event`.
    fn after(event: Event) -> Option<String> {
        let mut state = EventStreamState::default();
        for initial in desk() {
            state.apply(initial);
        }
        let before = scenes(&state);
        state.apply(event);
        let mut moved = moved(&before, &scenes(&state));
        let index = moved.iter().position(|(output, _)| output == "DP-1")?;
        Some(moved.swap_remove(index).1)
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
