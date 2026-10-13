//! What an action changed on the desktop, from niri's state before it and after it, so the
//! agent can tell whether it did something without a screenshot (pure). `elements` diffs
//! one window's accessible elements the same way.

pub(crate) mod elements;

use std::collections::BTreeMap;

use niri_ipc::Window;
use serde::Serialize;

use crate::niri::waiter::View;

/// At most this many window entries, across `opened`, `closed` and `windows`.
const WINDOWS: usize = 12;
/// Titles and names are cut to this many characters.
const TEXT: usize = 100;

/// What niri reported at one moment: the windows by id and the focused workspace.
#[derive(Debug, Clone)]
pub(crate) struct Scene {
    windows: BTreeMap<u64, Window>,
    focused_workspace: Option<u64>,
}

impl Scene {
    pub(crate) fn of(view: &View) -> Self {
        Self {
            windows: view
                .windows()
                .iter()
                .map(|(id, window)| (*id, window.clone()))
                .collect(),
            focused_workspace: view
                .workspaces()
                .values()
                .find(|workspace| workspace.is_focused)
                .map(|workspace| workspace.id),
        }
    }

    pub(crate) fn has(&self, id: u64) -> bool {
        self.windows.contains_key(&id)
    }

    fn focused_window(&self) -> Option<u64> {
        self.windows
            .values()
            .find(|window| window.is_focused)
            .map(|window| window.id)
    }
}

/// What changed between two scenes. Each field is left out when nothing in it changed.
#[derive(Debug, Default, PartialEq, Serialize)]
pub(crate) struct Changes {
    /// Windows open after but not before, and those that opened and closed in between.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) opened: Vec<Opened>,
    /// Windows open before but not after, and those that opened and closed in between.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) closed: Vec<Closed>,
    /// The window with keyboard focus before and after, when they differ.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) focus: Option<Moved>,
    /// The focused workspace before and after, when they differ.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) workspace: Option<Moved>,
    /// Windows open before and after whose title, app id, workspace, floating, size,
    /// floating position or urgency changed, with the new values.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) windows: Vec<Changed>,
    /// Window entries left out past the cap.
    #[serde(skip_serializing_if = "is_zero")]
    pub(crate) truncated: usize,
    /// Why the report is incomplete: niri's event stream ended before the report did, and
    /// what is here was seen before that, or it couldn't be watched at all, and only
    /// `elements`, from walks of their own, can be here.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) lost: Option<String>,
    /// With `element_changes`: what changed among the window's accessible elements.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) elements: Option<elements::Report>,
}

impl Changes {
    /// Whether there is nothing to report.
    pub(crate) fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde's skip_serializing_if passes the field by reference"
)]
const fn is_zero(count: &usize) -> bool {
    *count == 0
}

#[derive(Debug, PartialEq, Serialize)]
pub(crate) struct Opened {
    pub(crate) id: u64,
    pub(crate) app_id: Option<String>,
    pub(crate) workspace_id: Option<u64>,
    pub(crate) title: Option<String>,
}

#[derive(Debug, PartialEq, Serialize)]
pub(crate) struct Closed {
    pub(crate) id: u64,
    pub(crate) app_id: Option<String>,
}

/// An id before and after; null is none.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Moved {
    pub(crate) from: Option<u64>,
    pub(crate) to: Option<u64>,
}

/// A window's fields that changed, each with its new value. A title or app id that went
/// away is empty.
#[derive(Debug, Default, PartialEq, Serialize)]
pub(crate) struct Changed {
    pub(crate) id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) app_id: Option<String>,
    /// Left out when the window left every workspace, which a mapped window doesn't.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) workspace_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) floating: Option<bool>,
    /// niri's `window_size`. niri 26.04 has no fullscreen flag, so fullscreen shows here as
    /// the output's size.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) size: Option<(i32, i32)>,
    /// A window floating before and after that moved: niri's `tile_pos_in_workspace_view`.
    /// A tiled window's place isn't reported, since opening a column shifts the ones to its
    /// right.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) position: Option<(f64, f64)>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) urgent: Option<bool>,
}

