//! What a relay answers itself before it first reaches an engine, for a client that
//! requires the shared engine and had none when it started: the handshake, the tool list
//! and pings, as the engine would answer them, so the session goes on unchanged once a
//! call reaches one.

use rmcp::model::{ListToolsResult, ProtocolVersion, Tool};
use serde_json::{Value, json};

use crate::tools;

/// The tools to list until an engine is reached.
pub(crate) struct Before {
    tools: Vec<Tool>,
}

impl Before {
    pub(crate) const fn new(tools: Vec<Tool>) -> Self {
        Self { tools }
    }

    /// The answer to the client's request `id`, `method`, in `line`, when the relay gives
    /// it itself.
    pub(crate) fn answer(&self, id: &Value, method: &str, line: &[u8]) -> Option<Vec<u8>> {
        let result = match method {
            "initialize" => {
                let info = tools::info().with_protocol_version(negotiated(line));
                serde_json::to_value(info).ok()?
            }
            "tools/list" => {
                serde_json::to_value(ListToolsResult::with_all_items(self.tools.clone())).ok()?
            }
            "ping" => json!({}),
            _ => return None,
        };
        let mut answer = json!({"jsonrpc": "2.0", "id": id, "result": result})
            .to_string()
            .into_bytes();
        answer.push(b'\n');
        Some(answer)
    }
}

/// The protocol version rmcp answers the `initialize` in `line` with: the client's, when
/// it is known and has the handshake, and the newest with one otherwise.
fn negotiated(line: &[u8]) -> ProtocolVersion {
    let requested = serde_json::from_slice::<Value>(line)
        .ok()
        .and_then(|request| {
            serde_json::from_value(request.pointer("/params/protocolVersion")?.clone()).ok()
        });
    requested
        .filter(|version: &ProtocolVersion| {
            version.has_initialize() && ProtocolVersion::KNOWN_VERSIONS.contains(version)
        })
        .unwrap_or(ProtocolVersion::LATEST_WITH_INITIALIZE)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn initialize(version: &str) -> Value {
        let request = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": version, "capabilities": {}, "clientInfo": {"name": "c", "version": "1"}}});
        let line = Before::new(Vec::new())
            .answer(&json!(1), "initialize", request.to_string().as_bytes())
            .unwrap();
        serde_json::from_slice(&line).unwrap()
    }

    #[test]
    fn an_unknown_version_or_one_without_the_handshake_gets_the_newest_with_one() {
        for version in ["2099-01-01", ProtocolVersion::LATEST.as_str()] {
            let response = initialize(version);
            assert_eq!(
                response["result"]["protocolVersion"],
                ProtocolVersion::LATEST_WITH_INITIALIZE.as_str(),
                "{version}"
            );
        }
    }
}
