//! What an action changed among one window's accessible elements, from a walk before it
//! and one after it (pure). An element is the same element when the object paths from the
//! frame down to it are the same: an index alone shifting, as when a sibling is inserted
//! above it, is no change, and an element moved under another parent is one removed and
//! one added.

use std::collections::HashMap;

use serde::Serialize;

use super::cut;
use crate::a11y::model::{self, States};
use crate::a11y::{Capped, ElementRef};
use crate::error::{ErrorName, ToolError};

/// At most this many entries across `added`, `removed` and `changed`.
const ELEMENTS: usize = 20;

/// One element a walk listed, as `elements` would list it.
#[derive(Debug, Clone)]
pub(crate) struct Seen {
    /// What a ref to it keeps.
    pub(crate) element: ElementRef,
    /// The app's text: returned, never logged.
    pub(crate) name: String,
    pub(crate) states: States,
}

impl Seen {
    /// The object paths from the frame's child down to the element.
    fn key(&self) -> Vec<&str> {
        self.element
            .kept
            .lineage
            .iter()
            .map(|link| link.path.as_str())
            .collect()
    }

    fn role(&self) -> &'static str {
        model::role_name(self.element.kept.role)
    }
}

/// What `changes.elements` says.
#[derive(Debug, PartialEq, Serialize)]
#[serde(untagged)]
pub(crate) enum Report {
    Diff(Diff),
    Unknown(Unknown),
}

/// The elements of `window_id` that were added, removed or changed.
#[derive(Debug, PartialEq, Serialize)]
pub(crate) struct Diff {
    pub(crate) window_id: u64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) added: Vec<Added>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) removed: Vec<Removed>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) changed: Vec<Changed>,
    /// Entries left out past the cap.
    #[serde(skip_serializing_if = "super::is_zero")]
    pub(crate) truncated: usize,
}

/// Why the elements' changes aren't known.
#[derive(Debug, PartialEq, Serialize)]
pub(crate) struct Unknown {
    /// None when no window had focus to walk.
    pub(crate) window_id: Option<u64>,
    #[serde(rename = "unknown")]
    pub(crate) why: Why,
    pub(crate) detail: String,
}

/// Why: the window is gone, a walk stopped early, or an error's name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Why {
    /// No window had keyboard focus when the action started.
    NoWindow,
    WindowGone,
    NodeCap,
    BudgetExhausted,
    ChildrenRefused,
    #[serde(untagged)]
    Error(ErrorName),
}

impl Unknown {
    pub(crate) fn failed(window_id: Option<u64>, error: ToolError) -> Self {
        Self {
            window_id,
            why: Why::Error(error.name),
            detail: error.detail,
        }
    }

    pub(crate) fn no_window() -> Self {
        Self {
            window_id: None,
            why: Why::NoWindow,
            detail: "no window had keyboard focus when the action started".to_owned(),
        }
    }

    pub(crate) fn gone(window_id: u64) -> Self {
        Self {
            window_id: Some(window_id),
            why: Why::WindowGone,
            detail: format!("window {window_id} is gone"),
        }
    }

    pub(crate) fn capped(window_id: u64, capped: Capped) -> Self {
        let (why, detail) = match capped {
            Capped::NodeCap => (
                Why::NodeCap,
                "a walk read as many accessible objects as it may",
            ),
            Capped::BudgetExhausted => {
                (Why::BudgetExhausted, "a walk ran out of its three seconds")
            }
            Capped::ChildrenRefused => (
                Why::ChildrenRefused,
                "the app refused to list an object's children",
            ),
        };
        Self {
            window_id: Some(window_id),
            why,
            detail: detail.to_owned(),
        }
    }
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Added {
    pub(crate) element_ref: Option<String>,
    pub(crate) role: &'static str,
    pub(crate) name: String,
    pub(crate) states: Vec<&'static str>,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Removed {
    pub(crate) role: &'static str,
    pub(crate) name: String,
}

/// An element whose role, name or states changed: its new ref, role and name, its name
/// before when that changed, and the states set and cleared.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Changed {
    pub(crate) element_ref: Option<String>,
    pub(crate) role: &'static str,
    pub(crate) name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) was: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) states_set: Vec<&'static str>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) states_cleared: Vec<&'static str>,
}

