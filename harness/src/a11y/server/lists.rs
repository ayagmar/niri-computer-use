//! Lists whose toolkit keeps objects only for the rows in view, and may reuse a row's
//! objects for another record: GTK 4's `Gtk.ListView` and Qt Quick's `ListView` with
//! `reuseItems`. Each fixture's rows hold a label with their record and a `Delete` button
//! that removes the record the row shows when it is pressed. A ref listed for one record's
//! `Delete` must never delete another record: after a record was removed, the list
//! scrolled or it jumped a hundred rows, an old ref is either refused with nothing deleted
//! or deletes its own record. Where the listing marks a ref `unbound`, every press through
//! it is refused with `element_unbound`; where it doesn't, a fresh ref deletes its own
//! record. The marks must be the documented ones: GTK 4's rows have no name unless the app
//! labels them, and Qt Quick's have a row role and a name only when the delegate gives
//! them. GTK 4's old refs must all be refused, as the docs say. The record of the run logs
//! what each toolkit puts on the bus after each change.

use std::fs;
use std::time::Duration;

use serde_json::{Value, json};

use super::actions::activation;
use super::{Checks, element_ref, expect};
use crate::a11y::{Bus, Fixture, rows};
use crate::changes::lists;
use crate::failure::{Context as _, Failure, Result};
use crate::mcp::field;
use crate::session::Session;

/// How long a refused activation has to delete something anyway.
const SETTLE: Duration = Duration::from_millis(500);
const WAIT: Duration = Duration::from_secs(5);
/// The refusals that send nothing to a row: its ref is stale or bound to no record, or it
/// isn't showing.
const REFUSALS: [&str; 3] = ["element_stale", "element_unbound", "element_unmappable"];
/// How many showing rows of each list are deleted twice through one ref.
const TWICE: usize = 3;
/// What an `element_unbound` refusal tells the agent to do instead.
const POINTER: &str = "aim the pointer tools at its pixels";

/// A list fixture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum List {
    /// GTK 4's list views: unnamed rows, records `Record 001` on, and rows labelled with
    /// their record, `Labelled 001` on.
    Gtk4,
    /// Qt Quick's list views with `reuseItems`: rows off the bus, records `Record 001` on;
    /// unnamed list items, `Item 001` on; and list items named after their record, `Named
    /// 001` on.
    Qt,
}

impl List {
    pub(super) const ALL: [Self; 2] = [Self::Gtk4, Self::Qt];

    fn start(self, session: &Session<'_>) -> Result<Fixture> {
        match self {
            Self::Gtk4 => Fixture::start_list(session),
            Self::Qt => Fixture::start_qt_list(session),
        }
    }

    /// The first word of each list's records.
    const fn prefixes(self) -> &'static [&'static str] {
        match self {
            Self::Gtk4 => &["Record", "Labelled"],
            Self::Qt => &["Record", "Item", "Named"],
        }
    }

    /// How the listing marks the first rows' `Delete` refs in the list whose records start
    /// with `prefix`: GTK 4's list items have no name unless the app labels them, and Qt
    /// Quick's rows have no row role unless the delegate gives one (`Item`, `Named`), nor a
    /// name unless it gives that too (`Named`).
    fn mark(self, prefix: &str) -> Option<&'static str> {
        match (self, prefix) {
            (Self::Gtk4, "Record") | (Self::Qt, "Item") => Some("unnamed_row"),
            (Self::Qt, "Record") => Some("repeated"),
            _ => None,
        }
    }

    /// The file the fixture appends each deleted record to.
    const fn deletes(self) -> &'static str {
        match self {
            Self::Gtk4 => "list-deletes",
            Self::Qt => "qt-list-deletes",
        }
    }
}

/// A row's `Delete` button as first listed: its ref, the record its row showed, whether it
/// was showing, and why the listing marked its ref unbound, if it did.
#[derive(Debug, Clone)]
struct Row {
    element: String,
    record: String,
    showing: bool,
    unbound: Option<String>,
}

/// The rows of one list the checks go through: the first, third, fifth and seventh.
struct Picked {
    first: Row,
    third: Row,
    fifth: Row,
    seventh: Row,
}

