//! The audit log: one JSON line per tool call at
//! `$XDG_STATE_HOME/niri-computer-use/audit.jsonl`, in a `0700` directory and a `0600` file.
//! It records argument metadata and the outcome, never typed text, clipboard contents,
//! screenshot data or window titles. A string the agent chose is logged as it is only once
//! it names something the server knows: a preset, a panel, a role, a ref in the form refs
//! are issued in, or key combinations XKB knows. Anything else is a category and its
//! length, and text, paths, app ids and output names only ever their length or kind. The
//! passthroughs log metadata too: `niri_action` its action's name, field names, numbers and
//! booleans with every string as its length in bytes, and `noctalia` only its argument
//! count and byte lengths.

use std::fs::{DirBuilder, OpenOptions};
use std::io::Write as _;
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use chrono::{SecondsFormat, Utc};
use niri_ipc::Action;
use rmcp::ErrorData;
use rmcp::model::CallToolResult;
use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::error::CANCELLED;
use crate::input::keyboard::Expect;
use crate::input::keymap;

/// Where the log goes, and the last write error, which `status` reports.
#[derive(Debug, Clone)]
pub(crate) struct Audit {
    path: Option<PathBuf>,
    last_error: Arc<Mutex<Option<String>>>,
}

/// One tool call, as written to the log.
#[derive(Debug, Serialize)]
struct Record<'a> {
    ts: &'a str,
    /// The MCP client's name and this server's PID, such as `claude-code/4711`.
    session: &'a str,
    instance: Option<&'a str>,
    tool: &'a str,
    /// Metadata only: sizes, targets and IDs, never contents.
    args: &'a Value,
    /// For action tools, from their result; null for read-only tools and for failures.
    accepted: Option<bool>,
    observed: Option<&'a str>,
    /// For action tools that reported changes, how many of each kind, from their result.
    #[serde(skip_serializing_if = "Option::is_none")]
    changes: Option<Value>,
    error: Option<String>,
    /// For a failure after some of its input went out, or may have: true, from its result.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    may_have_sent: bool,
    duration_ms: u128,
}

/// A tool call in progress: which tool, whether it acts on the desktop, and when it
/// started.
#[derive(Debug)]
pub(crate) struct Call<'a> {
    tool: &'a str,
    action: bool,
    ts: String,
    started: Instant,
}

impl<'a> Call<'a> {
    pub(crate) fn start(tool: &'a str) -> Self {
        Self {
            tool,
            action: false,
            ts: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
            started: Instant::now(),
        }
    }

    /// An action tool's call, whose result says what was accepted and observed.
    pub(crate) fn action(tool: &'a str) -> Self {
        Self {
            action: true,
            ..Self::start(tool)
        }
    }
}

/// Who made a call and on which compositor instance.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Caller<'a> {
    pub(crate) session: &'a str,
    pub(crate) instance: Option<&'a str>,
}

/// What `status` reports about the log.
#[derive(Debug, Serialize)]
pub(crate) struct AuditStatus {
    path: Option<PathBuf>,
    last_error: Option<String>,
}

impl Audit {
    /// `None` when neither `XDG_STATE_HOME` nor `HOME` is set; calls then go unlogged and
    /// `status` says so.
    pub(crate) fn new(state_dir: Option<PathBuf>) -> Self {
        let last_error = state_dir
            .is_none()
            .then(|| "neither XDG_STATE_HOME nor HOME is set".to_owned());
        Self {
            path: state_dir.map(|dir| dir.join("niri-computer-use").join("audit.jsonl")),
            last_error: Arc::new(Mutex::new(last_error)),
        }
    }

    pub(crate) fn status(&self) -> AuditStatus {
        AuditStatus {
            path: self.path.clone(),
            last_error: self.last().clone(),
        }
    }

    /// Logs a finished call: `args` is its metadata, and the outcome comes from `result`.
    pub(crate) fn finish(
        &self,
        call: &Call<'_>,
        caller: Caller<'_>,
        mut args: Value,
        result: &Result<CallToolResult, ErrorData>,
    ) {
        matched(&mut args, result);
        let (accepted, observed, changes) = if call.action {
            effect(result)
        } else {
            (None, None, None)
        };
        self.write(&Record {
            ts: &call.ts,
            session: caller.session,
            instance: caller.instance,
            tool: call.tool,
            args: &args,
            accepted,
            observed,
            changes,
            error: outcome(result),
            may_have_sent: may_have_sent(result),
            duration_ms: call.started.elapsed().as_millis(),
        });
    }