/// The elements of window `window_id` added, removed and changed from `before` to
/// `after`, or none when nothing changed. `remember` keeps a ref to each added or changed
/// element and names it, as `elements` does, since a changed name makes an older ref
/// stale.
pub(crate) fn diff(
    window_id: u64,
    before: &[Seen],
    after: &[Seen],
    mut remember: impl FnMut(&Seen) -> Option<String>,
) -> Option<Diff> {
    let later: HashMap<Vec<&str>, &Seen> = after.iter().map(|seen| (seen.key(), seen)).collect();
    let mut kept = 0;
    let mut dropped = 0;
    let mut room = || {
        let fits = kept < ELEMENTS;
        if fits {
            kept += 1;
        } else {
            dropped += 1;
        }
        fits
    };
    let mut added = Vec::new();
    let mut changed = Vec::new();
    for (_, seen, old) in entries(before, after) {
        if !room() {
            continue;
        }
        match old {
            None => added.push(Added {
                element_ref: remember(seen),
                role: seen.role(),
                name: cut(&seen.name),
                states: seen.states.names(),
            }),
            Some(old) => changed.push(change(old, seen, &mut remember)),
        }
    }
    let removed: Vec<Removed> = before
        .iter()
        .filter(|seen| !later.contains_key(&seen.key()))
        .filter(|_| room())
        .map(|seen| Removed {
            role: seen.role(),
            name: cut(&seen.name),
        })
        .collect();
    let diff = Diff {
        window_id,
        added,
        removed,
        changed,
        truncated: dropped,
    };
    let empty = diff.added.is_empty() && diff.removed.is_empty() && diff.changed.is_empty();
    (!empty || diff.truncated > 0).then_some(diff)
}

/// Where in `after` the elements are that `diff` keeps a ref to, so their refs can be
/// made ready first.
pub(crate) fn reported(before: &[Seen], after: &[Seen]) -> Vec<usize> {
    entries(before, after)
        .take(ELEMENTS)
        .map(|(at, _, _)| at)
        .collect()
}

/// The elements of `after` added or changed from `before`, in order: where each is in
/// `after`, and what it was before, if it was there.
fn entries<'a>(
    before: &'a [Seen],
    after: &'a [Seen],
) -> impl Iterator<Item = (usize, &'a Seen, Option<&'a Seen>)> {
    let earlier: HashMap<Vec<&str>, &Seen> = before.iter().map(|seen| (seen.key(), seen)).collect();
    after
        .iter()
        .enumerate()
        .filter_map(move |(at, seen)| match earlier.get(&seen.key()) {
            None => Some((at, seen, None)),
            Some(old) if differs(old, seen) => Some((at, seen, Some(*old))),
            Some(_) => None,
        })
}

fn differs(old: &Seen, new: &Seen) -> bool {
    old.element.kept.role != new.element.kept.role
        || old.name != new.name
        || old.states != new.states
}

