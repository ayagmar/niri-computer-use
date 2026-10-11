//! What the nested suites check of an action's `changes`: each report is logged with its
//! size and whether it was checked to tell what the action did, for `docs/results/`.

use serde_json::Value;

use crate::failure::{Failure, Result};
use crate::mcp::field;
use crate::session::Session;

/// `outcome`'s report, which must tell what `what` did: `holds` of its `changes`.
pub(crate) fn told(
    session: &mut Session<'_>,
    what: &str,
    outcome: &Value,
    holds: impl FnOnce(&Value) -> bool,
) -> Result<()> {
    let changes = field(outcome, "/changes");
    if !holds(changes) {
        return Err(Failure::new(format!(
            "changes: expected {what} in the report; saw {outcome}"
        )));
    }
    log(session, what, true, changes)
}

/// `outcome`'s report, logged unchecked: where it can't tell what the action did, or
/// tells it only when the app is quick enough.
pub(crate) fn logged(session: &mut Session<'_>, what: &str, outcome: &Value) -> Result<()> {
    log(session, what, false, field(outcome, "/changes"))
}

fn log(session: &mut Session<'_>, what: &str, checked: bool, changes: &Value) -> Result<()> {
    let bytes = if changes.is_null() {
        0
    } else {
        changes.to_string().len()
    };
    session.log(&format!(
        "changes: {what}: checked {checked}, {bytes} bytes: {changes}"
    ))
}

/// Whether `list`, an array of windows or of ids, has the window `id`.
pub(crate) fn lists(list: &Value, id: &Value) -> bool {
    list.as_array().is_some_and(|items| {
        items
            .iter()
            .any(|item| item == id || field(item, "/id") == id)
    })
}