/// What changed from `before` to `after`. `passing` holds the windows seen at any moment
/// in between, as last seen, so one that opened and closed again is listed as both.
pub(crate) fn desktop(before: &Scene, after: &Scene, passing: &BTreeMap<u64, Window>) -> Changes {
    let flashed: Vec<&Window> = passing
        .values()
        .filter(|window| !before.has(window.id) && !after.has(window.id))
        .collect();
    let new = after
        .windows
        .values()
        .filter(|window| !before.has(window.id));
    let mut opened: Vec<&Window> = new.chain(flashed.iter().copied()).collect();
    opened.sort_by_key(|window| window.id);
    let gone = before
        .windows
        .values()
        .filter(|window| !after.has(window.id));
    let mut closed: Vec<&Window> = gone.chain(flashed).collect();
    closed.sort_by_key(|window| window.id);
    let changed = after
        .windows
        .values()
        .filter_map(|window| changed(before.windows.get(&window.id)?, window));
    let mut entries = Entries::default();
    entries.opened = opened
        .into_iter()
        .filter_map(|window| entries.keep(opened_entry(window)))
        .collect();
    entries.closed = closed
        .into_iter()
        .filter_map(|window| entries.keep(closed_entry(window)))
        .collect();
    entries.windows = changed.filter_map(|change| entries.keep(change)).collect();
    Changes {
        opened: entries.opened,
        closed: entries.closed,
        focus: moved(before.focused_window(), after.focused_window()),
        workspace: moved(before.focused_workspace, after.focused_workspace),
        windows: entries.windows,
        truncated: entries.dropped,
        lost: None,
        elements: None,
    }
}

/// The window entries kept so far under the cap, and how many were dropped.
#[derive(Default)]
struct Entries {
    opened: Vec<Opened>,
    closed: Vec<Closed>,
    windows: Vec<Changed>,
    kept: usize,
    dropped: usize,
}

impl Entries {
    fn keep<T>(&mut self, entry: T) -> Option<T> {
        if self.kept == WINDOWS {
            self.dropped += 1;
            return None;
        }
        self.kept += 1;
        Some(entry)
    }
}

fn opened_entry(window: &Window) -> Opened {
    Opened {
        id: window.id,
        app_id: window.app_id.clone(),
        workspace_id: window.workspace_id,
        title: window.title.as_deref().map(cut),
    }
}

fn closed_entry(window: &Window) -> Closed {
    Closed {
        id: window.id,
        app_id: window.app_id.clone(),
    }
}

/// The fields of window `after` that differ from `before`, or none.
fn changed(before: &Window, after: &Window) -> Option<Changed> {
    let text = |text: &Option<String>| text.as_deref().map(cut).unwrap_or_default();
    let change = Changed {
        id: after.id,
        title: (before.title != after.title).then(|| text(&after.title)),
        app_id: (before.app_id != after.app_id).then(|| text(&after.app_id)),
        workspace_id: after
            .workspace_id
            .filter(|_| before.workspace_id != after.workspace_id),
        floating: (before.is_floating != after.is_floating).then_some(after.is_floating),
        size: (before.layout.window_size != after.layout.window_size)
            .then_some(after.layout.window_size),
        position: floated(before, after),
        urgent: (before.is_urgent != after.is_urgent).then_some(after.is_urgent),
    };
    let unchanged = Changed {
        id: after.id,
        ..Changed::default()
    };
    (change != unchanged).then_some(change)
}

/// Where window `after` is now, when it floated before and after and moved.
fn floated(before: &Window, after: &Window) -> Option<(f64, f64)> {
    let place = |window: &Window| {
        window
            .layout
            .tile_pos_in_workspace_view
            .filter(|_| window.is_floating)
    };
    place(after).filter(|now| place(before).is_some_and(|then| then != *now))
}

fn moved(from: Option<u64>, to: Option<u64>) -> Option<Moved> {
    (from != to).then_some(Moved { from, to })
}