    /// Appends one line. A failure is kept for `status` and doesn't fail the tool call.
    fn write(&self, record: &Record<'_>) {
        let Some(path) = &self.path else {
            return;
        };
        let result = serde_json::to_vec(record)
            .map_err(|error| error.to_string())
            .and_then(|line| {
                append(path, &line).map_err(|error| format!("{}: {error}", path.display()))
            });
        if let Err(error) = result {
            *self.last() = Some(error);
        }
    }

    fn last(&self) -> std::sync::MutexGuard<'_, Option<String>> {
        self.last_error
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

fn append(path: &std::path::Path, line: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    }
    let mut file = OpenOptions::new()
        .append(true)
        .create(true)
        .mode(0o600)
        .open(path)?;
    // One write per line, so concurrent servers appending to the same file don't
    // interleave within a line.
    file.write_all(&[line, b"\n"].concat())
}

/// `niri_action`'s action for the log: its name and field names, as niri-ipc spells them,
/// with numbers, booleans and nulls as they are and every string as its length in bytes,
/// so a command or a workspace name never reaches the log.
pub(crate) fn action(action: &Action) -> Value {
    serde_json::to_value(action).map_or(Value::Null, |json| lengths(&json))
}

/// JSON that isn't a niri action, for the log: a fixed category and its size, never its
/// keys, which are the agent's text.
pub(crate) fn invalid_action(action: &Map<String, Value>) -> Value {
    json!({
        "category": "invalid_action",
        "keys": action.len(),
        "bytes": serde_json::to_string(action).map_or(0, |text| text.len()),
    })
}

/// A string becomes `{"bytes": n}` and a list of strings `{"count": n, "bytes": [...]}`.
fn lengths(value: &Value) -> Value {
    match value {
        Value::String(text) => json!({ "bytes": text.len() }),
        Value::Array(items) if items.iter().all(Value::is_string) => json!({
            "count": items.len(),
            "bytes": items
                .iter()
                .filter_map(Value::as_str)
                .map(str::len)
                .collect::<Vec<_>>(),
        }),
        Value::Array(items) => items.iter().map(lengths).collect(),
        Value::Object(fields) => fields
            .iter()
            .map(|(key, field)| (key.clone(), lengths(field)))
            .collect(),
        Value::Null | Value::Bool(_) | Value::Number(_) => value.clone(),
    }
}

/// A name the agent gave, as it is when the server knows it, such as a preset, a panel or
/// a role; otherwise `category` and its length in bytes, since it is the agent's text.
pub(crate) fn known(name: &str, is_known: bool, category: &str) -> Value {
    if is_known {
        return Value::from(name);
    }
    json!({ "category": category, "bytes": name.len() })
}

/// A screenshot or element ref, as it is when it has the form refs are issued in.
pub(crate) fn ref_id(id: &str) -> Value {
    known(id, crate::refs::well_formed(id), "invalid_ref")
}

/// `key`'s combinations as they are when XKB knows every one, otherwise only how many.
pub(crate) fn keys(keys: &[String]) -> Value {
    if keys.iter().all(|combo| keymap::known_combo(combo)) {
        return json!(keys);
    }
    json!({ "category": "invalid_keys", "count": keys.len() })
}

/// A pointer tool's held modifiers as they are when each is one a combination may hold,
/// otherwise only how many.
pub(crate) fn modifiers(keys: &[String]) -> Value {
    if keys
        .iter()
        .all(|key| keymap::known_combo(&format!("{key}+a")))
    {
        return json!(keys);
    }
    json!({ "category": "invalid_keys", "count": keys.len() })
}

/// `expect` for the log: a window id or `none` as given, and an app id only as its length
/// in characters, to which `finish` adds whether focus matched it.
pub(crate) fn expect(expect: &Expect) -> Value {
    match expect {
        Expect::Window(id) => json!({ "window_id": id }),
        Expect::App(app_id) => json!({ "app_id_len": app_id.chars().count() }),
        Expect::Unchecked => json!("none"),
    }
}

/// Adds `matched` to an `expect` logged as an app id's length: true when the result says
/// focus matched it, false on `focus_mismatch`, and null when the call ended before the
/// check or without saying.
fn matched(args: &mut Value, result: &Result<CallToolResult, ErrorData>) {
    let Some(expect) = args
        .get_mut("expect")
        .and_then(Value::as_object_mut)
        .filter(|expect| expect.contains_key("app_id_len"))
    else {
        return;
    };
    let focus = result
        .as_ref()
        .ok()
        .filter(|result| result.is_error != Some(true))
        .and_then(|result| result.structured_content.as_ref())
        .and_then(|content| content.get("focus"))
        .and_then(Value::as_str);
    let matched = match (outcome(result).as_deref(), focus) {
        (Some("focus_mismatch"), _) => Value::Bool(false),
        (None, Some("matched")) => Value::Bool(true),
        _ => Value::Null,
    };
    expect.insert("matched".to_owned(), matched);
}

