//! What the accessibility tree says, and where that is in niri's layout (pure). A
//! toolkit reports an element's extents relative to its window (AT-SPI's WINDOW
//! coordinates); on Wayland its screen coordinates are meaningless. niri knows where the
//! window is, so an element's layout box is the window's place in the layout plus those
//! extents. That holds only when the toolkit's window origin is niri's window geometry,
//! which the frame guard checks: the frame's own WINDOW size must equal niri's
//! `window_size`. GTK 3 measures from its surface's shadow and Qt from inside its own
//! decorations, and both fail the guard with client-side decorations.

use std::collections::BTreeMap;
use std::hash::{DefaultHasher, Hash, Hasher};

use niri_ipc::{LogicalOutput, Output, Window, WindowLayout, Workspace};
use serde::Serialize;

use crate::coords::LayoutPt;

/// AT-SPI's roles, by number, in at-spi2-core 2.60's `AtspiRole` order, named in snake case.
const ROLES: [&str; 131] = [
    "invalid",
    "accelerator_label",
    "alert",
    "animation",
    "arrow",
    "calendar",
    "canvas",
    "check_box",
    "check_menu_item",
    "color_chooser",
    "column_header",
    "combo_box",
    "date_editor",
    "desktop_icon",
    "desktop_frame",
    "dial",
    "dialog",
    "directory_pane",
    "drawing_area",
    "file_chooser",
    "filler",
    "focus_traversable",
    "font_chooser",
    "frame",
    "glass_pane",
    "html_container",
    "icon",
    "image",
    "internal_frame",
    "label",
    "layered_pane",
    "list",
    "list_item",
    "menu",
    "menu_bar",
    "menu_item",
    "option_pane",
    "page_tab",
    "page_tab_list",
    "panel",
    "password_text",
    "popup_menu",
    "progress_bar",
    "button",
    "radio_button",
    "radio_menu_item",
    "root_pane",
    "row_header",
    "scroll_bar",
    "scroll_pane",
    "separator",
    "slider",
    "spin_button",
    "split_pane",
    "status_bar",
    "table",
    "table_cell",
    "table_column_header",
    "table_row_header",
    "tearoff_menu_item",
    "terminal",
    "text",
    "toggle_button",
    "tool_bar",
    "tool_tip",
    "tree",
    "tree_table",
    "unknown",
    "viewport",
    "window",
    "extended",
    "header",
    "footer",
    "paragraph",
    "ruler",
    "application",
    "autocomplete",
    "editbar",
    "embedded",
    "entry",
    "chart",
    "caption",
    "document_frame",
    "heading",
    "page",
    "section",
    "redundant_object",
    "form",
    "link",
    "input_method_window",
    "table_row",
    "tree_item",
    "document_spreadsheet",
    "document_presentation",
    "document_text",
    "document_web",
    "document_email",
    "comment",
    "list_box",
    "grouping",
    "image_map",
    "notification",
    "info_bar",
    "level_bar",
    "title_bar",
    "block_quote",
    "audio",
    "video",
    "definition",
    "article",
    "landmark",
    "log",
    "marquee",
    "math",
    "rating",
    "timer",
    "static",
    "math_fraction",
    "math_root",
    "subscript",
    "superscript",
    "description_list",
    "description_term",
    "description_value",
    "footnote",
    "content_deletion",
    "content_insertion",
    "mark",
    "suggestion",
    "push_button_menu",
    "switch",
];
/// AT-SPI's states, by bit, in `AtspiStateType` order, named in snake case.
const STATES: [&str; 44] = [
    "invalid",
    "active",
    "armed",
    "busy",
    "checked",
    "collapsed",
    "defunct",
    "editable",
    "enabled",
    "expandable",
    "expanded",
    "focusable",
    "focused",
    "has_tooltip",
    "horizontal",
    "iconified",
    "modal",
    "multi_line",
    "multiselectable",
    "opaque",
    "pressed",
    "resizable",
    "selectable",
    "selected",
    "sensitive",
    "showing",
    "single_line",
    "stale",
    "transient",
    "vertical",
    "visible",
    "manages_descendants",
    "indeterminate",
    "required",
    "truncated",
    "animated",
    "invalid_entry",
    "supports_autocompletion",
    "selectable_text",
    "is_default",
    "visited",
    "checkable",
    "has_popup",
    "read_only",
];

/// A role's name, or `unknown` for a number newer than this table.
pub(crate) fn role_name(role: u32) -> &'static str {
    usize::try_from(role)
        .ok()
        .and_then(|index| ROLES.get(index))
        .copied()
        .unwrap_or("unknown")
}

/// Whether `name` is a role this table knows, for checking a `role` argument.
pub(crate) fn is_role(name: &str) -> bool {
    ROLES.contains(&name)
}

/// An AT-SPI state, by its bit in the state set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum State {
    Editable = 7,
    Showing = 25,
    Visible = 30,
}

/// A state set as `Accessible.GetState` returns it: two 32-bit words, low bits first.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct States(u64);

impl States {
    pub(crate) fn from_words(words: &[u32]) -> Self {
        let word = |index: usize| u64::from(words.get(index).copied().unwrap_or(0));
        Self(word(0) | (word(1) << 32))
    }

    pub(crate) const fn has(self, state: State) -> bool {
        self.0 & (1 << state as u64) != 0
    }

