//! A minimal MCP client speaking JSON-RPC over the server's stdin and stdout. It fails the
//! test if any stdout line isn't a JSON-RPC 2.0 message.

use std::process::{ExitStatus, Stdio};
use std::sync::PoisonError;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::mpsc;

use crate::fixture::{Fixture, SPAWNING};

/// Longer than any deadline in the server.
pub(crate) const WAIT: Duration = Duration::from_secs(10);
pub(crate) const CLIENT: &str = "protocol-test";

#[derive(Debug)]
pub(crate) struct Server {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: mpsc::UnboundedReceiver<Value>,
    /// Every message received so far, in order.
    received: Vec<Value>,
    next_id: u64,
    pub(crate) pid: u32,
}

impl Server {
    /// Starts the server without a handshake.
    pub(crate) fn spawn(fixture: &Fixture) -> Self {
        let spawning = SPAWNING.lock().unwrap_or_else(PoisonError::into_inner);
        let mut child = command()
            .arg("serve")
            .env_clear()
            .envs(fixture.env())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        drop(spawning);
        let pid = child.id().unwrap();
        let stdin = child.stdin.take();
        let mut stdout = BufReader::new(child.stdout.take().unwrap()).lines();
        let (sender, lines) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Some(line) = stdout.next_line().await.unwrap() {
                sender.send(json_rpc(&line)).ok();
            }
        });
        Self {
            child,
            stdin,
            lines,
            received: Vec::new(),
            next_id: 1,
            pid,
        }
    }

    /// Starts the server and completes the handshake at the newest version with one.
    pub(crate) async fn start(fixture: &Fixture) -> Self {
        let mut server = Self::spawn(fixture);
        let response = server.initialize("2025-11-25").await;
        assert!(response.get("result").is_some(), "{response}");
        server
    }

    /// `initialize`, then `notifications/initialized`. Returns the whole response.
    pub(crate) async fn initialize(&mut self, version: &str) -> Value {
        let params = json!({
            "protocolVersion": version,
            "capabilities": {},
            "clientInfo": {"name": CLIENT, "version": "1"}
        });
        let id = self.request("initialize", params).await;
        let response = self.response(id).await;
        self.notify("notifications/initialized", json!({})).await;
        response
    }

    pub(crate) async fn request(&mut self, method: &str, params: Value) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .await;
        id
    }

    pub(crate) async fn notify(&mut self, method: &str, params: Value) {
        self.send(&json!({"jsonrpc": "2.0", "method": method, "params": params}))
            .await;
    }

    /// Sends `tools/call` without waiting for the answer.
    pub(crate) async fn start_call(&mut self, tool: &str, arguments: Value) -> u64 {
        self.request("tools/call", json!({"name": tool, "arguments": arguments}))
            .await
    }

    pub(crate) async fn cancel(&mut self, id: u64) {
        self.notify(
            "notifications/cancelled",
            json!({"requestId": id, "reason": "test"}),
        )
        .await;
    }

    async fn send(&mut self, message: &Value) {
        let mut line = serde_json::to_vec(message).unwrap();
        line.push(b'\n');
        let stdin = self.stdin.as_mut().unwrap();
        stdin.write_all(&line).await.unwrap();
        stdin.flush().await.unwrap();
    }

    /// The response to `id`, waiting up to `WAIT` for it.
    pub(crate) async fn response(&mut self, id: u64) -> Value {
        loop {
            if let Some(found) = self.received.iter().find(|message| message["id"] == id) {
                return found.clone();
            }
            let message = tokio::time::timeout(WAIT, self.lines.recv())
                .await
                .unwrap_or_else(|_| panic!("no response to {id} within {WAIT:?}"))
                .unwrap_or_else(|| panic!("stdout closed before the response to {id}"));
            self.received.push(message);
        }
    }

    /// A tool's result: the `result` of a `tools/call` response.
    pub(crate) async fn call(&mut self, tool: &str, arguments: Value) -> Value {
        let id = self.start_call(tool, arguments).await;
        let response = self.response(id).await;
        response
            .get("result")
            .unwrap_or_else(|| panic!("{tool}: {response}"))
            .clone()
    }

    /// A successful tool's structured content.
    pub(crate) async fn structured(&mut self, tool: &str) -> Value {
        let result = self.call(tool, json!({})).await;
        assert_eq!(result["isError"], false, "{tool}: {result}");
        result["structuredContent"].clone()
    }

    pub(crate) async fn tools(&mut self) -> Vec<Value> {
        let id = self.request("tools/list", json!({})).await;
        let response = self.response(id).await;
        response["result"]["tools"].as_array().unwrap().clone()
    }

    /// The ids of the responses received so far, in order.
    pub(crate) fn answered(&self) -> Vec<u64> {
        self.received
            .iter()
            .filter_map(|message| message["id"].as_u64())
            .collect()
    }

    /// Closes stdin and waits for the server to exit. Returns its status, the messages
    /// it wrote that nobody read yet, and its stderr.
    pub(crate) async fn stop(mut self) -> (ExitStatus, Vec<Value>, String) {
        drop(self.stdin.take());
        let status = tokio::time::timeout(WAIT, self.child.wait())
            .await
            .expect("the server didn't exit after stdin closed")
            .unwrap();
        let mut unread = Vec::new();
        while let Some(message) = self.lines.recv().await {
            unread.push(message);
        }
        let mut stderr = String::new();
        if let Some(mut pipe) = self.child.stderr.take() {
            pipe.read_to_string(&mut stderr).await.unwrap();
        }
        (status, unread, stderr)
    }
}

#[expect(
    clippy::disallowed_methods,
    reason = "the test starts the server binary itself, outside the server's runner"
)]
fn command() -> Command {
    Command::new(env!("CARGO_BIN_EXE_niri-desktop-mcp"))
}

/// One stdout line, which must be a JSON-RPC 2.0 message.
fn json_rpc(line: &str) -> Value {
    let message: Value = serde_json::from_str(line)
        .unwrap_or_else(|error| panic!("stdout line isn't JSON ({error}): {line}"));
    assert_eq!(message["jsonrpc"], "2.0", "not JSON-RPC 2.0: {line}");
    message
}

/// A tool error's stable name and detail, checking that the text content repeats the
/// structured content.
pub(crate) fn tool_error(result: &Value) -> (String, String) {
    assert_eq!(result["isError"], true, "{result}");
    let structured = &result["structuredContent"];
    let text: Value = serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(&text, structured);
    (
        structured["error"].as_str().unwrap().to_owned(),
        structured["detail"].as_str().unwrap().to_owned(),
    )
}

/// An argument mistake's message: `isError` with one text block and no structured content.
pub(crate) fn mistake(result: &Value) -> String {
    assert_eq!(result["isError"], true, "{result}");
    assert_eq!(result.get("structuredContent"), None, "{result}");
    let content = result["content"].as_array().unwrap();
    assert_eq!(content.len(), 1, "{result}");
    assert_eq!(content[0]["type"], "text", "{result}");
    content[0]["text"].as_str().unwrap().to_owned()
}