/// `noctalia`'s arguments for the log: how many, and each one's length in bytes.
pub(crate) fn noctalia(args: &[String]) -> Value {
    json!({
        "count": args.len(),
        "bytes": args.iter().map(String::len).collect::<Vec<_>>(),
    })
}

/// The outcome's error name, read from the tool's result and never from its content: a
/// stable name for a failure, `invalid_arguments` for an argument mistake, and `cancelled`
/// or `internal` when the call didn't produce a result.
fn outcome(result: &Result<CallToolResult, ErrorData>) -> Option<String> {
    let result = match result {
        Ok(result) => result,
        Err(error) if error.message == CANCELLED => return Some("cancelled".to_owned()),
        Err(_) => return Some("internal".to_owned()),
    };
    if result.is_error != Some(true) {
        return None;
    }
    let name = result
        .structured_content
        .as_ref()
        .and_then(|content| content.get("error"))
        .and_then(Value::as_str);
    Some(name.unwrap_or("invalid_arguments").to_owned())
}

/// Whether a failure says some of its input went out, or may have: only that boolean of
/// its result is read.
fn may_have_sent(result: &Result<CallToolResult, ErrorData>) -> bool {
    result
        .as_ref()
        .ok()
        .filter(|result| result.is_error == Some(true))
        .and_then(|result| result.structured_content.as_ref())
        .and_then(|content| content.get("may_have_sent"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// An action's `accepted` and `observed` fields, which hold only booleans and outcome
/// names, and its change report as counts. Failures have none of them.
fn effect(
    result: &Result<CallToolResult, ErrorData>,
) -> (Option<bool>, Option<&str>, Option<Value>) {
    let Some(content) = result
        .as_ref()
        .ok()
        .filter(|result| result.is_error != Some(true))
        .and_then(|result| result.structured_content.as_ref())
    else {
        return (None, None, None);
    };
    (
        content.get("accepted").and_then(Value::as_bool),
        content.get("observed").and_then(Value::as_str),
        content.get("changes").map(counted),
    )
}

/// A change report as counts: windows opened, closed and changed, whether focus and the
/// focused workspace moved, and the element changes' sizes or why they are unknown, which
/// is an error name. Never a title, an app id or an element's name.
fn counted(changes: &Value) -> Value {
    let count =
        |of: &Value, field: &str| of.get(field).and_then(Value::as_array).map_or(0, Vec::len);
    let number = |of: &Value, field: &str| of.get(field).and_then(Value::as_u64).unwrap_or(0);
    let elements = changes.get("elements").map(|elements| {
        if let Some(unknown) = elements.get("unknown") {
            return json!({ "unknown": unknown.as_str() });
        }
        json!({
            "added": count(elements, "added"),
            "removed": count(elements, "removed"),
            "changed": count(elements, "changed"),
            "truncated": number(elements, "truncated"),
        })
    });
    json!({
        "opened": count(changes, "opened"),
        "closed": count(changes, "closed"),
        "windows": count(changes, "windows"),
        "focus": changes.get("focus").is_some(),
        "workspace": changes.get("workspace").is_some(),
        "truncated": number(changes, "truncated"),
        "lost": changes.get("lost").is_some(),
        "elements": elements,
    })
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;

    use rmcp::model::ContentBlock;

    use super::*;
    use crate::error::{ErrorName, ToolError};

    fn record<'a>(tool: &'a str, args: &'a Value, error: Option<String>) -> Record<'a> {
        Record {
            ts: "2026-10-08T00:00:00.000Z",
            session: "smoke/1",
            instance: Some("niri.wayland-1.1.sock"),
            tool,
            args,
            accepted: None,
            observed: None,
            changes: None,
            error,
            may_have_sent: false,
            duration_ms: 12,
        }
    }

    #[test]
    fn appends_one_private_line_per_call() {
        let dir = crate::test_support::fresh_dir("audit");
        let audit = Audit::new(Some(dir.join("state")));
        let args = serde_json::json!({"target": "focused_output"});
        audit.write(&record("screenshot", &args, None));
        audit.write(&record(
            "desktop_state",
            &args,
            Some("niri_unavailable".to_owned()),
        ));
        let path = dir.join("state/niri-computer-use/audit.jsonl");
        let lines: Vec<Value> = std::fs::read_to_string(&path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(
            lines[0],
            serde_json::json!({
                "ts": "2026-10-08T00:00:00.000Z", "session": "smoke/1",
                "instance": "niri.wayland-1.1.sock", "tool": "screenshot",
                "args": {"target": "focused_output"}, "accepted": null, "observed": null,
                "error": null, "duration_ms": 12
            })
        );
        assert_eq!(lines[1]["error"], "niri_unavailable");
        let mode = |file: &std::path::Path| file.metadata().unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&path), 0o600);
        assert_eq!(mode(path.parent().unwrap()), 0o700);
        assert_eq!(audit.status().last_error, None);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_failed_write_is_kept_for_status() {
        let dir = crate::test_support::fresh_dir("audit-fail");
        // A file where the log's directory should be.
        std::fs::write(dir.join("niri-computer-use"), "").unwrap();
        let audit = Audit::new(Some(dir.clone()));
        audit.write(&record("status", &Value::Null, None));
        assert!(audit.status().last_error.is_some());
        assert_eq!(
            Audit::new(None).status().last_error.as_deref(),
            Some("neither XDG_STATE_HOME nor HOME is set")
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_outcome_never_reads_content() {
        let secret = CallToolResult::structured(serde_json::json!({"text": "hunter2"}));
        assert_eq!(outcome(&Ok(secret)), None);
        let failed = ToolError::new(ErrorName::UpstreamError, "hunter2").into_result();
        assert_eq!(outcome(&Ok(failed)).as_deref(), Some("upstream_error"));
        let mistake = CallToolResult::error(vec![ContentBlock::text("invalid arguments: x")]);
        assert_eq!(outcome(&Ok(mistake)).as_deref(), Some("invalid_arguments"));
        let cancelled = ErrorData::internal_error(CANCELLED, None);
        assert_eq!(outcome(&Err(cancelled)).as_deref(), Some("cancelled"));
        let other = ErrorData::internal_error("serialize", None);
        assert_eq!(outcome(&Err(other)).as_deref(), Some("internal"));
        let ts = Call::start("status").ts;
        assert!(ts.ends_with('Z') && ts.len() == 24, "{ts}");
    }

    /// A stale lock read refuses after the `expect` check passed, and its detail can name
    /// the window; the line keeps only the error's name and the bounded arguments.
    #[test]
    fn a_stale_lock_read_is_logged_by_name_with_bounded_arguments() {
        let dir = crate::test_support::fresh_dir("audit-stale");
        let audit = Audit::new(Some(dir.clone()));
        let caller = Caller {
            session: "smoke/1",
            instance: None,
        };
        let marker = "MARKER-SENTINEL";
        let args = json!({
            "keys": keys(&[format!("ctrl+{marker}")]),
            "expect": expect(&Expect::App(marker.to_owned())),
        });
        let stale = ToolError::new(ErrorName::LockStale, format!("read before {marker}"));
        audit.finish(&Call::action("key"), caller, args, &Ok(stale.into_result()));
        let text = std::fs::read_to_string(dir.join("niri-computer-use/audit.jsonl")).unwrap();
        assert!(!text.contains(marker), "{text}");
        let line: Value = serde_json::from_str(text.trim_end()).unwrap();
        assert_eq!(line["error"], "lock_stale");
        assert_eq!(
            line["args"],
            json!({
                "keys": {"category": "invalid_keys", "count": 1},
                "expect": {"app_id_len": 15, "matched": null}
            })
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_niri_action_is_logged_as_its_names_numbers_and_byte_lengths() {
        let action = |json: Value| serde_json::from_value::<Action>(json).unwrap();
        assert_eq!(
            super::action(&action(
                json!({"Spawn": {"command": ["foot", "-e", "sécret"]}})
            )),
            json!({"Spawn": {"command": {"count": 3, "bytes": [4, 2, 7]}}})
        );
        assert_eq!(
            super::action(&action(json!({"SpawnSh": {"command": "echo hunter2"}}))),
            json!({"SpawnSh": {"command": {"bytes": 12}}})
        );
        assert_eq!(
            super::action(&action(
                json!({"SetWindowWidth": {"id": 12, "change": {"SetFixed": 1600}}})
            )),
            json!({"SetWindowWidth": {"id": 12, "change": {"SetFixed": 1600}}})
        );
        assert_eq!(
            super::action(&action(json!({"MoveWindowToWorkspace": {
                "window_id": null, "reference": {"Name": "hunter2"}, "focus": true
            }}))),
            json!({"MoveWindowToWorkspace": {
                "window_id": null, "reference": {"Name": {"bytes": 7}}, "focus": true
            }})
        );
    }

    #[test]
    fn json_that_is_no_action_is_logged_as_a_category_and_its_size() {
        let garbled: Map<String, Value> =
            serde_json::from_value(json!({"hunter2": {"x": 1}})).unwrap();
        assert_eq!(
            invalid_action(&garbled),
            json!({"category": "invalid_action", "keys": 1, "bytes": 19})
        );
    }

    #[test]
    fn noctalia_logs_only_counts_and_byte_lengths() {
        let args = ["panel-open".to_owned(), "hunter-two".to_owned()];
        assert_eq!(noctalia(&args), json!({"count": 2, "bytes": [10, 10]}));
    }

    #[test]
    fn actions_log_what_was_accepted_and_observed() {
        let dir = crate::test_support::fresh_dir("audit-action");
        let audit = Audit::new(Some(dir.clone()));
        let caller = Caller {
            session: "smoke/1",
            instance: None,
        };
        let args = serde_json::json!({"id": 4});
        let done = CallToolResult::structured(serde_json::json!({
            "accepted": true, "observed": "focused", "focused_window": 4
        }));
        audit.finish(
            &Call::action("focus"),
            caller,
            args.clone(),
            &Ok(done.clone()),
        );
        audit.finish(
            &Call::start("desktop_state"),
            caller,
            args.clone(),
            &Ok(done),
        );
        let refused = ToolError::new(ErrorName::LeaseRequired, "no").into_result();
        audit.finish(&Call::action("focus"), caller, args, &Ok(refused));
        let lines: Vec<Value> = std::fs::read_to_string(dir.join("niri-computer-use/audit.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let effects: Vec<Value> = lines
            .iter()
            .map(|line| serde_json::json!([line["accepted"], line["observed"], line["error"]]))
            .collect();
        assert_eq!(
            effects,
            [
                serde_json::json!([true, "focused", null]),
                serde_json::json!([null, null, null]),
                serde_json::json!([null, null, "lease_required"]),
            ]
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_change_report_is_logged_as_counts_without_titles_or_names() {
        let dir = crate::test_support::fresh_dir("audit-changes");
        let audit = Audit::new(Some(dir.clone()));
        let caller = Caller {
            session: "smoke/1",
            instance: None,
        };
        let reported = CallToolResult::structured(serde_json::json!({
            "accepted": true, "observed": "present", "focused_window": 4,
            "changes": {
                "opened": [{"id": 4, "app_id": "APP-SENTINEL", "workspace_id": 1, "title": "TITLE-SENTINEL"}],
                "focus": {"from": 1, "to": 4},
                "windows": [{"id": 1, "title": "TITLE-SENTINEL"}],
                "elements": {
                    "window_id": 1,
                    "added": [{"element_ref": "elem-1", "role": "label", "name": "NAME-SENTINEL", "states": []}],
                    "changed": [{"element_ref": "elem-2", "role": "button", "name": "NAME-SENTINEL", "was": "NAME-SENTINEL"}],
                    "truncated": 3
                }
            }
        }));
        let unknown = CallToolResult::structured(serde_json::json!({
            "accepted": true, "observed": "sent",
            "changes": {"elements": {"window_id": 1, "unknown": "not_accessible", "detail": "DETAIL-SENTINEL"}}
        }));
        let args = serde_json::json!({});
        audit.finish(
            &Call::action("activate_element"),
            caller,
            args.clone(),
            &Ok(reported),
        );
        audit.finish(&Call::action("key"), caller, args, &Ok(unknown));
        let text = std::fs::read_to_string(dir.join("niri-computer-use/audit.jsonl")).unwrap();
        assert!(!text.contains("SENTINEL"), "{text}");
        let lines: Vec<Value> = text
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(
            lines[0]["changes"],
            serde_json::json!({
                "opened": 1, "closed": 0, "windows": 1, "focus": true, "workspace": false,
                "truncated": 0, "lost": false,
                "elements": {"added": 1, "removed": 0, "changed": 1, "truncated": 3}
            })
        );
        assert_eq!(
            lines[1]["changes"]["elements"],
            serde_json::json!({"unknown": "not_accessible"})
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
