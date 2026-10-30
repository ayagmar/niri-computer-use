//! `watch`: what agents are doing on this niri instance, as JSON lines on stdout for a
//! shell to show. One `hello` line first, then `state` whenever the lease, the stop flag
//! the input-dirty marker or the agents' servers change, `action` for each tool call the audit log records
//! for this instance, the last `BACKLOG` of them first, `live` for each record of the
//! action in progress (`crate::live`), and `alive` every `ALIVE`, so a
//! reader can tell a quiet desktop from a watch that ended. The lines carry only what the
//! audit log and the lease record already hold, and for `live` the `app_id` niri reports
//! for the window an action is about, never typed text or window titles.

mod clients;
mod run;
mod tail;
mod wake;

pub(crate) use run::run;

use serde::Serialize;
use serde_json::Value;

pub(crate) use clients::{Client, project_of};

use crate::control::lease::Holder;

/// The format's version, in `hello`. A reader refuses a version it doesn't know.
pub(crate) const FORMAT: u32 = 1;

/// How many recorded calls `watch` sends when it starts.
const BACKLOG: usize = 20;

/// The longest audit line read; a longer one is skipped.
const LINE_LIMIT: usize = 64 * 1024;

/// One line of `watch`'s output.
#[derive(Debug, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub(crate) enum Event<'a> {
    Hello {
        format: u32,
        version: &'static str,
        instance: &'a str,
    },
    State(&'a State),
    Action(Value),
    Live(Value),
    Alive,
}

/// What the desk looks like now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct State {
    pub(crate) lease: Option<Driver>,
    pub(crate) stopped: bool,
    pub(crate) input_dirty: bool,
    /// Every agent's server on this instance.
    pub(crate) clients: Vec<Client>,
}

/// Who holds the lease, or is asking for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Driver {
    /// Such as `claude-code/4711`, as in the audit log.
    pub(crate) session: String,
    pub(crate) since: String,
    /// The holder took the lock and waits for the user to allow it.
    pub(crate) pending: bool,
    /// The last part of the agent's working directory, such as the project's name.
    pub(crate) project: Option<String>,
}

impl Driver {
    pub(crate) fn new(holder: Holder, project: Option<String>) -> Self {
        Self {
            session: holder.label,
            since: holder.since,
            pending: holder.pending,
            project,
        }
    }
}

/// The server PID a session label such as `claude-code/4711` ends with.
pub(crate) fn label_pid(label: &str) -> Option<u32> {
    label.rsplit_once('/')?.1.parse().ok()
}

/// The parent PID in a `/proc/<pid>/status` text.
pub(crate) fn parent_pid(status: &str) -> Option<u32> {
    status
        .lines()
        .find_map(|line| line.strip_prefix("PPid:"))?
        .trim()
        .parse()
        .ok()
}

/// The call an audit line records, if it is one on `instance`, without the instance.
pub(crate) fn action(line: &[u8], instance: &str) -> Option<Value> {
    let mut value: Value = serde_json::from_slice(line).ok()?;
    let object = value.as_object_mut()?;
    if object.remove("instance")?.as_str()? != instance {
        return None;
    }
    Some(value)
}

/// Which record of which call a `live.json` holds: its `pid`, `call` and `phase`.
pub(crate) type LiveKey = (u64, u64, String);

/// The overlay's record in `bytes`, if it is one: a JSON object of at most `live::LIMIT`
/// bytes with a numeric `pid` and `call` and a string `phase`.
pub(crate) fn live(bytes: &[u8]) -> Option<(LiveKey, Value)> {
    if bytes.len() > crate::live::LIMIT {
        return None;
    }
    let record: Value = serde_json::from_slice(bytes).ok()?;
    let key = (
        record.get("pid")?.as_u64()?,
        record.get("call")?.as_u64()?,
        record.get("phase")?.as_str()?.to_owned(),
    );
    record.is_object().then_some((key, record))
}

