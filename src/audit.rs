//! The audit log: one JSON line per tool call at
//! `$XDG_STATE_HOME/niri-computer-use/audit.jsonl`, in a `0700` directory and a `0600` file.
//! It records argument metadata and the outcome, never typed text, clipboard contents,
//! screenshot data or window titles. A string the agent chose is logged as it is only once
//! it names something the server knows: a preset, a panel, a role, a ref in the form refs
//! are issued in, or key combinations XKB knows. Anything else is a category and its
//! length, and text, paths, app ids and output names only ever their length or kind. The
//! passthroughs log metadata too: `niri_action` its action's name, field names, numbers and
//! booleans with every string as its length in bytes, and `noctalia` its argument count
//! and byte lengths, plus its command, that command's description and a plugin id only as
//! Noctalia's own lists name them (`noctalia::Named`).
//!
//! Once the log reaches `ROTATE_AT`, the next write first moves it aside to
//! `audit.jsonl.1`, shifting older ones up to `audit.jsonl.3` and dropping the oldest
//! (`rotate`).
//!
//! Lines are written by a thread of the log's own, in the order calls finish, so a file
//! system that stalls, as an NFS or FUSE state directory can, never holds up the runtime.
//! A call waits at most `WRITE_LIMIT` for its line, and one that finds `QUEUE` lines
//! waiting goes unlogged; both say so in `status.audit.last_error`.

use std::borrow::Cow;
use std::fs::{DirBuilder, OpenOptions, TryLockError};
use std::io::Write as _;
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{SyncSender, TrySendError};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use chrono::{SecondsFormat, Utc};
use niri_ipc::Action;
use rmcp::ErrorData;
use rmcp::model::CallToolResult;
use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::error::CANCELLED;
use crate::input::keyboard::Expect;
use crate::input::keymap;
use crate::noctalia::Named;

/// The log's size from which the next write moves it aside first.
const ROTATE_AT: u64 = 10 * 1024 * 1024;
/// How many logs moved aside are kept: `audit.jsonl.1`, the newest, to `audit.jsonl.3`.
const KEPT: u32 = 3;
/// How long a server on its way out waits to say its log is behind.
const WARN_LIMIT: Duration = Duration::from_millis(100);

/// How long a call waits for its line to be written before it goes on without.
pub(crate) const WRITE_LIMIT: Duration = Duration::from_secs(1);
/// How many lines may wait for the writer; a call that finds this many goes unlogged.
const QUEUE: usize = 64;

/// A line for the writer, and the sender it answers on once the line is written.
type Job = (Vec<u8>, tokio::sync::oneshot::Sender<()>);

/// Where the log goes, and the last write error, which `status` reports.
#[derive(Debug, Clone)]
pub(crate) struct Audit {
    path: Option<PathBuf>,
    last_error: Arc<Mutex<Option<String>>>,
    /// The writer thread's queue, started with the first line; `None` if it couldn't be.
    writer: Arc<OnceLock<Option<SyncSender<Job>>>>,
}

/// A line handed to the writer, to wait for.
#[derive(Debug)]
pub(crate) struct Pending {
    audit: Audit,
    /// Answered once the line is written; `None` when it never reached the writer.
    written: Option<tokio::sync::oneshot::Receiver<()>>,
}

impl Pending {
    /// Waits for the line until `by`. Past it the call goes on, and `status` says the
    /// write is still pending.
    pub(crate) async fn wait(self, by: tokio::time::Instant) {
        let Some(written) = self.written else {
            return;
        };
        let unlogged = match tokio::time::timeout_at(by, written).await {
            Ok(Ok(())) => return,
            Ok(Err(_)) => "the log's writer thread ended; this call went unlogged".to_owned(),
            Err(_) => format!(
                "a write took over {} s, as on a stalled file system; it is still pending",
                WRITE_LIMIT.as_secs()
            ),
        };
        if let Some(path) = &self.audit.path {
            *self.audit.last() = Some(format!("{}: {unlogged}", path.display()));
        }
    }
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
    /// For element actions, whether their effect was read back, from their result.
    #[serde(skip_serializing_if = "Option::is_none")]
    effect: Option<&'a str>,
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
    tool: Cow<'a, str>,
    action: bool,
    ts: String,
    started: Instant,
}

impl<'a> Call<'a> {
    pub(crate) fn start(tool: impl Into<Cow<'a, str>>) -> Self {
        Self {
            tool: tool.into(),
            action: false,
            ts: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
            started: Instant::now(),
        }
    }

