//! What the bridge reads of each MCP line it relays, without interpreting the rest: whether
//! it is a request, which the bridge must see answered, and the answers it makes itself.
//! Pure.

use serde::Deserialize;
use serde_json::{Value, json};

use crate::error::ToolError;

/// One line, as far as the bridge cares.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Message {
    /// Expects a response with the same `id`.
    Request {
        id: Value,
        method: String,
    },
    /// `cancels` is the request a `notifications/cancelled` names.
    Notification {
        cancels: Option<Value>,
    },
    Response {
        id: Value,
    },
    /// Anything else, such as a line that isn't JSON-RPC.
    Other,
}

#[derive(Deserialize)]
struct Envelope {
    id: Option<Value>,
    method: Option<String>,
    params: Option<Params>,
}

#[derive(Deserialize)]
struct Params {
    #[serde(rename = "requestId")]
    request_id: Option<Value>,
}

pub(crate) fn read(line: &[u8]) -> Message {
    let Ok(envelope) = serde_json::from_slice::<Envelope>(line) else {
        return Message::Other;
    };
    match (envelope.id, envelope.method) {
        (Some(id), Some(method)) => Message::Request { id, method },
        (None, Some(method)) => Message::Notification {
            cancels: (method == "notifications/cancelled")
                .then(|| envelope.params.and_then(|params| params.request_id))
                .flatten(),
        },
        (Some(id), None) => Message::Response { id },
        (None, None) => Message::Other,
    }
}

/// The bridge's own answer to the request `id`, a `method` call, failing with `error`: a
/// tool result with `isError` for `tools/call`, as a tool's failure would be, and a
/// JSON-RPC error otherwise. One line, newline included.
pub(crate) fn answer(id: &Value, method: &str, error: ToolError) -> Vec<u8> {
    let message = if method == "tools/call" {
        let result = serde_json::to_value(error.into_result()).unwrap_or(Value::Null);
        json!({"jsonrpc": "2.0", "id": id, "result": result})
    } else {
        let message = serde_json::to_value(error.name)
            .ok()
            .and_then(|name| Some(format!("{}: {}", name.as_str()?, error.detail)))
            .unwrap_or(error.detail);
        json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32603, "message": message}})
    };
    let mut line = message.to_string().into_bytes();
    line.push(b'\n');
    line
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorName;

    #[test]
    fn requests_notifications_and_responses_are_told_apart() {
        assert_eq!(
            read(br#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{}}"#),
            Message::Request {
                id: json!(7),
                method: "tools/call".to_owned()
            }
        );
        assert_eq!(
            read(br#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":"a"}}"#),
            Message::Notification {
                cancels: Some(json!("a"))
            }
        );
        assert_eq!(
            read(br#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#),
            Message::Notification { cancels: None }
        );
        assert_eq!(
            read(br#"{"jsonrpc":"2.0","id":"x","result":{}}"#),
            Message::Response { id: json!("x") }
        );
        assert_eq!(read(b"not json"), Message::Other);
    }

    #[test]
    fn a_failed_tool_call_is_a_tool_result_and_anything_else_a_json_rpc_error() {
        let error = || ToolError::new(ErrorName::EngineLost, "it ended");
        let call: Value =
            serde_json::from_slice(&answer(&json!(3), "tools/call", error())).unwrap();
        assert_eq!(call["id"], 3);
        assert_eq!(call["result"]["isError"], true);
        assert_eq!(
            call["result"]["structuredContent"],
            json!({"error": "engine_lost", "detail": "it ended"})
        );
        let other: Value = serde_json::from_slice(&answer(&json!("p"), "ping", error())).unwrap();
        assert_eq!(other["id"], "p");
        assert_eq!(other["error"]["message"], "engine_lost: it ended");
    }
}