/// The list fixture's window, its process and its kind.
struct Shown {
    window: u64,
    pid: i32,
    list: List,
}

impl Checks<'_, '_> {
    /// Lists the first rows' `Delete` buttons of each list and deletes its third record
    /// through its ref, refused where the listing marks it unbound; then tries that ref
    /// again, the fifth record's after the rows below moved up, the first's after a short
    /// scroll and the seventh's after a jump of a hundred rows. Then, a step further, a few
    /// showing rows are deleted twice through one ref.
    pub(super) fn recycled_rows(&mut self, bus: &Bus, list: List) -> Result<()> {
        let fixture = list.start(self.session)?;
        let shown = Shown {
            window: fixture.window(self.session, bus)?.id,
            pid: fixture.pid(),
            list,
        };
        self.call("focus", json!({"window_id": shown.window}))?;
        let listing = self.list(shown.window, None, None)?;
        let picked = list
            .prefixes()
            .iter()
            .map(|prefix| picked(&listing, list, prefix))
            .collect::<Result<Vec<_>>>()?;
        let start = rows::snapshot(self.session, bus, shown.pid)?;
        self.session.log(&format!(
            "M9b: {list:?} list: listed (role, name) {}; bus (path, role, name, parent) {}",
            listed(&listing),
            rows::described(&start),
        ))?;
        let mut old = Vec::new();
        for rows in &picked {
            let deleted = self.delete_row(&shown, &rows.third)?;
            expect(
                (field(&deleted, "/accepted") == true) == rows.third.unbound.is_none(),
                "a fresh ref's Delete accepted unless the listing marked it unbound",
                &deleted,
            )?;
        }
        let removed = self.looked(bus, &shown, "the third records' Delete", &start)?;
        for rows in &picked {
            old.push(self.delete_row(&shown, &rows.third)?);
            old.push(self.delete_row(&shown, &rows.fifth)?);
        }
        self.press(&shown, "Step")?;
        let stepped = self.looked(bus, &shown, "Step", &removed)?;
        for rows in &picked {
            old.push(self.delete_row(&shown, &rows.first)?);
        }
        self.press(&shown, "Jump")?;
        let jumped = self.looked(bus, &shown, "Jump", &stepped)?;
        for rows in &picked {
            old.push(self.delete_row(&shown, &rows.seventh)?);
        }
        self.press(&shown, "Step")?;
        self.looked(bus, &shown, "Step after Jump", &jumped)?;
        old.extend(self.deleted_twice(bus, &shown)?);
        if list == List::Gtk4 {
            all_refused(&old)?;
        }
        fixture.stop().map(drop)
    }

    /// The first `TWICE` showing rows' `Delete` of each list, listed afresh, then those
    /// that deleted their record through the same ref at once, and again after a step. Qt
    /// Quick keeps a deleted row's objects on the bus, shown and named as before, until it
    /// reuses them for a record scrolling in. Logs the bus around the first row's. Returns
    /// the later tries' results.
    fn deleted_twice(&mut self, bus: &Bus, shown: &Shown) -> Result<Vec<Value>> {
        let listing = self.list(shown.window, None, None)?;
        let mut showing = Vec::new();
        for prefix in shown.list.prefixes() {
            let rows = deletes(&listing, prefix)?;
            showing.extend(rows.into_iter().filter(|row| row.showing).take(TWICE));
        }
        let mut deleted = Vec::new();
        let mut again = Vec::new();
        for (count, row) in showing.iter().enumerate() {
            let before = (count == 0).then(|| rows::snapshot(self.session, bus, shown.pid));
            let fresh = self.delete_row(shown, row)?;
            if let Some(before) = before {
                let what = format!("{}'s Delete", row.record);
                self.looked(bus, shown, &what, &before?)?;
            }
            expect(
                (field(&fresh, "/accepted") == true) == row.unbound.is_none(),
                "a fresh ref's Delete accepted unless the listing marked it unbound",
                &fresh,
            )?;
            if row.unbound.is_none() {
                again.push(self.delete_row(shown, row)?);
                deleted.push(row);
            }
        }
        self.press(shown, "Step")?;
        for row in deleted {
            again.push(self.delete_row(shown, row)?);
        }
        Ok(again)
    }