fn change(old: &Seen, new: &Seen, remember: &mut impl FnMut(&Seen) -> Option<String>) -> Changed {
    let (states_set, states_cleared) = new.states.changes(old.states);
    Changed {
        element_ref: remember(new),
        role: new.role(),
        name: cut(&new.name),
        was: (old.name != new.name).then(|| cut(&old.name)),
        states_set,
        states_cleared,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::a11y::model::{Kept, Link, NameHash, Row};

    const BUTTON: u32 = 43;
    const LABEL: u32 = 29;
    /// `enabled`, `sensitive`, `showing` and `visible`.
    const SHOWING: u32 = (1 << 8) | (1 << 24) | (1 << 25) | (1 << 30);
    const CHECKED: u32 = 1 << 4;

    /// An element under the frame at `paths`, each with index `index` in its parent.
    fn seen(paths: &[&str], index: i32, role: u32, name: &str, states: u32) -> Seen {
        let lineage = paths
            .iter()
            .map(|path| Link {
                path: (*path).to_owned(),
                index,
                name: NameHash::of(name),
            })
            .collect();
        Seen {
            element: ElementRef {
                bus: ":1.5".to_owned(),
                path: (*paths.last().unwrap()).to_owned(),
                frame: "/frame".to_owned(),
                kept: Kept {
                    role,
                    window: 3,
                    pid: 9,
                    actions: Vec::new(),
                    lineage,
                    row: Row::Lineage,
                },
            },
            name: name.to_owned(),
            states: States::from_words(&[states]),
        }
    }

    /// Refs named after the element's path.
    fn named(seen: &Seen) -> String {
        format!("ref{}", seen.element.path)
    }

    #[test]
    fn a_button_that_adds_one_element_and_removes_another_reports_both() {
        let before = [
            seen(&["/p", "/vanish"], 0, BUTTON, "Vanish", SHOWING),
            seen(&["/p", "/ok"], 1, BUTTON, "OK", SHOWING),
        ];
        let after = [
            seen(&["/p", "/ok"], 0, BUTTON, "OK", SHOWING),
            seen(&["/p", "/note"], 1, LABEL, "Saved", SHOWING),
        ];
        let diff = diff(3, &before, &after, |seen| Some(named(seen))).unwrap();
        assert_eq!(
            serde_json::to_value(&diff).unwrap(),
            json!({
                "window_id": 3,
                "added": [{
                    "element_ref": "ref/note", "role": "label", "name": "Saved",
                    "states": ["enabled", "sensitive", "showing", "visible"]
                }],
                "removed": [{"role": "button", "name": "Vanish"}],
            })
        );
    }

    #[test]
    fn the_same_paths_at_new_indexes_are_no_change_and_a_new_parent_is_a_new_element() {
        let before = [seen(&["/p", "/ok"], 1, BUTTON, "OK", SHOWING)];
        let shifted = [seen(&["/p", "/ok"], 4, BUTTON, "OK", SHOWING)];
        assert_eq!(diff(3, &before, &shifted, |seen| Some(named(seen))), None);
        let moved = [seen(&["/q", "/ok"], 1, BUTTON, "OK", SHOWING)];
        let diff = diff(3, &before, &moved, |seen| Some(named(seen))).unwrap();
        assert_eq!((diff.added.len(), diff.removed.len()), (1, 1));
    }

    #[test]
    fn a_relabelled_checked_button_is_changed_with_its_old_name_and_its_states() {
        let before = [seen(&["/p", "/count"], 0, BUTTON, "Count: 2", SHOWING)];
        let after = [seen(
            &["/p", "/count"],
            0,
            BUTTON,
            "Count: 3",
            SHOWING | CHECKED,
        )];
        let diff = diff(3, &before, &after, |seen| Some(named(seen))).unwrap();
        assert_eq!(
            serde_json::to_value(&diff).unwrap(),
            json!({
                "window_id": 3,
                "changed": [{
                    "element_ref": "ref/count", "role": "button", "name": "Count: 3",
                    "was": "Count: 2", "states_set": ["checked"]
                }],
            })
        );
    }

    #[test]
    fn entries_stop_at_the_cap_and_only_kept_entries_get_refs() {
        let after: Vec<Seen> = (0..25)
            .map(|at| seen(&[&format!("/e{at}")], at, BUTTON, "b", SHOWING))
            .collect();
        let mut refs = 0;
        let diff = diff(3, &[], &after, |seen| {
            refs += 1;
            Some(named(seen))
        })
        .unwrap();
        assert_eq!((diff.added.len(), diff.truncated, refs), (20, 5, 20));
    }

    #[test]
    fn an_unknown_report_names_the_error_or_why_the_walk_stopped() {
        let failed = Unknown::failed(
            Some(3),
            ToolError::new(ErrorName::NotAccessible, "gone".to_owned()),
        );
        assert_eq!(
            serde_json::to_value(Report::Unknown(failed)).unwrap(),
            json!({"window_id": 3, "unknown": "not_accessible", "detail": "gone"})
        );
        let capped = Unknown::capped(3, Capped::BudgetExhausted);
        assert_eq!(
            serde_json::to_value(Report::Unknown(capped)).unwrap()["unknown"],
            "budget_exhausted"
        );
    }
}