    /// The names of the states set, in bit order. Bits past the table are left out.
    pub(crate) fn names(self) -> Vec<&'static str> {
        STATES
            .iter()
            .enumerate()
            .filter(|(bit, _)| self.0 & (1 << bit) != 0)
            .map(|(_, name)| *name)
            .collect()
    }

    /// The names of the states set now but not `before`, then of those cleared since.
    pub(crate) fn changes(self, before: Self) -> (Vec<&'static str>, Vec<&'static str>) {
        (
            Self(self.0 & !before.0).names(),
            Self(before.0 & !self.0).names(),
        )
    }
}

/// A rectangle as `Component.GetExtents` returns it, in logical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub(crate) struct Extents {
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) width: i32,
    pub(crate) height: i32,
}

/// A rectangle in niri's layout space, in logical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub(crate) struct LayoutBox {
    pub(crate) x: f64,
    pub(crate) y: f64,
    pub(crate) width: f64,
    pub(crate) height: f64,
}

impl LayoutBox {
    pub(crate) const fn centre(self) -> LayoutPt {
        LayoutPt {
            x: self.width.mul_add(0.5, self.x),
            y: self.height.mul_add(0.5, self.y),
        }
    }
}

/// Why an element has no layout box.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Unmappable {
    /// The window's accessible frame isn't the size niri gives the window, so the
    /// toolkit's window coordinates start elsewhere.
    FrameSizeMismatch,
    /// The element isn't showing and visible, its floating window's workspace isn't the
    /// one shown on its output, or niri's overview is open.
    NotShowing,
    /// A fullscreen window covers the element's floating window: niri hides a workspace's
    /// floating windows while its active window is a fullscreen tiled one.
    #[serde(rename = "not_showing")]
    Covered,
    /// niri switched a workspace, opened or closed the overview, or moved, resized,
    /// floated or opened the element's window a moment ago, or changed what is shown since
    /// the element was placed, so the view may still be moving. Only an aim gives it.
    #[serde(rename = "not_showing")]
    Moving,
    /// niri moved or resized the element's window again within the settle interval of
    /// the time before, so it may keep moving and waiting may not help. Only an aim gives
    /// it.
    #[serde(rename = "not_showing")]
    Restless,
    /// The window is tiled: niri 26.04 gives only floating windows a place in their
    /// workspace's view, so where a tiled one is on screen isn't known.
    Tiled,
    /// The element's centre is off its window's output: niri lets a floating window
    /// reach past its output's edge, and shows that part nowhere.
    #[serde(rename = "not_showing")]
    OffOutput,
    /// The element has no area.
    Empty,
    /// The element's centre is outside the screenshot the pointer aims through.
    OutsideScreenshot,
    /// The screenshot the pointer aims through is of another output than the element's
    /// window. Only an aim gives it.
    #[serde(rename = "outside_screenshot")]
    OtherOutput,
}

/// Where a placed window's geometry starts in the layout, and the output it is shown on.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Origin {
    pub(crate) at: LayoutPt,
    /// The output's name.
    pub(crate) output: String,
    /// The output's place and size in the layout.
    pub(crate) area: LogicalOutput,
}

/// What niri reports for the call about where windows are shown: its workspaces, its
/// outputs, its other windows, and whether the overview is open.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Scene<'a> {
    pub(crate) workspaces: &'a [Workspace],
    pub(crate) outputs: &'a BTreeMap<String, Output>,
    pub(crate) windows: &'a [Window],
    pub(crate) overview_open: bool,
}

/// Where `window`'s geometry starts in the layout and the output it is shown on, from the
/// `scene` niri reports, or why that isn't known. A tile in the scrolling layout is never placed (`Tiled`), wherever its
/// workspace is, even on no output: niri 26.04 gives it no place, and a place derived from
/// where the view is heading would be wrong while the view scrolls. A floating window is
/// placed only on the active workspace of its output (`NotShowing` otherwise, on no output
/// too): niri reports its place on every workspace, shown or not, and the shown
/// workspace's windows are where a hidden one's would be. Nor while the overview is open,
/// which zooms the workspaces out and leaves that place as it was, nor while a fullscreen
/// window covers it (`Covered`).
pub(crate) fn origin(window: &Window, scene: &Scene<'_>) -> Result<Origin, Unmappable> {
    if window.layout.pos_in_scrolling_layout.is_some() {
        return Err(Unmappable::Tiled);
    }
    if scene.overview_open {
        return Err(Unmappable::NotShowing);
    }
    let workspace = window
        .workspace_id
        .and_then(|id| scene.workspaces.iter().find(|workspace| workspace.id == id))
        .filter(|workspace| workspace.is_active)
        .ok_or(Unmappable::NotShowing)?;
    let (name, area) = workspace
        .output
        .as_ref()
        .and_then(|name| Some((name, scene.outputs.get(name)?.logical?)))
        .ok_or(Unmappable::NotShowing)?;
    if covered(workspace, &area, scene.windows) {
        return Err(Unmappable::Covered);
    }
    Ok(Origin {
        at: window_origin(&area, &window.layout)?,
        output: name.clone(),
        area,
    })
}

/// Whether `workspace`'s active window is a tiled one as large as `output`, as a fullscreen
/// window is. niri then draws none of the workspace's floating windows and sends the
/// pointer to the fullscreen one (`Workspace::is_floating_visible`). IPC doesn't say
/// whether a window is fullscreen, so a tiled window that fills the output some other way
/// counts too, and so does one whose workspace niri shows its floating windows above.
fn covered(workspace: &Workspace, output: &LogicalOutput, windows: &[Window]) -> bool {
    windows
        .iter()
        .filter(|window| Some(window.id) == workspace.active_window_id)
        .any(|window| {
            let (width, height) = window.layout.tile_size;
            window.layout.pos_in_scrolling_layout.is_some()
                && width >= f64::from(output.width) - 1.0
                && height >= f64::from(output.height) - 1.0
        })
}