/// `text` cut to its first hundred characters, with `…` when anything was cut.
pub(crate) fn cut(text: &str) -> String {
    match text.char_indices().nth(TEXT) {
        Some((end, _)) => format!("{}…", text.get(..end).unwrap_or(text)),
        None => text.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::niri::waiter::tests::{view_on, window, workspace};

    fn scene(focused_workspace: u64, windows: Vec<Window>) -> Scene {
        let workspaces = vec![
            workspace(1, focused_workspace == 1),
            workspace(2, focused_workspace == 2),
        ];
        Scene::of(&view_on(workspaces, windows))
    }

    fn titled(id: u64, title: &str, focused: bool) -> Window {
        Window {
            title: Some(title.to_owned()),
            ..window(id, Some("app"), 1, focused)
        }
    }

    #[test]
    fn a_dialog_that_opens_and_takes_focus_is_opened_and_a_focus_change() {
        let before = scene(1, vec![titled(1, "Main", true)]);
        let after = scene(1, vec![titled(1, "Main", false), titled(4, "Dialog", true)]);
        let passing = BTreeMap::from([(4, titled(4, "Dialog", true))]);
        let changes = desktop(&before, &after, &passing);
        assert_eq!(
            serde_json::to_value(&changes).unwrap(),
            json!({
                "opened": [{"id": 4, "app_id": "app", "workspace_id": 1, "title": "Dialog"}],
                "focus": {"from": 1, "to": 4},
            })
        );
    }

    #[test]
    fn nothing_changed_is_empty_even_with_windows_seen_in_between_that_were_open_before() {
        let before = scene(1, vec![titled(1, "Main", true)]);
        let passing = BTreeMap::from([(1, titled(1, "Main", true))]);
        assert!(desktop(&before, &before.clone(), &passing).is_empty());
    }

    #[test]
    fn a_window_that_opened_and_closed_in_between_is_both_and_a_closed_one_keeps_its_app() {
        let before = scene(
            1,
            vec![titled(1, "Main", true), window(2, Some("gone"), 1, false)],
        );
        let after = scene(2, vec![titled(1, "Main", false)]);
        let passing = BTreeMap::from([(7, window(7, Some("flash"), 2, true))]);
        let changes = desktop(&before, &after, &passing);
        assert_eq!(
            serde_json::to_value(&changes).unwrap(),
            json!({
                "opened": [{"id": 7, "app_id": "flash", "workspace_id": 2, "title": "t"}],
                "closed": [{"id": 2, "app_id": "gone"}, {"id": 7, "app_id": "flash"}],
                "focus": {"from": 1, "to": null},
                "workspace": {"from": 1, "to": 2},
            })
        );
    }

    #[test]
    fn a_window_that_changed_reports_only_its_changed_fields_with_their_new_values() {
        let before = scene(1, vec![titled(1, "Doc", true), titled(2, "Other", false)]);
        let mut doc = titled(1, "Doc *", true);
        doc.is_floating = true;
        doc.is_urgent = true;
        doc.layout.window_size = (800, 600);
        doc.workspace_id = Some(2);
        let after = scene(1, vec![doc, titled(2, "Other", false)]);
        let changes = desktop(&before, &after, &BTreeMap::new());
        assert_eq!(
            serde_json::to_value(&changes).unwrap(),
            json!({"windows": [{
                "id": 1, "title": "Doc *", "workspace_id": 2, "floating": true,
                "size": [800, 600], "urgent": true
            }]})
        );
        // A focus change alone is `focus`, not a window entry; layout positions aren't
        // reported, since opening a column shifts the ones to its right.
        let mut moved = titled(2, "Other", true);
        moved.layout.pos_in_scrolling_layout = Some((3, 1));
        let refocused = scene(1, vec![titled(1, "Doc", false), moved]);
        let focus_only = desktop(&before, &refocused, &BTreeMap::new());
        assert_eq!(
            serde_json::to_value(&focus_only).unwrap(),
            json!({"focus": {"from": 1, "to": 2}})
        );
    }

    #[test]
    fn a_floating_window_that_moved_says_where_and_one_floated_says_only_that() {
        let floating = |id, place| {
            let mut window = titled(id, "Tool", false);
            window.is_floating = true;
            window.layout.pos_in_scrolling_layout = None;
            window.layout.tile_pos_in_workspace_view = Some(place);
            window
        };
        let tiled = titled(2, "Doc", true);
        let before = scene(1, vec![floating(1, (200.0, 50.0)), tiled.clone()]);
        let mut floated = tiled;
        floated.is_floating = true;
        floated.layout.tile_pos_in_workspace_view = Some((10.0, 10.0));
        let after = scene(1, vec![floating(1, (100.0, 50.0)), floated]);
        assert_eq!(
            serde_json::to_value(desktop(&before, &after, &BTreeMap::new())).unwrap(),
            json!({"windows": [
                {"id": 1, "position": [100.0, 50.0]},
                {"id": 2, "floating": true},
            ]})
        );
    }

    #[test]
    fn window_entries_stop_at_the_cap_and_count_the_rest() {
        let before = scene(
            1,
            (1..=10).map(|id| window(id, Some("a"), 1, false)).collect(),
        );
        let after = scene(
            1,
            (6..=20).map(|id| window(id, Some("a"), 1, false)).collect(),
        );
        let passing = (11..=20)
            .map(|id| (id, window(id, Some("a"), 1, false)))
            .collect();
        let changes = desktop(&before, &after, &passing);
        assert_eq!(changes.opened.len(), 10);
        assert_eq!(
            changes.closed.iter().map(|c| c.id).collect::<Vec<_>>(),
            [1, 2]
        );
        assert_eq!(changes.truncated, 3);
    }

    #[test]
    fn long_titles_are_cut_at_a_hundred_characters() {
        let long = "é".repeat(150);
        assert_eq!(cut(&long), format!("{}…", "é".repeat(100)));
        assert_eq!(cut(&"a".repeat(100)), "a".repeat(100));
    }
}