    /// When the call started, as its line says.
    pub(crate) fn ts(&self) -> &str {
        &self.ts
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
            writer: Arc::default(),
        }
    }

    /// The log's file, if there is a state directory to keep it in.
    pub(crate) fn path(&self) -> Option<PathBuf> {
        self.path.clone()
    }

    pub(crate) fn status(&self) -> AuditStatus {
        AuditStatus {
            path: self.path.clone(),
            last_error: self.last().clone(),
        }
    }

    /// Logs a finished call, waiting at most `WRITE_LIMIT` for its line: `args` is its
    /// metadata, and the outcome comes from `result`.
    pub(crate) async fn finish(
        &self,
        call: &Call<'_>,
        caller: Caller<'_>,
        args: Value,
        result: &Result<CallToolResult, ErrorData>,
    ) {
        let by = tokio::time::Instant::now() + WRITE_LIMIT;
        self.queue(call, caller, args, result).wait(by).await;
    }

    /// Queues a finished call's line without waiting for it, for a caller that logs
    /// several and waits for them under one deadline (`Pending::wait`).
    pub(crate) fn queue(
        &self,
        call: &Call<'_>,
        caller: Caller<'_>,
        mut args: Value,
        result: &Result<CallToolResult, ErrorData>,
    ) -> Pending {
        matched(&mut args, result);
        let reported = if call.action {
            reported(result)
        } else {
            Reported::default()
        };
        let summary = summarize(&reported, result);
        self.queue_record(&Record {
            ts: &call.ts,
            session: caller.session,
            instance: caller.instance,
            tool: &call.tool,
            args: &args,
            accepted: summary.accepted,
            observed: summary.observed.as_deref(),
            effect: reported.effect,
            changes: reported.changes,
            error: summary.error,
            may_have_sent: may_have_sent(result),
            duration_ms: call.started.elapsed().as_millis(),
        })
    }

    /// Appends one line through the writer, waiting at most `WRITE_LIMIT`.
    #[cfg(test)]
    async fn write(&self, record: &Record<'_>) {
        let by = tokio::time::Instant::now() + WRITE_LIMIT;
        self.queue_record(record).wait(by).await;
    }

    /// Queues one line for the writer. A failure is kept for `status` and doesn't fail
    /// the tool call.
    fn queue_record(&self, record: &Record<'_>) -> Pending {
        let unqueued = Pending {
            audit: self.clone(),
            written: None,
        };
        let Some(path) = &self.path else {
            return unqueued;
        };
        let line = match serde_json::to_vec(record) {
            Ok(line) => line,
            Err(error) => {
                *self.last() = Some(error.to_string());
                return unqueued;
            }
        };
        match self.send(path, line) {
            Ok(written) => Pending {
                written: Some(written),
                ..unqueued
            },
            Err(unlogged) => {
                *self.last() = Some(format!("{}: {unlogged}", path.display()));
                unqueued
            }
        }
    }

    /// Hands `line` to the writer; an empty one is written as nothing, so waiting for it
    /// waits for every line before it.
    fn send(
        &self,
        path: &Path,
        line: Vec<u8>,
    ) -> Result<tokio::sync::oneshot::Receiver<()>, String> {
        let Some(jobs) = self.writer(path) else {
            return Err(
                "the log's writer thread couldn't be started; calls go unlogged".to_owned(),
            );
        };
        let (done, written) = tokio::sync::oneshot::channel();
        match jobs.try_send((line, done)) {
            Ok(()) => Ok(written),
            Err(TrySendError::Full(_)) => Err(format!(
                "{QUEUE} lines are waiting to be written, as on a stalled file system; this call went unlogged"
            )),
            Err(TrySendError::Disconnected(_)) => {
                Err("the log's writer thread ended; this call went unlogged".to_owned())
            }
        }
    }

    /// Waits until every line queued so far is written, at most `WRITE_LIMIT`, for a
    /// server about to exit: the writer is a thread the process doesn't wait for. A log
    /// still behind says so on stderr, and this returns false.
    pub(crate) async fn flush(&self) -> bool {
        let (Some(path), Some(_started)) = (&self.path, self.writer.get()) else {
            return true;
        };
        let by = tokio::time::Instant::now() + WRITE_LIMIT;
        let flushed = tokio::time::timeout_at(by, async {
            // A full queue takes the marker once the writer has room again.
            let written = loop {
                match self.send(path, Vec::new()) {
                    Ok(written) => break written,
                    Err(_) => tokio::time::sleep(Duration::from_millis(10)).await,
                }
            };
            written.await.is_ok()
        })
        .await
        .unwrap_or(false);
        if !flushed {
            let warning = format!(
                "the audit log {} was still being written when the server ended; its last lines may be missing",
                path.display()
            );
            let _written = crate::cli::print_error_within(warning, WARN_LIMIT).await;
        }
        flushed
    }

    /// The writer's queue, starting its thread for the first line.
    fn writer(&self, path: &Path) -> Option<&SyncSender<Job>> {
        self.writer
            .get_or_init(|| {
                let (jobs, queue) = std::sync::mpsc::sync_channel::<Job>(QUEUE);
                let (path, last_error) = (path.to_owned(), Arc::clone(&self.last_error));
                std::thread::Builder::new()
                    .name("audit-writer".to_owned())
                    .spawn(move || write_each(&queue, &path, &last_error))
                    .ok()
                    .map(|_thread| jobs)
            })
            .as_ref()
    }

    fn last(&self) -> std::sync::MutexGuard<'_, Option<String>> {
        self.last_error
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