/// Where a floating window's geometry starts in the layout: the output's origin, plus the
/// tile's place in the workspace's view, plus the window's offset in its tile.
fn window_origin(output: &LogicalOutput, layout: &WindowLayout) -> Result<LayoutPt, Unmappable> {
    let (tile_x, tile_y) = layout
        .tile_pos_in_workspace_view
        .ok_or(Unmappable::NotShowing)?;
    let (offset_x, offset_y) = layout.window_offset_in_tile;
    Ok(LayoutPt {
        x: f64::from(output.x) + tile_x + offset_x,
        y: f64::from(output.y) + tile_y + offset_y,
    })
}

/// The frame guard: the frame's WINDOW extents are niri's window size, give or take a
/// pixel of rounding.
pub(crate) const fn frame_fits(frame: Extents, window_size: (i32, i32)) -> bool {
    (frame.width - window_size.0).abs() <= 1 && (frame.height - window_size.1).abs() <= 1
}

/// What decides whether an element can be aimed at.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Placement {
    pub(crate) states: States,
    pub(crate) extents: Extents,
    pub(crate) frame_fits: bool,
    pub(crate) origin: Result<Origin, Unmappable>,
}

/// The element's box in the layout, or why it has none.
pub(crate) fn place(placement: Placement) -> Result<LayoutBox, Unmappable> {
    let Placement {
        states,
        extents,
        frame_fits,
        origin,
    } = placement;
    if !(states.has(State::Showing) && states.has(State::Visible)) {
        return Err(Unmappable::NotShowing);
    }
    if extents.width <= 0 || extents.height <= 0 {
        return Err(Unmappable::Empty);
    }
    if !frame_fits {
        return Err(Unmappable::FrameSizeMismatch);
    }
    let origin = origin?;
    let found = LayoutBox {
        x: origin.at.x + f64::from(extents.x),
        y: origin.at.y + f64::from(extents.y),
        width: f64::from(extents.width),
        height: f64::from(extents.height),
    };
    if !on(&origin.area, found.centre()) {
        return Err(Unmappable::OffOutput);
    }
    Ok(found)
}

/// Whether `point` lies on the output whose layout area is `area`.
fn on(area: &LogicalOutput, point: LayoutPt) -> bool {
    let (x, y) = (f64::from(area.x), f64::from(area.y));
    (x..x + f64::from(area.width)).contains(&point.x)
        && (y..y + f64::from(area.height)).contains(&point.y)
}

/// One of an application's top-level frames.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Frame<'a> {
    pub(crate) extents: Extents,
    /// The frame's accessible name, which toolkits set to the window title.
    pub(crate) name: &'a str,
}

/// The frame that is niri's window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Picked {
    /// The frame at this index passes the frame guard.
    Fits(usize),
    /// The application's only frame, or the only one with the window's title, fails it.
    Mismatch(usize),
}

/// One of niri's windows of the application, as frames are matched to it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Shape<'a> {
    pub(crate) size: (i32, i32),
    pub(crate) title: Option<&'a str>,
}

/// Why no frame is niri's window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NoFrame {
    /// The application has no frames.
    None,
    /// Several frames fit, and the title doesn't tell them apart.
    Ambiguous(usize),
    /// The frame of the window's size doesn't have the window's title, and another frame
    /// does: the size and the title point at different frames.
    Contradicted,
    /// Another of the application's windows could be the picked frame as well: no other
    /// frame has its size, and the titles don't tell the two windows apart.
    Shared,
}

