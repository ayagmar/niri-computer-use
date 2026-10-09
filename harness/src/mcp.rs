//! A minimal MCP client for the nested checks: one `niri-computer-use serve` whose stdin
//! stays open for one request after another. The server's output goes to a log file, one
//! JSON-RPC message per line, and the client reads replies back from there.

use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use serde_json::{Map, Value, json};

use crate::failure::{Context as _, Failure, Result};
use crate::runner::Process;
use crate::session::Session;

/// Longer than the slowest action, about fifteen seconds: the readiness report, five
/// seconds of waiting, half a second of settling, and a screenshot.
const REPLY: Duration = Duration::from_secs(20);

#[derive(Debug)]
pub(crate) struct Client {
    process: Process,
    log: PathBuf,
    next_id: u64,
}

impl Client {
    /// Starts `server serve` as the MCP client `name` and completes the handshake. The
    /// server runs until `stop`, or until `deadline` kills it.
    pub(crate) fn start(
        session: &mut Session<'_>,
        server: &str,
        name: &str,
        deadline: Duration,
    ) -> Result<Self> {
        Self::start_command(session, server, &["serve".into()], name, deadline)
    }

    /// As `start`, with the server started by `program` and `args`, such as `env` to
    /// change its environment.
    pub(crate) fn start_command(
        session: &mut Session<'_>,
        program: &str,
        args: &[OsString],
        name: &str,
        deadline: Duration,
    ) -> Result<Self> {
        let log = session.artifact(&format!("server-{name}.log"));
        let process = session.serve(program, args, log.clone(), deadline)?;
        let mut client = Self {
            process,
            log,
            next_id: 1,
        };
        let id = client.request(
            "initialize",
            json!({
                "protocolVersion": "2025-11-25", "capabilities": {},
                "clientInfo": {"name": name, "version": "1"}
            }),
        )?;
        client.reply(session, id)?;
        client.write(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))?;
        Ok(client)
    }

    /// Sends `tools/call` without waiting for the reply. Returns the request's id.
    pub(crate) fn start_call(&mut self, tool: &str, arguments: Value) -> Result<u64> {
        let mut params = Map::new();
        params.insert("name".to_owned(), Value::from(tool));
        params.insert("arguments".to_owned(), arguments);
        self.request("tools/call", Value::Object(params))
    }

    /// Calls `tool` and returns the `result` of its reply.
    pub(crate) fn call(
        &mut self,
        session: &mut Session<'_>,
        tool: &str,
        arguments: Value,
    ) -> Result<Value> {
        let id = self.start_call(tool, arguments)?;
        self.result(session, id)
    }

    /// The `result` of the reply to `id`, waiting up to fifteen seconds for it.
    pub(crate) fn result(&self, session: &mut Session<'_>, id: u64) -> Result<Value> {
        let reply = self.reply(session, id)?;
        reply
            .get("result")
            .cloned()
            .ok_or_else(|| Failure::new(format!("request {id} failed: {reply}")))
    }

    /// The names of the tools the server lists.
    pub(crate) fn tools(&mut self, session: &mut Session<'_>) -> Result<Vec<String>> {
        let id = self.request("tools/list", json!({}))?;
        let result = self.result(session, id)?;
        Ok(field(&result, "/tools")
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|tool| tool.get("name")?.as_str().map(str::to_owned))
            .collect())
    }

    pub(crate) fn cancel(&mut self, id: u64) -> Result<()> {
        self.write(&json!({"jsonrpc": "2.0", "method": "notifications/cancelled", "params": {"requestId": id, "reason": "nested cancellation test"}}))
    }

    pub(crate) fn stop(self) -> Result<()> {
        self.process.stop().map(drop)
    }

    fn request(&mut self, method: &str, params: Value) -> Result<u64> {
        let id = self.next_id;
        self.next_id += 1;
        let mut message = Map::new();
        message.insert("jsonrpc".to_owned(), Value::from("2.0"));
        message.insert("id".to_owned(), Value::from(id));
        message.insert("method".to_owned(), Value::from(method));
        message.insert("params".to_owned(), params);
        self.write(&Value::Object(message))?;
        Ok(id)
    }

    fn write(&mut self, message: &Value) -> Result<()> {
        let mut line = serde_json::to_vec(message).context("encode a request")?;
        line.push(b'\n');
        self.process.send(line)
    }

    fn reply(&self, session: &mut Session<'_>, id: u64) -> Result<Value> {
        let what = format!("the server's reply to request {id}");
        session.wait_until("m3-reply", &what, REPLY, |_| {
            let text =
                fs::read_to_string(&self.log).context(format!("read {}", self.log.display()))?;
            Ok(reply_in(&text, id))
        })
    }
}

/// Waits until the server sees niri, the nested Noctalia and an unlocked screen, and
/// returns that status.
pub(crate) fn ready(
    session: &mut Session<'_>,
    client: &mut Client,
    step: &str,
    deadline: Duration,
) -> Result<Value> {
    session.wait_until(
        step,
        "status with Noctalia running and the screen unlocked",
        deadline,
        |session| {
            let status = structured(&client.call(session, "status", json!({}))?)?;
            let ready = field(&status, "/noctalia") == "running"
                && field(&status, "/lock/state") == "unlocked";
            Ok(ready.then_some(status))
        },
    )
}

/// A successful call's structured content.
pub(crate) fn structured(result: &Value) -> Result<Value> {
    if field(result, "/isError") != false {
        return Err(Failure::new(format!(
            "expected a successful call; saw {result}"
        )));
    }
    Ok(field(result, "/structuredContent").clone())
}

pub(crate) fn field<'a>(value: &'a Value, pointer: &str) -> &'a Value {
    value.pointer(pointer).unwrap_or(&Value::Null)
}

/// The JSON-RPC response with `id` among the log's lines. Lines that aren't JSON, such as
/// the server's own errors on stderr, are skipped.
fn reply_in(log: &str, id: u64) -> Option<Value> {
    log.lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|message| message.get("id") == Some(&json!(id)) && message.get("method").is_none())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_a_reply_by_id_among_other_lines() {
        let log = "not json\n{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}\n\
                   {\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"ping\"}\n\
                   {\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"x\":1}}\n";
        assert_eq!(reply_in(log, 2).unwrap()["result"]["x"], 1);
        assert_eq!(reply_in(log, 1).unwrap()["result"], json!({}));
        assert_eq!(reply_in(log, 3), None);
    }
}