/// The writer thread: appends each queued line to `path` in order, keeping a failure in
/// `last_error`, and answers once it is written. It ends once every `Audit` sharing the
/// queue is dropped.
fn write_each(
    queue: &std::sync::mpsc::Receiver<Job>,
    path: &Path,
    last_error: &Mutex<Option<String>>,
) {
    for (line, done) in queue {
        if line.is_empty() {
            let _ignored = done.send(());
            continue;
        }
        if let Err(error) = append(path, &line, ROTATE_AT) {
            *last_error.lock().unwrap_or_else(PoisonError::into_inner) = Some(error.to_string());
        }
        // The call may have stopped waiting; its line is written all the same.
        let _ignored = done.send(());
    }
}

/// Appends `line` to the log at `path`, moving the log aside first once it has reached
/// `limit` bytes. A failed rotation doesn't cost the line: it is written all the same, and
/// the rotation's error returned after. Each error names the file it is about.
fn append(path: &Path, line: &[u8], limit: u64) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
            .map_err(about(dir))?;
    }
    let rotated = rotate(path, limit);
    let mut file = OpenOptions::new()
        .append(true)
        .create(true)
        .mode(0o600)
        .open(path)
        .map_err(about(path))?;
    // One write per line, so concurrent servers appending to the same file don't
    // interleave within a line. A server that opened the log just before another moved it
    // aside writes its line to the moved file, whole.
    file.write_all(&[line, b"\n"].concat())
        .map_err(about(path))?;
    rotated
}

/// `error` with the file it is about in front.
fn about(path: &Path) -> impl FnOnce(std::io::Error) -> std::io::Error {
    move |error| std::io::Error::new(error.kind(), format!("{}: {error}", path.display()))
}

/// Moves the log at `path` aside once it has reached `limit` bytes: it becomes `.1`, `.1`
/// becomes `.2`, and so on to `KEPT`, the oldest dropped. Every server sharing the log
/// does this under its lock file, and looks at the size again once it holds it, so two
/// can't rotate the same log twice. A server that finds the lock held doesn't wait for
/// it: the holder is rotating, and this line goes to the log as it is.
fn rotate(path: &Path, limit: u64) -> std::io::Result<()> {
    let below = || std::fs::metadata(path).map_or(true, |log| log.len() < limit);
    if below() {
        return Ok(());
    }
    let lock_path = numbered(path, "lock");
    let lock = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(&lock_path)
        .map_err(about(&lock_path))?;
    match lock.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => return Ok(()),
        Err(TryLockError::Error(error)) => return Err(about(&lock_path)(error)),
    }
    if below() {
        return Ok(());
    }
    for kept in (1..KEPT).rev() {
        let older = numbered(path, &kept.to_string());
        if let Err(error) = std::fs::rename(&older, numbered(path, &(kept + 1).to_string()))
            && error.kind() != std::io::ErrorKind::NotFound
        {
            return Err(about(&older)(error));
        }
    }
    std::fs::rename(path, numbered(path, "1")).map_err(about(path))
}

/// `path` with `.suffix` after its name, such as `audit.jsonl.1`.
fn numbered(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".");
    name.push(suffix);
    PathBuf::from(name)
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