/// Picks the frame that is niri's `window` among an application's frames, deciding for
/// the app's `others` windows together: the one that passes the frame guard, or failing
/// that the only frame. When several qualify, only the one whose name is the window's
/// title is taken. A size match never overrides the title: a frame of the window's size
/// without its title, beside another frame with it, is `Contradicted`, as when the
/// window's own frame is larger, with client-side decorations, and another window of the
/// app happens to have its size. Nor does it bind a frame another window could own, with
/// the same title or one of the titles unknown, as two "Untitled" documents are: that is
/// `Shared`.
pub(crate) fn pick_frame(
    frames: &[Frame<'_>],
    window: Shape<'_>,
    others: &[Shape<'_>],
) -> Result<Picked, NoFrame> {
    let fitting: Vec<usize> = (0..frames.len())
        .filter(|&index| {
            frames
                .get(index)
                .is_some_and(|frame| frame_fits(frame.extents, window.size))
        })
        .collect();
    let picked = if fitting.is_empty() {
        let all: Vec<usize> = (0..frames.len()).collect();
        Picked::Mismatch(only(frames, &all, window.title)?)
    } else {
        let picked = only(frames, &fitting, window.title)?;
        if titled_elsewhere(frames, picked, window.title) {
            return Err(NoFrame::Contradicted);
        }
        Picked::Fits(picked)
    };
    let (Picked::Fits(index) | Picked::Mismatch(index)) = picked;
    if shared(frames, index, window.title, others) {
        return Err(NoFrame::Shared);
    }
    Ok(picked)
}

/// Whether the frame at `picked` lacks `title` while another frame has it.
fn titled_elsewhere(frames: &[Frame<'_>], picked: usize, title: Option<&str>) -> bool {
    let Some(title) = title else {
        return false;
    };
    let named = |index: usize| frames.get(index).is_some_and(|frame| frame.name == title);
    !named(picked) && (0..frames.len()).any(named)
}

/// Whether another window of the app could be the frame at `picked` instead of the
/// window titled `title`. Only `others` whose titles are the same or unknown count. For
/// each of their sizes, when more of them have that size, within a pixel, than other frames
/// fit it, one of them has no frame of its own and could be the picked one.
fn shared(frames: &[Frame<'_>], picked: usize, title: Option<&str>, others: &[Shape<'_>]) -> bool {
    let alike: Vec<(i32, i32)> = others
        .iter()
        .filter(|other| match (title, other.title) {
            (Some(title), Some(other)) => title == other,
            _ => true,
        })
        .map(|other| other.size)
        .collect();
    alike.iter().any(|&size| {
        // Within a pixel, as the frame guard counts sizes, so tiled siblings whose heights
        // round apart count as one size.
        let need = alike
            .iter()
            .filter(|other| (other.0 - size.0).abs() <= 1 && (other.1 - size.1).abs() <= 1)
            .count();
        let have = frames
            .iter()
            .enumerate()
            .filter(|(index, frame)| *index != picked && frame_fits(frame.extents, size))
            .count();
        need > have
    })
}

/// The one candidate, or the one whose name is `title`.
fn only(frames: &[Frame<'_>], candidates: &[usize], title: Option<&str>) -> Result<usize, NoFrame> {
    match candidates {
        [] => Err(NoFrame::None),
        [one] => Ok(*one),
        several => {
            let titled: Vec<usize> = several
                .iter()
                .copied()
                .filter(|&index| {
                    frames
                        .get(index)
                        .is_some_and(|frame| title.is_some_and(|title| frame.name == title))
                })
                .collect();
            match titled.as_slice() {
                [one] => Ok(*one),
                _ => Err(NoFrame::Ambiguous(several.len())),
            }
        }
    }
}

/// The `elements` filters: an exact role name, text the name contains, ignoring case, and
/// whether only interactive elements count.
#[derive(Debug, Clone, Default)]
pub(crate) struct Filter {
    pub(crate) role: Option<String>,
    /// Lowercased.
    name_contains: Option<String>,
    /// Only elements with actions or editable text, unless a role was asked for.
    pub(crate) interactive_only: bool,
}

impl Filter {
    pub(crate) fn new(
        role: Option<String>,
        name_contains: Option<&str>,
        interactive_only: bool,
    ) -> Self {
        Self {
            role,
            name_contains: name_contains.map(str::to_lowercase),
            interactive_only,
        }
    }

    pub(crate) fn matches(&self, role: &str, name: &str) -> bool {
        self.role.as_deref().is_none_or(|wanted| wanted == role)
            && self
                .name_contains
                .as_deref()
                .is_none_or(|part| name.to_lowercase().contains(part))
    }
}

/// An element as a ref keeps it, to check again before aiming at it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Kept {
    pub(crate) role: u32,
    pub(crate) window: u64,
    pub(crate) pid: i32,
    /// The action names it listed, which `activate_element` chooses from.
    pub(crate) actions: Vec<String>,
    /// Where it was in its window's tree, to tell it apart from another object that took
    /// its path, as a virtualized list reuses its rows: each object from the frame's child
    /// down to the element, the element last. Never logged or returned.
    pub(crate) lineage: Vec<Link>,
    /// Whether the record it acts on is bound to the ref. Never logged or returned.
    pub(crate) row: Row,
}

/// How a ref is bound to the record its element acts on, as a list's `Delete` button
/// acts on its row's record, from the nearest row around it: a list item, table row or
/// tree item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Row {
    /// The lineage binds it: its row has a name, which the lineage keeps as it keeps every
    /// name above the element, or it sits in no row and no other element read has its
    /// role and name.
    Lineage,
    /// It is a named row itself, so its own name is compared for the pointer tools too.
    Own,
    /// Nothing on the bus binds it to a record, so the ref is never acted on.
    Unbound(Unbound),
}

/// Why nothing binds a ref to its record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Unbound {
    /// Its row has no name.
    UnnamedRow,
    /// It sits in no row, and other elements read have its role and name, as when a
    /// toolkit puts a list's rows on the bus without containers.
    Repeated,
}

/// One object on the way from a window's frame down to an element: its path, its index
/// among its parent's children, and its accessible name's hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Link {
    pub(crate) path: String,
    pub(crate) index: i32,
    pub(crate) name: NameHash,
}

/// A hash of an accessible name, to tell whether the name changed without keeping it.
/// Kept in memory only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NameHash(u64);

impl NameHash {
    pub(crate) fn of(name: &str) -> Self {
        let mut hasher = DefaultHasher::new();
        name.hash(&mut hasher);
        Self(hasher.finish())
    }
}

/// The element and its window as they are now.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Fresh {
    pub(crate) role: u32,
    pub(crate) placement: Placement,
}

/// Why a ref can't be aimed at any more.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Refused {
    /// The element is gone or is now something else.
    Stale(String),
    Unmappable(Unmappable),
}

/// Checks a kept element against how it is now: the same role, then a layout box.
pub(crate) fn recheck(kept: &Kept, fresh: Fresh) -> Result<LayoutBox, Refused> {
    if fresh.role != kept.role {
        return Err(Refused::Stale(format!(
            "the element was a {} and is now a {}",
            role_name(kept.role),
            role_name(fresh.role)
        )));
    }
    place(fresh.placement).map_err(Refused::Unmappable)
}

#[cfg(test)]
mod tests {
    use niri_ipc::Transform;

    use super::*;

    const SHOWN: States = States((1 << 25) | (1 << 30));

    fn output(x: i32, y: i32, scale: f64) -> LogicalOutput {
        LogicalOutput {
            x,
            y,
            width: 960,
            height: 720,
            scale,
            transform: Transform::Flipped180,
        }
    }