    /// Activates the list window's button named `name`.
    fn press(&mut self, shown: &Shown, name: &str) -> Result<()> {
        let element = self.named(shown.window, Some("button"), name)?;
        self.call("activate_element", activation(&element, shown.window))
            .map(drop)
    }

    /// The bus after `what`, logged with how many of the paths `before` had it kept and
    /// which of those now have another name or a parent with another name.
    fn looked(
        &mut self,
        bus: &Bus,
        shown: &Shown,
        what: &str,
        before: &[rows::Object],
    ) -> Result<Vec<rows::Object>> {
        let after = rows::snapshot(self.session, bus, shown.pid)?;
        let kept = before
            .iter()
            .filter(|then| after.iter().any(|now| now.path == then.path))
            .count();
        self.session.log(&format!(
            "M9b: {:?} list: after {what}, {kept} of {} paths kept, reused for another name {}; bus {}",
            shown.list,
            before.len(),
            Value::from(rows::recycled(before, &after)),
            rows::described(&after)
        ))?;
        Ok(after)
    }

    /// Activates a row's `Delete` through the ref first listed for it, and returns the
    /// result. Whatever it was, the fixture must have deleted the row's own record or
    /// nothing: nothing when refused with one of `REFUSALS`. A ref the listing marked
    /// unbound must be refused with `element_unbound`, pointing at the pointer tools.
    fn delete_row(&mut self, shown: &Shown, row: &Row) -> Result<Value> {
        let before = self.deleted(shown.list)?;
        let result = self.client.call(
            self.session,
            "activate_element",
            activation(&row.element, shown.window),
        )?;
        let result = field(&result, "/structuredContent").clone();
        let refused = field(&result, "/error");
        expect(
            row.unbound.is_none() || unbound(&result),
            "a ref the listing marked unbound refused with element_unbound",
            &result,
        )?;
        let what = if refused.is_null() {
            let deleted = self.newly_deleted(shown.list, &before)?;
            expect(
                deleted == [row.record.as_str()],
                &format!("an accepted Delete for {} to delete it alone", row.record),
                &json!({"result": result, "deleted": deleted}),
            )?;
            format!("accepted, deleted {deleted:?}")
        } else {
            expect(
                REFUSALS.iter().any(|name| refused == name),
                "a refusal that sends nothing",
                &result,
            )?;
            self.nothing_deleted(shown.list, &before)?;
            "refused, nothing deleted".to_owned()
        };
        self.session.log(&format!(
            "M9b: {:?} list: {}'s Delete through its first ref {what}: {result}",
            shown.list, row.record
        ))?;
        Ok(result)
    }

    /// The records deleted since `before`, once there are some.
    fn newly_deleted(&mut self, list: List, before: &[String]) -> Result<Vec<String>> {
        let what = format!("a record deleted after {before:?}");
        let session = &mut *self.session;
        let now = session.wait_until("m9b-deleted", &what, WAIT, |session| {
            Ok(read_deletes(session, list)?.filter(|now| now.len() > before.len()))
        })?;
        Ok(now.get(before.len()..).unwrap_or_default().to_vec())
    }

    /// The fixture deletes nothing more for a while.
    fn nothing_deleted(&self, list: List, before: &[String]) -> Result<()> {
        let session = &*self.session;
        session.still_absent("m9b-nothing-deleted", SETTLE, || {
            Ok(read_deletes(session, list)?.is_some_and(|now| now != before))
        })
    }

    /// The records deleted so far, once the fixture wrote them whole.
    fn deleted(&mut self, list: List) -> Result<Vec<String>> {
        let what = format!("{} written", list.deletes());
        self.session
            .wait_until("m9b-deletes", &what, WAIT, |session| {
                read_deletes(session, list)
            })
    }
}