/// A call rmcp refused before its tool ran, for the log: the names of the fields it was
/// given, each as it is when the tool takes a field of that name and otherwise as
/// `unknown_field` and its length, since it is the agent's text; never their values. Only
/// the first [`REFUSED_FIELDS`] are named and the rest counted, so a call with thousands of
/// keys can't write a line that pushes earlier calls out of the kept logs.
pub(crate) fn refused(given: &[String], takes: impl Fn(&str) -> bool) -> Value {
    let fields: Vec<Value> = given
        .iter()
        .take(REFUSED_FIELDS)
        .map(|field| known(field, takes(field), "unknown_field"))
        .collect();
    match given.len().saturating_sub(REFUSED_FIELDS) {
        0 => json!({ "fields": fields }),
        more => json!({ "fields": fields, "more": more }),
    }
}

/// How many of a refused call's field names its log line holds.
const REFUSED_FIELDS: usize = 32;

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

/// `key`'s combinations as they are when XKB knows every one by name, otherwise only how
/// many, as for names that give a keysym's number, which can spell any text.
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

/// `noctalia`'s logged arguments with what Noctalia's lists named of them, when the call
/// got far enough to ask, never the arguments themselves.
pub(crate) fn noctalia_named(mut logged: Value, named: Option<&Named>) -> Value {
    let (Some(named), Some(fields)) = (named, logged.as_object_mut()) else {
        return logged;
    };
    for (key, value) in [
        ("command", &named.command),
        ("description", &named.description),
        ("plugin", &named.plugin),
    ] {
        if let Some(value) = value {
            fields.insert(key.to_owned(), Value::String(value.clone()));
        }
    }
    logged
}

/// `launch`'s arguments with the desktop entry its name found, which the server knows, in
/// place of the unknown preset.
pub(crate) fn desktop_entry(mut logged: Value, entry: Option<&String>) -> Value {
    if let (Some(entry), Some(fields)) = (entry, logged.as_object_mut()) {
        fields.remove("preset");
        fields.insert("desktop_entry".to_owned(), Value::String(entry.clone()));
    }
    logged
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

/// An action's outcome as the audit log keeps it: `accepted` and `observed` from its
/// result, and the error's name. The overlay's record of the action (`live`) keeps the
/// same, so the two never disagree.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub(crate) struct Summary {
    pub(crate) accepted: Option<bool>,
    pub(crate) observed: Option<String>,
    pub(crate) error: Option<String>,
}

/// An action's outcome, from its result.
pub(crate) fn summary(result: &Result<CallToolResult, ErrorData>) -> Summary {
    summarize(&reported(result), result)
}

fn summarize(reported: &Reported<'_>, result: &Result<CallToolResult, ErrorData>) -> Summary {
    Summary {
        accepted: reported.accepted,
        observed: reported.observed.map(str::to_owned),
        error: outcome(result),
    }
}

/// What an action's result says it did, as the log keeps it.
#[derive(Debug, Default)]
struct Reported<'a> {
    accepted: Option<bool>,
    observed: Option<&'a str>,
    effect: Option<&'a str>,
    changes: Option<Value>,
}

