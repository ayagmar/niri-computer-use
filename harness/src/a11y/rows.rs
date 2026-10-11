//! What GTK 4 puts on the accessibility bus for the list fixture's `Gtk.ListView`: each
//! object's path, role, name and parent, before and after a record is deleted or the list
//! scrolls, so the record of a run shows whether a row's objects keep their path while
//! showing another record, and whether a row is named after its record. The fixture's
//! names are synthetic, so reading them is safe.

use serde_json::{Value, json};

use super::hidden::{ACCESSIBLE, app, call, name};
use super::{Bus, ROOT};
use crate::failure::Result;
use crate::session::Session;

/// How deep the walk goes; the list's buttons sit a few levels under the window.
const DEPTH: usize = 12;

/// One object on the bus.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Object {
    pub(crate) path: String,
    pub(crate) role: u64,
    pub(crate) name: String,
    /// The path of the object it is a child of.
    pub(crate) parent: String,
}

/// Every object of the application with process `pid`, from its root down.
pub(crate) fn snapshot(session: &Session<'_>, bus: &Bus, pid: i32) -> Result<Vec<Object>> {
    let app = app(session, bus, pid)?;
    let mut walk = Walk {
        session,
        bus,
        found: Vec::new(),
    };
    walk.down((&app, ROOT), "", DEPTH)?;
    Ok(walk.found)
}

/// A walk of an application's tree, and what it found so far.
struct Walk<'a, 'b> {
    session: &'a Session<'b>,
    bus: &'a Bus,
    found: Vec<Object>,
}

impl Walk<'_, '_> {
    /// Notes the object `at`, a child of `parent`, and walks its children `depth` deep.
    fn down(&mut self, at: (&str, &str), parent: &str, depth: usize) -> Result<()> {
        let (app, path) = at;
        let (session, bus) = (self.session, self.bus);
        let role = call(session, bus, app, path, &[ACCESSIBLE, "GetRole"])?;
        self.found.push(Object {
            path: path.to_owned(),
            role: role.pointer("/data/0").and_then(Value::as_u64).unwrap_or(0),
            name: name(session, bus, app, path)?,
            parent: parent.to_owned(),
        });
        if depth == 0 {
            return Ok(());
        }
        let children = call(session, bus, app, path, &[ACCESSIBLE, "GetChildren"])?;
        let children = children
            .pointer("/data/0")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for child in children {
            if let (Some(child_app), Some(child_path)) = (
                child.pointer("/0").and_then(Value::as_str),
                child.pointer("/1").and_then(Value::as_str),
            ) {
                self.down((child_app, child_path), path, depth - 1)?;
            }
        }
        Ok(())
    }
}

/// The objects at a path both snapshots have, whose name or parent's name changed: objects
/// reused for another record.
pub(crate) fn recycled(before: &[Object], after: &[Object]) -> Vec<Value> {
    let named = |objects: &[Object], path: &str| {
        objects
            .iter()
            .find(|object| object.path == path)
            .map(|object| object.name.clone())
    };
    after
        .iter()
        .filter_map(|now| {
            let then = before.iter().find(|then| then.path == now.path)?;
            let parent_then = named(before, &then.parent);
            let parent_now = named(after, &now.parent);
            (then.name != now.name || parent_then != parent_now).then(|| {
                json!({
                    "path": now.path, "role": now.role,
                    "name": [then.name, now.name],
                    "parent_name": [parent_then, parent_now],
                })
            })
        })
        .collect()
}

/// The snapshot as JSON for the run's record.
pub(crate) fn described(objects: &[Object]) -> Value {
    objects
        .iter()
        .map(|object| json!([object.path, object.role, object.name, object.parent]))
        .collect()
}
