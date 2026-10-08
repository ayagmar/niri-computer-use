//! The audit log: one JSON line per tool call at
//! `$XDG_STATE_HOME/niri-computer-use/audit.jsonl`, in a `0700` directory and a `0600` file.
//! It records argument metadata and the outcome, never typed text, clipboard contents,
//! screenshot data or window titles.

use std::fs::{DirBuilder, OpenOptions};
use std::io::Write as _;
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use chrono::{SecondsFormat, Utc};
use rmcp::ErrorData;
use rmcp::model::CallToolResult;
use serde::Serialize;
use serde_json::Value;

use crate::error::CANCELLED;

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
    /// For action tools, which don't exist yet; null for read-only tools.
    accepted: Option<bool>,
    observed: Option<&'a str>,
    error: Option<String>,
    duration_ms: u128,
}

/// A tool call in progress: which tool, and when it started.
#[derive(Debug)]
pub(crate) struct Call<'a> {
    tool: &'a str,
    ts: String,
    started: Instant,
}

impl<'a> Call<'a> {
    pub(crate) fn start(tool: &'a str) -> Self {
        Self {
            tool,
            ts: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
            started: Instant::now(),
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
        args: &Value,
        result: &Result<CallToolResult, ErrorData>,
    ) {
        self.write(&Record {
            ts: &call.ts,
            session: caller.session,
            instance: caller.instance,
            tool: call.tool,
            args,
            accepted: None,
            observed: None,
            error: outcome(result),
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
            error,
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
            "outputs",
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
}