/// The records `list`'s fixture deleted, or `None` while the file is missing or being
/// written: Qt's `XMLHttpRequest` `PUT` empties it first.
fn read_deletes(session: &Session<'_>, list: List) -> Result<Option<Vec<String>>> {
    let path = session.test_dir().root().join(list.deletes());
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(Failure::new(format!("read {}: {error}", path.display()))),
    };
    if text.trim().is_empty() {
        return Ok(None);
    }
    serde_json::from_str(&text)
        .map(Some)
        .context(format!("parse {}", path.display()))
}

/// `result` refuses a ref as bound to no record, pointing at the pointer tools.
fn unbound(result: &Value) -> bool {
    field(result, "/error") == "element_unbound"
        && field(result, "/detail")
            .as_str()
            .is_some_and(|detail| detail.contains(POINTER))
}

/// Every result in `old` is a refusal: `element_stale`, or `element_unbound` for a ref the
/// listing marked.
fn all_refused(old: &[Value]) -> Result<()> {
    let refused = old.iter().all(|result| {
        ["element_stale", "element_unbound"]
            .iter()
            .any(|name| field(result, "/error") == name)
    });
    expect(
        refused,
        "every old Delete ref refused once the list changed",
        &Value::from(old.to_vec()),
    )
}

/// The first, third, fifth and seventh `Delete` buttons of the listing among the rows
/// whose record starts with `prefix`, each marked as `list` marks them.
fn picked(listing: &Value, list: List, prefix: &str) -> Result<Picked> {
    let rows = deletes(listing, prefix)?;
    let [first, _, third, _, fifth, _, seventh, ..] = rows.as_slice() else {
        return Err(Failure::new(format!(
            "M9b: expected seven {prefix} rows with a Delete button in {listing}"
        )));
    };
    let mark = list.mark(prefix);
    expect(
        [first, third, fifth, seventh]
            .iter()
            .all(|row| row.unbound.as_deref() == mark),
        &format!("the first {prefix} rows' Delete marked {mark:?}"),
        listing,
    )?;
    Ok(Picked {
        first: first.clone(),
        third: third.clone(),
        fifth: fifth.clone(),
        seventh: seventh.clone(),
    })
}

/// Each `Delete` button of the listing, with the record of the label starting with
/// `prefix` listed before it.
fn deletes(listing: &Value, prefix: &str) -> Result<Vec<Row>> {
    let mut record = None;
    let mut rows = Vec::new();
    for element in field(listing, "/elements").as_array().into_iter().flatten() {
        let name = field(element, "/name").as_str().unwrap_or_default();
        match field(element, "/role").as_str() {
            Some("label") if name.starts_with(&format!("{prefix} ")) => {
                record = Some(name.to_owned());
            }
            Some("label") => record = None,
            Some("button") if name == "Delete" => {
                if let Some(record) = record.take() {
                    rows.push(Row {
                        element: element_ref(element)?,
                        record,
                        showing: lists(field(element, "/states"), &json!("showing")),
                        unbound: field(element, "/unbound").as_str().map(str::to_owned),
                    });
                }
            }
            _ => {}
        }
    }
    Ok(rows)
}

/// The listing's (role, name) pairs.
fn listed(listing: &Value) -> Value {
    field(listing, "/elements")
        .as_array()
        .into_iter()
        .flatten()
        .map(|element| json!([field(element, "/role"), field(element, "/name")]))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_delete_button_goes_with_the_record_listed_before_it_in_its_own_list() {
        let element = |role: &str, name: &str, id: &str| json!({"role": role, "name": name, "element_ref": id});
        let listing = json!({"elements": [
            element("button", "Step", "e-1"),
            element("label", "Record 001", "e-2"),
            element("button", "Delete", "e-3"),
            element("label", "Named 001", "e-4"),
            element("button", "Delete", "e-5"),
            element("label", "Record 002", "e-6"),
            element("button", "Delete", "e-7"),
        ]});
        let found = |prefix| {
            deletes(&listing, prefix)
                .unwrap()
                .into_iter()
                .map(|row| (row.element, row.record))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            found("Record"),
            [
                ("e-3".to_owned(), "Record 001".to_owned()),
                ("e-7".to_owned(), "Record 002".to_owned())
            ]
        );
        assert_eq!(found("Named"), [("e-5".to_owned(), "Named 001".to_owned())]);
    }
}
