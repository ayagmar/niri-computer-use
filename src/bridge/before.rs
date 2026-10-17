//! What a relay answers itself before it first reaches an engine, for a client that
//! requires the shared engine and had none when it started: the handshake, the tool list
//! and pings, as the engine would answer them, so the session goes on unchanged once a
//! call reaches one.

use rmcp::model::{InitializeRequestParams, ListToolsResult, ProtocolVersion, Tool};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::tools;

/// JSON-RPC's code for invalid params.
const INVALID_PARAMS: i32 = -32602;

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
        let outcome = match method {
            "initialize" => initialize(line),
            "tools/list" => Ok(serde_json::to_value(ListToolsResult::with_all_items(
                self.tools.clone(),
            ))
            .ok()?),
            "ping" => Ok(json!({})),
            _ => return None,
        };
        let message = match outcome {
            Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
            Err(message) => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {"code": INVALID_PARAMS, "message": message}
            }),
        };
        let mut answer = message.to_string().into_bytes();
        answer.push(b'\n');
        Some(answer)
    }
}

/// The engine's answer to the `initialize` request in `line`, or why its params are
/// invalid, which an engine would refuse when the request is replayed to it.
fn initialize(line: &[u8]) -> Result<Value, String> {
    #[derive(Deserialize)]
    struct Request {
        params: InitializeRequestParams,
    }
    let request: Request = serde_json::from_slice(line)
        .map_err(|error| format!("invalid initialize params: {error}"))?;
    let info = tools::info().with_protocol_version(negotiated(request.params.protocol_version));
    serde_json::to_value(info).map_err(|error| error.to_string())
}

/// The protocol version rmcp answers a client `requested` with: that one, when it is known
/// and has the handshake, and the newest with one otherwise.
fn negotiated(requested: ProtocolVersion) -> ProtocolVersion {
    if requested.has_initialize() && ProtocolVersion::KNOWN_VERSIONS.contains(&requested) {
        requested
    } else {
        ProtocolVersion::LATEST_WITH_INITIALIZE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn initialize(params: &Value) -> Value {
        let request = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": params});
        let line = Before::new(Vec::new())
            .answer(&json!(1), "initialize", request.to_string().as_bytes())
            .unwrap();
        serde_json::from_slice(&line).unwrap()
    }

    #[test]
    fn an_unknown_version_or_one_without_the_handshake_gets_the_newest_with_one() {
        for version in ["2099-01-01", ProtocolVersion::LATEST.as_str()] {
            let params = json!({"protocolVersion": version, "capabilities": {}, "clientInfo": {"name": "c", "version": "1"}});
            let response = initialize(&params);
            assert_eq!(
                response["result"]["protocolVersion"],
                ProtocolVersion::LATEST_WITH_INITIALIZE.as_str(),
                "{version}"
            );
        }
    }

    /// A handshake an engine couldn't take when it is replayed isn't acknowledged.
    #[test]
    fn an_initialize_without_its_params_is_refused() {
        let response = initialize(&json!({}));
        assert_eq!(response["error"]["code"], INVALID_PARAMS, "{response}");
        assert!(response.get("result").is_none(), "{response}");
    }
}