/// Splits bytes read in pieces into whole lines, dropping a line longer than
/// `LINE_LIMIT` and keeping a last line without its newline for the next piece.
#[derive(Debug, Default)]
pub(crate) struct Lines {
    partial: Vec<u8>,
    overlong: bool,
}

impl Lines {
    pub(crate) fn feed(&mut self, piece: &[u8]) -> Vec<Vec<u8>> {
        let mut pieces = piece.split(|&byte| byte == b'\n');
        let last = pieces.next_back().unwrap_or_default();
        let lines = pieces.filter_map(|line| self.end(line)).collect();
        self.push(last);
        lines
    }

    /// Ends the line with its last bytes, returning it unless it was too long.
    fn end(&mut self, bytes: &[u8]) -> Option<Vec<u8>> {
        self.push(bytes);
        let line = std::mem::take(&mut self.partial);
        (!std::mem::take(&mut self.overlong)).then_some(line)
    }

    fn push(&mut self, bytes: &[u8]) {
        if self.partial.len() + bytes.len() > LINE_LIMIT {
            self.overlong = true;
            self.partial.clear();
        } else if !self.overlong {
            self.partial.extend_from_slice(bytes);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: Vec<Vec<u8>> = Vec::new();

    #[test]
    fn a_line_split_across_reads_comes_out_once_and_whole() {
        let mut lines = Lines::default();
        assert_eq!(lines.feed(b"{\"a\":"), NONE);
        assert_eq!(lines.feed(b"1}\n{\"b\""), vec![b"{\"a\":1}".to_vec()]);
        assert_eq!(lines.feed(b":2}\n"), vec![b"{\"b\":2}".to_vec()]);
        assert_eq!(lines.feed(b""), NONE);
    }

    #[test]
    fn an_overlong_line_is_skipped_and_the_next_one_still_read() {
        let mut lines = Lines::default();
        let long = vec![b'x'; LINE_LIMIT / 2 + 1];
        assert_eq!(lines.feed(&long), NONE);
        assert_eq!(lines.feed(&long), NONE);
        assert_eq!(lines.feed(b"tail\nnext\n"), vec![b"next".to_vec()]);
    }

    #[test]
    fn only_calls_on_this_instance_are_actions() {
        let line = br#"{"ts":"t","session":"claude-code/7","instance":"niri.1.sock","tool":"key"}"#;
        let action = action(line, "niri.1.sock").unwrap();
        assert_eq!(action["tool"], "key");
        assert!(action.get("instance").is_none());
        assert_eq!(super::action(line, "niri.2.sock"), None);
        assert_eq!(super::action(br#"{"tool":"key"}"#, "niri.1.sock"), None);
        assert_eq!(super::action(b"not json", "niri.1.sock"), None);
    }

    #[test]
    fn a_live_record_needs_its_key_and_fits_the_limit() {
        let record = br#"{"pid":7,"call":2,"phase":"aim","tool":"click"}"#;
        let (key, value) = live(record).unwrap();
        assert_eq!(key, (7, 2, "aim".to_owned()));
        assert_eq!(value["tool"], "click");
        assert_eq!(live(br#"{"pid":7,"phase":"aim"}"#), None);
        assert_eq!(live(b"{\"pid\":7,"), None);
        let long = format!(
            r#"{{"pid":7,"call":2,"phase":"aim","pad":"{}"}}"#,
            "x".repeat(5000)
        );
        assert_eq!(live(long.as_bytes()), None);
    }

    #[test]
    fn reads_the_server_pid_and_its_parent() {
        assert_eq!(label_pid("claude-code/4711"), Some(4711));
        assert_eq!(label_pid("odd name/x"), None);
        let status = "Name:\tniri-computer-u\nPid:\t4711\nPPid:\t4100\nTracerPid:\t0\n";
        assert_eq!(parent_pid(status), Some(4100));
        assert_eq!(parent_pid("Name:\tx\n"), None);
    }
}
