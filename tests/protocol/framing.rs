//! The limit on a client's lines over stdio, standalone or through the bridge: 16 MiB a
//! line, newline included, however the lines are split into writes. A line at the limit
//! is served; one over it ends the session, even between other lines.

use std::time::Duration;

use serde_json::{Value, json};

use crate::client::Server;
use crate::fixture::Fixture;
use crate::niri::Niri;

pub(crate) const MAX_LINE: usize = 16 * 1024 * 1024;

/// A `status` call as one line of `length` bytes, newline included, padded with leading
/// spaces.
pub(crate) fn padded_status(id: u64, length: usize) -> Vec<u8> {
    let call = json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "tools/call",
        "params": {"name": "status", "arguments": {}}
    })
    .to_string();
    let mut line = vec![b' '; length - call.len() - 1];
    line.extend(call.as_bytes());
    line.push(b'\n');
    line
}

/// A short `status` call, then one over the limit, then another short one.
pub(crate) fn oversized_between(first: u64) -> Vec<u8> {
    let mut lines = padded_status(first, 200);
    lines.extend(padded_status(first + 1, MAX_LINE + 1));
    lines.extend(padded_status(first + 2, 200));
    lines
}

/// Whether `messages` answer any of `ids`.
pub(crate) fn answers(messages: &[Value], ids: &[u64]) -> bool {
    messages
        .iter()
        .any(|message| ids.iter().any(|id| message["id"] == *id))
}

/// Writes `bytes` in two parts, split at `split`, with time between them for the server
/// to read the first part on its own.
async fn send_split(server: &mut Server, bytes: &[u8], split: usize) {
    let (first, second) = bytes.split_at(split);
    server.send_raw(first).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    server.send_raw(second).await;
}

#[tokio::test]
async fn a_line_at_the_limit_is_served_however_it_is_split() {
    let fixture = Fixture::new("framing-limit");
    let _niri = Niri::start(&fixture);
    let mut server = Server::start(&fixture).await;
    for (id, split) in [(100, 1), (101, MAX_LINE / 2), (102, MAX_LINE - 1)] {
        send_split(&mut server, &padded_status(id, MAX_LINE), split).await;
        let response = server.response(id).await;
        assert!(response.get("result").is_some(), "{response}");
    }
}

#[tokio::test]
async fn a_line_over_the_limit_between_others_ends_the_session_however_it_is_split() {
    let lines = oversized_between(100);
    // The oversized line's newline, and the bytes on either side of it.
    let newline = 200 + MAX_LINE;
    for split in [1, newline - 1, newline, newline + 1] {
        let fixture = Fixture::new("framing-over");
        let _niri = Niri::start(&fixture);
        let mut server = Server::start(&fixture).await;
        send_split(&mut server, &lines, split).await;
        let (_, unread, _) = server.stop().await;
        assert!(
            !answers(&unread, &[101, 102]),
            "split at {split}: {unread:?}"
        );
    }
}