    fn layout(tile: Option<(f64, f64)>, offset: (f64, f64)) -> WindowLayout {
        WindowLayout {
            pos_in_scrolling_layout: None,
            tile_size: (400.0, 300.0),
            window_size: (400, 300),
            tile_pos_in_workspace_view: tile,
            window_offset_in_tile: offset,
        }
    }

    const fn extents(x: i32, y: i32, width: i32, height: i32) -> Extents {
        Extents {
            x,
            y,
            width,
            height,
        }
    }

    /// A placement whose window starts at `origin` on an output far larger than any test's
    /// boxes.
    fn placement(extents: Extents, origin: Result<LayoutPt, Unmappable>) -> Placement {
        let area = LogicalOutput {
            x: -10_000,
            y: -10_000,
            width: 20_000,
            height: 20_000,
            ..output(0, 0, 1.0)
        };
        Placement {
            states: SHOWN,
            extents,
            frame_fits: true,
            origin: origin.map(|at| Origin {
                at,
                output: "DP-1".to_owned(),
                area,
            }),
        }
    }

    #[test]
    fn a_box_is_the_windows_place_in_the_layout_plus_its_window_extents() {
        // GTK 4 with CSD at scale 1: the Primary button, its tile at (20, 54).
        let origin = window_origin(&output(0, 0, 1.0), &layout(Some((20.0, 54.0)), (0.0, 0.0)));
        let primary = place(placement(extents(40, 109, 320, 34), origin)).unwrap();
        assert_eq!(
            primary,
            LayoutBox {
                x: 60.0,
                y: 163.0,
                width: 320.0,
                height: 34.0
            }
        );
        assert_eq!(primary.centre(), LayoutPt { x: 220.0, y: 180.0 });
    }

    #[test]
    fn fractional_tile_positions_offsets_and_output_origins_carry_through() {
        // At scale 1.5 niri places tiles at fractional logical positions.
        let origin = window_origin(
            &output(-1280, 100, 1.5),
            &layout(Some((57.333_333, 65.333_333)), (1.5, 2.25)),
        )
        .unwrap();
        let found = place(placement(extents(40, 70, 320, 34), Ok(origin))).unwrap();
        assert!((found.x - (-1280.0 + 57.333_333 + 1.5 + 40.0)).abs() < 1e-9);
        assert!((found.y - (100.0 + 65.333_333 + 2.25 + 70.0)).abs() < 1e-9);
        assert!((found.centre().x - (found.x + 160.0)).abs() < 1e-9);
    }

    #[test]
    fn the_frame_guard_allows_a_pixel_of_rounding_and_nothing_more() {
        let fits = |width, height| frame_fits(extents(0, 0, width, height), (400, 300));
        assert!(fits(400, 300));
        assert!(fits(401, 299));
        assert!(fits(399, 301));
        // GTK 3 measures from its shadow, Qt from inside its decorations.
        assert!(!fits(452, 352));
        assert!(!fits(394, 267));
        assert!(!fits(402, 300));
    }

    #[test]
    fn hidden_empty_and_unplaced_elements_have_no_box() {
        let origin = Ok(LayoutPt { x: 0.0, y: 0.0 });
        let hidden = Placement {
            states: States(1 << 30),
            ..placement(extents(1, 1, 10, 10), origin)
        };
        assert_eq!(place(hidden), Err(Unmappable::NotShowing));
        // Qt lists widgets on hidden tabs with no size.
        assert_eq!(
            place(placement(extents(0, 0, 0, 0), origin)),
            Err(Unmappable::Empty)
        );
        let mismatch = Placement {
            frame_fits: false,
            ..placement(extents(66, 131, 320, 34), origin)
        };
        assert_eq!(place(mismatch), Err(Unmappable::FrameSizeMismatch));
        // A floating window niri gives no place has none in the view.
        let unshown = window_origin(&output(0, 0, 1.0), &layout(None, (0.0, 0.0)));
        assert_eq!(unshown, Err(Unmappable::NotShowing));
        assert_eq!(
            place(placement(extents(1, 1, 10, 10), unshown)),
            Err(Unmappable::NotShowing)
        );
    }

    fn window_on(workspace: Option<u64>, layout: WindowLayout) -> Window {
        Window {
            id: 1,
            title: None,
            app_id: None,
            pid: None,
            workspace_id: workspace,
            is_focused: false,
            is_floating: layout.pos_in_scrolling_layout.is_none(),
            is_urgent: false,
            layout,
            focus_timestamp: None,
        }
    }

    fn workspace(id: u64, output: Option<&str>, active: bool) -> Workspace {
        Workspace {
            id,
            idx: 1,
            name: None,
            output: output.map(str::to_owned),
            is_urgent: false,
            is_active: active,
            is_focused: active,
            active_window_id: None,
        }
    }

    /// Outputs by name, each on at `(x, y)` or off.
    fn outputs(outputs: &[(&str, Option<(i32, i32)>)]) -> BTreeMap<String, Output> {
        outputs
            .iter()
            .map(|&(name, at)| {
                let output = Output {
                    name: name.to_owned(),
                    make: String::new(),
                    model: String::new(),
                    serial: None,
                    physical_size: None,
                    modes: Vec::new(),
                    current_mode: None,
                    is_custom_mode: false,
                    vrr_supported: false,
                    vrr_enabled: false,
                    logical: at.map(|(x, y)| output(x, y, 1.0)),
                };
                (name.to_owned(), output)
            })
            .collect()
    }