/// An action's `accepted`, `observed` and `effect` fields, which hold only booleans and
/// outcome names, and its change report as counts. Failures have none of them.
fn reported(result: &Result<CallToolResult, ErrorData>) -> Reported<'_> {
    let Some(content) = result
        .as_ref()
        .ok()
        .filter(|result| result.is_error != Some(true))
        .and_then(|result| result.structured_content.as_ref())
    else {
        return Reported::default();
    };
    Reported {
        accepted: content.get("accepted").and_then(Value::as_bool),
        observed: content.get("observed").and_then(Value::as_str),
        effect: content.get("effect").and_then(Value::as_str),
        changes: content.get("changes").map(counted),
    }
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

    /// A refused call with thousands of keys names only the first ones and counts the rest.
    #[test]
    fn a_refused_call_names_at_most_its_first_fields() {
        let given: Vec<String> = (0..40_000).map(|n| format!("k{n}")).collect();
        let logged = refused(&given, |field| field == "k0");
        assert_eq!(
            logged["fields"].as_array().map(Vec::len),
            Some(REFUSED_FIELDS)
        );
        assert_eq!(logged["fields"][0], "k0");
        assert_eq!(logged["more"], 40_000 - REFUSED_FIELDS);
        assert!(refused(&given[..2], |_| true).get("more").is_none());
    }

    fn record<'a>(tool: &'a str, args: &'a Value, error: Option<String>) -> Record<'a> {
        Record {
            ts: "2026-10-08T00:00:00.000Z",
            session: "smoke/1",
            instance: Some("niri.wayland-1.1.sock"),
            tool,
            args,
            accepted: None,
            observed: None,
            effect: None,
            changes: None,
            error,
            may_have_sent: false,
            duration_ms: 12,
        }
    }

    #[tokio::test]
    async fn appends_one_private_line_per_call() {
        let dir = crate::test_support::fresh_dir("audit");
        let audit = Audit::new(Some(dir.join("state")));
        let args = serde_json::json!({"target": "focused_output"});
        audit.write(&record("screenshot", &args, None)).await;
        audit
            .write(&record(
                "desktop_state",
                &args,
                Some("niri_unavailable".to_owned()),
            ))
            .await;
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
        let mode = |file: &Path| file.metadata().unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&path), 0o600);
        assert_eq!(mode(path.parent().unwrap()), 0o700);
        assert_eq!(audit.status().last_error, None);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A FIFO at the log's path, which blocks the writer's open as a stalled NFS or FUSE
    /// state directory would, and a reader that opens it once released, or after five
    /// seconds whatever happens, so a regression fails the test rather than hanging it.
    /// Each line is appended through an open of its own, so the reader opens the FIFO again
    /// until it has read `lines`.
    fn stalled_log(
        name: &str,
        lines: usize,
    ) -> (
        PathBuf,
        Audit,
        std::sync::mpsc::Sender<()>,
        std::thread::JoinHandle<String>,
    ) {
        let dir = crate::test_support::fresh_dir(name);
        let log = dir.join("niri-computer-use/audit.jsonl");
        std::fs::create_dir_all(log.parent().unwrap()).unwrap();
        rustix::fs::mknodat(
            rustix::fs::CWD,
            &log,
            rustix::fs::FileType::Fifo,
            rustix::fs::Mode::from_raw_mode(0o600),
            0,
        )
        .unwrap();
        let (release, released) = std::sync::mpsc::channel::<()>();
        let reader = std::thread::spawn(move || {
            let _released_or_timed_out = released.recv_timeout(Duration::from_secs(5));
            read_fifo(&log, lines, Instant::now() + Duration::from_secs(10))
        });
        (dir.clone(), Audit::new(Some(dir)), release, reader)
    }

    /// Reads `lines` from the FIFO at `log`, or what came by `by`. Opened for reading and
    /// writing, without blocking, it never waits for a writer, and a writer always finds
    /// it open, so a line that never comes ends the read at `by` instead of hanging it.
    #[expect(
        clippy::disallowed_methods,
        reason = "runs on a thread of its own, never on the runtime"
    )]
    fn read_fifo(log: &Path, lines: usize, by: Instant) -> String {
        use std::io::Read as _;
        let nonblocking = i32::try_from(rustix::fs::OFlags::NONBLOCK.bits()).unwrap();
        let mut fifo = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(nonblocking)
            .open(log)
            .unwrap();
        let mut read = Vec::new();
        let mut chunk = [0_u8; 4096];
        while String::from_utf8_lossy(&read).lines().count() < lines && Instant::now() < by {
            match fifo.read(&mut chunk) {
                Ok(count) => read.extend_from_slice(&chunk[..count]),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("{error}"),
            }
        }
        String::from_utf8(read).unwrap()
    }

    /// A stalled log holds a call up for at most `WRITE_LIMIT`, and the line is written
    /// once the file opens.
    #[tokio::test]
    async fn a_stalled_log_holds_a_call_up_for_at_most_its_limit() {
        let (dir, audit, release, reader) = stalled_log("audit-stalled", 1);

        let started = Instant::now();
        audit.write(&record("status", &Value::Null, None)).await;
        let took = started.elapsed();

        release.send(()).ok();
        let text = reader.join().unwrap();
        assert!(took < WRITE_LIMIT + Duration::from_millis(500), "{took:?}");
        let error = audit.status().last_error.unwrap_or_default();
        assert!(error.contains("still pending"), "{error}");
        assert!(text.contains("\"tool\":\"status\""), "{text}");
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Lines queued together and waited for under one deadline, as a bridge answering
    /// every call in flight does, hold it up once in all, and a flush before exit writes
    /// every one of them once the log opens.
    #[tokio::test]
    async fn lines_waited_for_under_one_deadline_hold_it_up_once() {
        let (dir, audit, release, reader) = stalled_log("audit-stalled-many", 3);
        let caller = Caller {
            session: "smoke/1",
            instance: None,
        };
        let refused = ToolError::new(ErrorName::EngineLost, "the engine ended").into_result();

        let started = Instant::now();
        let pending: Vec<Pending> = (0..3)
            .map(|_| {
                audit.queue(
                    &Call::action("key"),
                    caller,
                    json!({}),
                    &Ok(refused.clone()),
                )
            })
            .collect();
        let by = tokio::time::Instant::now() + WRITE_LIMIT;
        for line in pending {
            line.wait(by).await;
        }
        let took = started.elapsed();
        release.send(()).ok();
        audit.flush().await;

        let text = reader.join().unwrap();
        assert!(took < WRITE_LIMIT + Duration::from_millis(500), "{took:?}");
        assert_eq!(text.lines().count(), 3, "{text}");
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A flush that finds the queue full, behind a log that opens again within its
    /// limit, waits for the writer to make room rather than giving up at once.
    #[tokio::test]
    async fn a_flush_behind_a_full_queue_waits_for_room() {
        let lines = QUEUE + 1;
        let (dir, audit, release, reader) = stalled_log("audit-full", lines);
        let caller = Caller {
            session: "smoke/1",
            instance: None,
        };
        let refused = ToolError::new(ErrorName::EngineLost, "the engine ended").into_result();
        let queue = || {
            audit.queue(
                &Call::action("key"),
                caller,
                json!({}),
                &Ok(refused.clone()),
            )
        };
        // One line the writer holds in its open, and a full queue behind it.
        let first = queue();
        tokio::time::sleep(Duration::from_millis(50)).await;
        let queued: Vec<Pending> = (1..lines).map(|_| queue()).collect();

        release.send(()).ok();
        let caught_up = audit.flush().await;

        drop((first, queued));
        let text = reader.join().unwrap();
        assert!(caught_up);
        assert_eq!(text.lines().count(), lines);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn a_failed_write_is_kept_for_status() {
        let dir = crate::test_support::fresh_dir("audit-fail");
        // A file where the log's directory should be.
        std::fs::write(dir.join("niri-computer-use"), "").unwrap();
        let audit = Audit::new(Some(dir.clone()));
        audit.write(&record("status", &Value::Null, None)).await;
        assert!(audit.status().last_error.is_some());
        assert_eq!(
            Audit::new(None).status().last_error.as_deref(),
            Some("neither XDG_STATE_HOME nor HOME is set")
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// The log and the ones moved aside, newest first, as each one's lines.
    fn logs(path: &Path) -> Vec<Vec<String>> {
        let read = |file: PathBuf| {
            std::fs::read_to_string(file)
                .unwrap_or_default()
                .lines()
                .map(str::to_owned)
                .collect()
        };
        let mut logs = vec![read(path.to_owned())];
        logs.extend((1..=KEPT + 1).map(|kept| read(numbered(path, &kept.to_string()))));
        logs
    }

    /// Line `n`, ten bytes with its newline.
    fn line(n: usize) -> String {
        format!("line {n:04}")
    }

    /// Runs the ignored test `name` in a process of its own and checks that it passed. A
    /// child another test forks shares the log's lock file until it runs its program, and
    /// a rotation that finds the lock held waits for the next line, so tests of when the
    /// log is moved aside run where no other test forks.
    async fn alone(name: &str) {
        let name = format!("audit::tests::{name}");
        let args = ["--exact", "--ignored", "--test-threads=1", "--quiet", &name].map(String::from);
        let done = crate::runner::run(
            std::env::current_exe().unwrap(),
            crate::runner::Search::Own,
            &args,
            Duration::from_secs(30),
            64 * 1024,
        )
        .await
        .unwrap();
        let stdout = String::from_utf8_lossy(&done.stdout);
        assert!(done.status.success(), "{stdout}{}", done.stderr);
        assert!(stdout.contains("1 passed"), "{stdout}");
    }

    // The log is moved aside once it reaches its limit, before the next line; three logs
    // moved aside are kept and older ones dropped, and every file stays private.
    #[tokio::test]
    async fn a_full_log_is_moved_aside_and_only_three_old_ones_are_kept() {
        alone("full_log_alone").await;
    }

    #[test]
    #[ignore = "a_full_log_is_moved_aside_and_only_three_old_ones_are_kept runs it alone"]
    fn full_log_alone() {
        let dir = crate::test_support::fresh_dir("audit-rotate");
        let path = dir.join("audit.jsonl");
        // Three lines fill 30 bytes.
        for n in 0..20 {
            append(&path, line(n).as_bytes(), 30).unwrap();
        }
        let lines = |range: std::ops::Range<usize>| range.map(line).collect::<Vec<_>>();
        assert_eq!(
            logs(&path),
            [
                lines(18..20),
                lines(15..18),
                lines(12..15),
                lines(9..12),
                Vec::new()
            ]
        );
        for file in [
            "audit.jsonl",
            "audit.jsonl.1",
            "audit.jsonl.3",
            "audit.jsonl.lock",
        ] {
            let mode = dir.join(file).metadata().unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "{file}");
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    // A server that finds another rotating, holding the log's lock, doesn't wait: its line
    // goes to the log as it is, and the log is moved aside on a later write.
    #[tokio::test]
    async fn a_held_lock_delays_the_rotation_but_never_the_line() {
        alone("held_lock_alone").await;
    }

    #[test]
    #[ignore = "a_held_lock_delays_the_rotation_but_never_the_line runs it alone"]
    fn held_lock_alone() {
        let dir = crate::test_support::fresh_dir("audit-rotate-held");
        let path = dir.join("audit.jsonl");
        for n in 0..3 {
            append(&path, line(n).as_bytes(), 30).unwrap();
        }
        let lock = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(numbered(&path, "lock"))
            .unwrap();
        lock.try_lock().unwrap();
        append(&path, line(3).as_bytes(), 30).unwrap();
        assert_eq!(logs(&path)[..2], [(0..4).map(line).collect(), Vec::new()]);
        drop(lock);
        append(&path, line(4).as_bytes(), 30).unwrap();
        assert_eq!(
            logs(&path)[..2],
            [vec![line(4)], (0..4).map(line).collect()]
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    // A rotation that fails says which file it failed on, here the lock file, and the line
    // is still written.
    #[test]
    fn a_failed_rotation_names_its_file_and_keeps_the_line() {
        let dir = crate::test_support::fresh_dir("audit-rotate-fails");
        let path = dir.join("audit.jsonl");
        append(&path, line(0).as_bytes(), 1).unwrap();
        std::fs::create_dir(numbered(&path, "lock")).unwrap();
        let error = append(&path, line(1).as_bytes(), 1)
            .unwrap_err()
            .to_string();
        let lock = numbered(&path, "lock").display().to_string();
        assert!(error.starts_with(&format!("{lock}: ")), "{error}");
        assert_eq!(logs(&path)[0], [line(0), line(1)]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    // Servers sharing the log write and rotate it at once: every line ends up whole in one
    // of the logs, none twice and none lost, while fewer than four rotations are needed.
    #[test]
    fn servers_writing_at_once_lose_and_split_no_line() {
        let dir = crate::test_support::fresh_dir("audit-rotate-shared");
        let path = dir.join("audit.jsonl");
        // 400 lines of ten bytes: at most four logs of 1000 bytes, so none is dropped.
        let write = |writer: usize| {
            for n in 0..100 {
                append(&path, line(writer * 100 + n).as_bytes(), 1000).unwrap();
            }
        };
        std::thread::scope(|scope| {
            for writer in 0..4 {
                scope.spawn(move || write(writer));
            }
        });
        let mut written: Vec<String> = logs(&path).concat();
        written.sort();
        assert_eq!(written, (0..400).map(line).collect::<Vec<_>>());
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
    #[tokio::test]
    async fn a_stale_lock_read_is_logged_by_name_with_bounded_arguments() {
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
        audit
            .finish(&Call::action("key"), caller, args, &Ok(stale.into_result()))
            .await;
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

    /// A failure after input went out, or may have, says so in its result and in its audit
    /// line, so neither an agent nor the user takes it for one that sent nothing.
    #[tokio::test]
    async fn a_failure_that_may_have_sent_input_says_so_in_its_result_and_line() {
        let dir = crate::test_support::fresh_dir("audit-may-have-sent");
        let audit = Audit::new(Some(dir.clone()));
        let caller = Caller {
            session: "smoke/1",
            instance: None,
        };
        let mut sent = ToolError::new(ErrorName::UpstreamError, "niri didn't answer");
        sent.may_have_sent = true;
        let plain = ToolError::new(ErrorName::FocusMismatch, "another window has focus");
        let mut results = Vec::new();
        for error in [sent, plain] {
            let result = error.into_result();
            results.push(result.structured_content.clone().unwrap());
            audit
                .finish(&Call::action("key"), caller, json!({}), &Ok(result))
                .await;
        }
        assert_eq!(results[0]["may_have_sent"], true, "{}", results[0]);
        assert_eq!(results[1].get("may_have_sent"), None, "{}", results[1]);
        let text = std::fs::read_to_string(dir.join("niri-computer-use/audit.jsonl")).unwrap();
        let lines: Vec<Value> = text
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let logged: Vec<_> = lines
            .iter()
            .map(|line| (line["error"].clone(), line["may_have_sent"].clone()))
            .collect();
        assert_eq!(
            logged,
            [
                (json!("upstream_error"), json!(true)),
                (json!("focus_mismatch"), Value::Null)
            ]
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn keys_named_by_number_are_logged_as_a_count() {
        let named = [
            "ctrl+Escape".to_owned(),
            "a".to_owned(),
            "Return".to_owned(),
        ];
        assert_eq!(keys(&named), json!(named));
        // "SECR" spelled as code points and keysym numbers, as XKB resolves them.
        for spelled in [
            ["U0053", "U0045", "U0043", "u0052"],
            ["0x1000053", "0x1000045", "0x1000043", "0x1000052"],
            ["0x53", "0x45", "0x43", "0x52"],
        ] {
            let spelled = spelled.map(str::to_owned);
            assert_eq!(
                keys(&spelled),
                json!({"category": "invalid_keys", "count": 4}),
                "{spelled:?}"
            );
        }
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
    fn noctalia_logs_counts_byte_lengths_and_only_what_noctalia_named() {
        let args = ["clipboard-copy".to_owned(), "secret".to_owned()];
        assert_eq!(noctalia(&args), json!({"count": 2, "bytes": [14, 6]}));
        assert_eq!(
            noctalia_named(noctalia(&args), None),
            json!({"count": 2, "bytes": [14, 6]})
        );
        let named = Named {
            command: Some("clipboard-copy".to_owned()),
            description: Some("Copy text to the clipboard".to_owned()),
            plugin: None,
        };
        let logged = noctalia_named(noctalia(&args), Some(&named));
        assert_eq!(
            logged,
            json!({
                "count": 2, "bytes": [14, 6],
                "command": "clipboard-copy", "description": "Copy text to the clipboard"
            })
        );
        assert!(!logged.to_string().contains("secret"), "{logged}");
    }

    /// A desktop entry's id is the server's, read from a file it found, so it is logged
    /// by name; a name that found nothing stays a length.
    #[test]
    fn launch_logs_the_desktop_entry_it_found_by_id() {
        let asked =
            |name: &str| json!({"preset": known(name, false, "unknown_preset"), "reuse": false});
        assert_eq!(
            desktop_entry(
                asked("org.gnome.Nautilus"),
                Some(&"org.gnome.Nautilus".to_owned())
            ),
            json!({"desktop_entry": "org.gnome.Nautilus", "reuse": false})
        );
        assert_eq!(
            desktop_entry(asked("my secret"), None),
            json!({"preset": {"category": "unknown_preset", "bytes": 9}, "reuse": false})
        );
    }

    #[tokio::test]
    async fn actions_log_what_was_accepted_and_observed() {
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
        audit
            .finish(
                &Call::action("focus"),
                caller,
                args.clone(),
                &Ok(done.clone()),
            )
            .await;
        audit
            .finish(
                &Call::start("desktop_state"),
                caller,
                args.clone(),
                &Ok(done),
            )
            .await;
        let refused = ToolError::new(ErrorName::LeaseRequired, "no").into_result();
        audit
            .finish(&Call::action("focus"), caller, args, &Ok(refused))
            .await;
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

    #[tokio::test]
    async fn a_change_report_is_logged_as_counts_without_titles_or_names() {
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
        audit
            .finish(
                &Call::action("activate_element"),
                caller,
                args.clone(),
                &Ok(reported),
            )
            .await;
        audit
            .finish(&Call::action("key"), caller, args, &Ok(unknown))
            .await;
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