    /// A scene of `workspaces` on `outputs` with no other window.
    fn scene<'a>(
        workspaces: &'a [Workspace],
        outputs: &'a BTreeMap<String, Output>,
        overview_open: bool,
    ) -> Scene<'a> {
        Scene {
            workspaces,
            outputs,
            windows: &[],
            overview_open,
        }
    }

    #[test]
    fn a_floating_window_is_placed_only_on_the_workspace_its_output_shows() {
        let floating = layout(Some((20.0, 30.0)), (0.0, 0.0));
        let screens = outputs(&[("DP-1", Some((0, 0))), ("HDMI-A-1", Some((960, 0)))]);
        let shown = [
            workspace(1, Some("DP-1"), true),
            workspace(2, Some("HDMI-A-1"), true),
        ];
        let at = |id| {
            origin(
                &window_on(Some(id), floating.clone()),
                &scene(&shown, &screens, false),
            )
            .map(|origin| (origin.at, origin.output))
        };
        assert_eq!(
            at(1),
            Ok((LayoutPt { x: 20.0, y: 30.0 }, "DP-1".to_owned()))
        );
        let second = LayoutPt { x: 980.0, y: 30.0 };
        assert_eq!(at(2), Ok((second, "HDMI-A-1".to_owned())));
        // The overview zooms the workspaces out, and niri's place stays as it was.
        let zoomed = origin(
            &window_on(Some(1), floating.clone()),
            &scene(&shown, &screens, true),
        );
        assert_eq!(zoomed, Err(Unmappable::NotShowing));
        // niri still reports the window's place once another workspace is shown on its
        // output, where that workspace's windows now are.
        let hidden = [
            workspace(1, Some("DP-1"), false),
            workspace(3, Some("DP-1"), true),
        ];
        let behind = window_on(Some(1), floating.clone());
        assert_eq!(
            origin(&behind, &scene(&hidden, &screens, false)),
            Err(Unmappable::NotShowing)
        );
        // No workspace, an unknown one, one on no output or on an output that is off.
        let off = outputs(&[("DP-1", None)]);
        let nowhere = [workspace(1, None, true)];
        for (unplaced, workspaces, on) in [
            (window_on(None, floating.clone()), &shown[..], &screens),
            (window_on(Some(9), floating.clone()), &shown[..], &screens),
            (window_on(Some(1), floating.clone()), &nowhere[..], &screens),
            (window_on(Some(1), floating.clone()), &shown[..], &off),
        ] {
            assert_eq!(
                origin(&unplaced, &scene(workspaces, on, false)),
                Err(Unmappable::NotShowing),
                "{unplaced:?}"
            );
        }
    }

    // niri lets a floating window reach past its output's edge and shows that part on no
    // output, so an element whose centre is there has no box, even where another output
    // lies.
    #[test]
    fn an_element_past_its_windows_output_has_no_box() {
        let screens = outputs(&[("DP-1", Some((0, 0))), ("HDMI-A-1", Some((960, 0)))]);
        let shown = [
            workspace(1, Some("DP-1"), true),
            workspace(2, Some("HDMI-A-1"), true),
        ];
        let window = window_on(Some(1), layout(Some((700.0, 30.0)), (0.0, 0.0)));
        let at = |x| {
            place(Placement {
                origin: origin(&window, &scene(&shown, &screens, false)),
                ..placement(extents(x, 10, 20, 20), Ok(LayoutPt { x: 0.0, y: 0.0 }))
            })
        };
        // Centres at x 950 and 959 are on DP-1, one at 960 on HDMI-A-1, one at -10 on none.
        assert!(at(240).is_ok());
        assert_eq!(
            at(249).map(LayoutBox::centre).map(|centre| centre.x),
            Ok(959.0)
        );
        assert_eq!(at(250), Err(Unmappable::OffOutput));
        assert_eq!(at(-720), Err(Unmappable::OffOutput));
    }

    // niri draws no floating window of a workspace whose active window is a fullscreen
    // tiled one, and sends the pointer to that window.
    #[test]
    fn a_fullscreen_window_covers_its_workspaces_floating_windows() {
        let floating = window_on(Some(1), layout(Some((20.0, 30.0)), (0.0, 0.0)));
        let screens = outputs(&[("DP-1", Some((0, 0)))]);
        let other = |id, tiled: bool, size| {
            let mut window = window_on(
                Some(1),
                WindowLayout {
                    pos_in_scrolling_layout: tiled.then_some((1, 1)),
                    tile_size: size,
                    ..layout(Some((0.0, 0.0)), (0.0, 0.0))
                },
            );
            window.id = id;
            window
        };
        let windows = [
            other(5, true, (960.0, 720.0)),
            other(6, true, (959.5, 719.5)),
            other(7, true, (944.0, 704.0)),
            other(8, false, (960.0, 720.0)),
        ];
        let active = |id| {
            let mut shown = workspace(1, Some("DP-1"), true);
            shown.active_window_id = id;
            [shown]
        };
        let at = |id| {
            let workspaces = active(id);
            let scene = Scene {
                windows: &windows,
                ..scene(&workspaces, &screens, false)
            };
            origin(&floating, &scene).map(|origin| origin.at)
        };
        let placed = Ok(LayoutPt { x: 20.0, y: 30.0 });
        // Fullscreen, at a fractional scale too.
        assert_eq!(at(Some(5)), Err(Unmappable::Covered));
        assert_eq!(at(Some(6)), Err(Unmappable::Covered));
        // A column that leaves gaps, a floating window as large as the output, the
        // floating window itself active, or none.
        assert_eq!(at(Some(7)), placed);
        assert_eq!(at(Some(8)), placed);
        assert_eq!(at(Some(1)), placed);
        assert_eq!(at(None), placed);
    }

    #[test]
    fn a_tiled_window_is_never_placed_and_says_it_is_tiled() {
        let tiled = |tile| WindowLayout {
            pos_in_scrolling_layout: Some((2, 1)),
            ..layout(tile, (0.0, 0.0))
        };
        let screens = outputs(&[("DP-1", Some((0, 0)))]);
        let shown = [workspace(1, Some("DP-1"), true)];
        // niri 26.04 gives a tiled window no place in the view: the element is on
        // screen, so `not_showing` would send the agent looking for it.
        let unplaced = origin(
            &window_on(Some(1), tiled(None)),
            &scene(&shown, &screens, false),
        );
        assert_eq!(unplaced, Err(Unmappable::Tiled));
        assert_eq!(
            place(placement(
                extents(40, 70, 320, 34),
                unplaced.map(|origin| origin.at)
            )),
            Err(Unmappable::Tiled)
        );
        // A place niri gives a tiled window isn't trusted either.
        let given = window_on(Some(1), tiled(Some((16.0, 16.0))));
        assert_eq!(
            origin(&given, &scene(&shown, &screens, false)),
            Err(Unmappable::Tiled)
        );
        let overview = origin(
            &window_on(Some(1), tiled(None)),
            &scene(&shown, &screens, true),
        );
        assert_eq!(overview, Err(Unmappable::Tiled));
        // Whatever niri says of its workspace: tiled, it wouldn't be placed if shown.
        let hidden = [workspace(1, Some("DP-1"), false)];
        let nowhere = [workspace(1, None, true)];
        let off = outputs(&[("DP-1", None)]);
        for (workspace, workspaces, on) in [
            (Some(1), &hidden[..], &screens),
            (Some(1), &nowhere[..], &screens),
            (Some(1), &shown[..], &off),
            (Some(9), &shown[..], &screens),
            (None, &shown[..], &screens),
        ] {
            let window = window_on(workspace, tiled(None));
            assert_eq!(
                origin(&window, &scene(workspaces, on, false)),
                Err(Unmappable::Tiled),
                "{workspace:?} {workspaces:?}"
            );
        }
    }

    // Floating a tiled window fixes only `tiled`, so whatever else keeps an element from a
    // box is reported first: a hidden element, then one with no area, then a frame that
    // doesn't fit. A ref aimed checks the same placement as the listing.
    #[test]
    fn a_tiled_windows_element_says_what_floating_would_not_fix_first() {
        let showing = States(1 << 25);
        let visible = States(1 << 30);
        let fine = extents(40, 70, 320, 34);
        let cases = [
            (SHOWN, fine, true, Unmappable::Tiled),
            (showing, fine, true, Unmappable::NotShowing),
            (visible, fine, true, Unmappable::NotShowing),
            (States::default(), fine, true, Unmappable::NotShowing),
            (SHOWN, extents(40, 70, 0, 34), true, Unmappable::Empty),
            (SHOWN, extents(40, 70, 320, -1), true, Unmappable::Empty),
            (SHOWN, fine, false, Unmappable::FrameSizeMismatch),
            (showing, extents(0, 0, 0, 0), false, Unmappable::NotShowing),
            (SHOWN, extents(0, 0, 0, 0), false, Unmappable::Empty),
        ];
        let kept = Kept {
            role: 43,
            window: 1,
            pid: 4711,
            actions: Vec::new(),
            lineage: Vec::new(),
            row: Row::Lineage,
        };
        for (states, extents, frame_fits, reason) in cases {
            let placement = Placement {
                states,
                extents,
                frame_fits,
                origin: Err(Unmappable::Tiled),
            };
            assert_eq!(place(placement.clone()), Err(reason), "{placement:?}");
            let fresh = Fresh {
                role: 43,
                placement: placement.clone(),
            };
            assert_eq!(
                recheck(&kept, fresh),
                Err(Refused::Unmappable(reason)),
                "{placement:?}"
            );
        }
    }

    #[test]
    fn picks_the_frame_that_is_the_window() {
        let frame = |width, height, name| Frame {
            extents: extents(0, 0, width, height),
            name,
        };
        let size = (400, 300);
        let window = |title| Shape { size, title };
        let pick = |frames: &[Frame<'_>], title| pick_frame(frames, window(title), &[]);
        assert_eq!(
            pick(&[frame(200, 100, "about"), frame(400, 300, "main")], None),
            Ok(Picked::Fits(1))
        );
        // One frame that fails the guard is still the window, but unmappable.
        assert_eq!(
            pick(&[frame(452, 352, "gtk3")], Some("gtk3")),
            Ok(Picked::Mismatch(0))
        );
        let twins = [frame(400, 300, "one"), frame(400, 300, "two")];
        assert_eq!(pick(&twins, Some("two")), Ok(Picked::Fits(1)));
        assert_eq!(pick(&twins, Some("three")), Err(NoFrame::Ambiguous(2)));
        assert_eq!(pick(&twins, None), Err(NoFrame::Ambiguous(2)));
        // Two windows of one app: this one's frame is larger than niri's window, as with
        // client-side decorations, and the other's frame has this window's size.
        let mixed = [frame(450, 350, "this"), frame(400, 300, "other")];
        assert_eq!(pick(&mixed, Some("this")), Err(NoFrame::Contradicted));
        assert_eq!(pick(&mixed, Some("other")), Ok(Picked::Fits(1)));
        // A title no frame has leaves the size to decide.
        assert_eq!(pick(&mixed, Some("renamed")), Ok(Picked::Fits(1)));
        let unfit = [frame(10, 10, "a"), frame(20, 20, "b")];
        assert_eq!(pick(&unfit, Some("b")), Ok(Picked::Mismatch(1)));
        assert_eq!(pick(&[], None), Err(NoFrame::None));
    }

    #[test]
    fn a_frame_another_window_of_the_app_could_be_is_never_bound() {
        let frame = |width, height, name| Frame {
            extents: extents(0, 0, width, height),
            name,
        };
        let shape = |size, title| Shape { size, title };
        let this = shape((400, 300), Some("Untitled"));
        // The mixed case with one title: this window's frame is larger than niri's window,
        // as with client-side decorations, and the other window's frame has its size.
        let csd = [frame(450, 350, "Untitled"), frame(400, 300, "Untitled")];
        let other = shape((400, 300), Some("Untitled"));
        assert_eq!(pick_frame(&csd, this, &[other]), Err(NoFrame::Shared));
        assert_eq!(pick_frame(&csd, other, &[this]), Err(NoFrame::Shared));
        // Without a title on either side, the titles don't tell the windows apart either.
        let unnamed = [frame(450, 350, ""), frame(400, 300, "")];
        let untitled = shape((400, 300), None);
        assert_eq!(
            pick_frame(&unnamed, untitled, &[other]),
            Err(NoFrame::Shared)
        );
        assert_eq!(pick_frame(&csd, this, &[untitled]), Err(NoFrame::Shared));
        // An only frame that fails the guard could be the other window's as well.
        let lone = [frame(450, 350, "Untitled")];
        assert_eq!(
            pick_frame(&lone, this, &[shape((300, 200), Some("Untitled"))]),
            Err(NoFrame::Shared)
        );
        // Windows with server-side decorations, one title and different sizes each have a
        // frame of their own size, so each binds its own.
        let ssd = [frame(400, 300, "Untitled"), frame(300, 200, "Untitled")];
        let smaller = shape((300, 200), Some("Untitled"));
        assert_eq!(pick_frame(&ssd, this, &[smaller]), Ok(Picked::Fits(0)));
        assert_eq!(pick_frame(&ssd, smaller, &[this]), Ok(Picked::Fits(1)));
        // Two other windows of one size and one frame of that size left: one of them has
        // no frame of its own and could be the picked one.
        let three = [
            frame(450, 350, ""),
            frame(400, 300, ""),
            frame(350, 250, ""),
        ];
        let floating = shape((400, 300), None);
        let twins = [shape((350, 250), None), shape((350, 250), None)];
        assert_eq!(pick_frame(&three, floating, &twins), Err(NoFrame::Shared));
        let rounded = [shape((350, 250), None), shape((350, 251), None)];
        assert_eq!(pick_frame(&three, floating, &rounded), Err(NoFrame::Shared));
        // Distinct titles keep the title rule.
        let named = shape((400, 300), Some("Other"));
        assert_eq!(
            pick_frame(
                &[frame(450, 350, "x"), frame(400, 300, "Other")],
                named,
                &[this]
            ),
            Ok(Picked::Fits(1))
        );
    }

    #[test]
    fn filters_by_exact_role_and_name_text_in_any_case() {
        let all = Filter::default();
        assert!(all.matches("label", ""));
        let buttons = Filter::new(Some("button".to_owned()), Some("PRIMARY"), false);
        assert!(buttons.matches("button", "Primary: 3"));
        assert!(!buttons.matches("button", "Second: 0"));
        assert!(!buttons.matches("toggle_button", "Primary"));
        assert!(is_role("check_box"));
        assert!(!is_role("push button"));
    }

    #[test]
    fn roles_and_states_are_named_from_the_at_spi_tables() {
        assert_eq!(role_name(23), "frame");
        assert_eq!(role_name(43), "button");
        assert_eq!(role_name(61), "text");
        assert_eq!(role_name(130), "switch");
        assert_eq!(role_name(131), "unknown");
        let states = States::from_words(&[(1 << 8) | (1 << 25) | (1 << 30), 1 << 11]);
        assert!(states.has(State::Showing) && states.has(State::Visible));
        assert!(!States::from_words(&[1 << 30]).has(State::Showing));
        assert_eq!(
            states.names(),
            ["enabled", "showing", "visible", "read_only"]
        );
        assert_eq!(States::from_words(&[]), States::default());
        let pressed = States::from_words(&[(1 << 8) | (1 << 20) | (1 << 30)]);
        assert_eq!(
            pressed.changes(states),
            (vec!["pressed"], vec!["showing", "read_only"])
        );
        assert_eq!(states.changes(states), (vec![], vec![]));
    }

    #[test]
    fn a_kept_element_is_stale_once_it_changes_role_and_unmappable_once_hidden() {
        let kept = Kept {
            role: 43,
            window: 3,
            pid: 4711,
            actions: Vec::new(),
            lineage: Vec::new(),
            row: Row::Lineage,
        };
        let shown = placement(extents(40, 70, 320, 34), Ok(LayoutPt { x: 20.0, y: 20.0 }));
        let fresh = |role, placement| Fresh { role, placement };
        assert_eq!(
            recheck(&kept, fresh(43, shown.clone())).map(LayoutBox::centre),
            Ok(LayoutPt { x: 220.0, y: 107.0 })
        );
        assert!(matches!(
            recheck(&kept, fresh(29, shown.clone())),
            Err(Refused::Stale(detail)) if detail.contains("button") && detail.contains("label")
        ));
        let hidden = Placement {
            states: States::default(),
            ..shown
        };
        assert_eq!(
            recheck(&kept, fresh(43, hidden)),
            Err(Refused::Unmappable(Unmappable::NotShowing))
        );
    }
}
