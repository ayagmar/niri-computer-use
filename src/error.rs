//! Tool failures with the stable error names agents rely on.

use rmcp::model::CallToolResult;
use serde::Serialize;

/// The message of the internal error a cancelled call ends with. rmcp drops the response.
pub(crate) const CANCELLED: &str = "the client cancelled the request";

/// The error names this server returns so far. The names are a stable contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ErrorName {
    /// Requested input cannot be sent safely by the selected backend.
    Refused,
    /// niri's socket is unknown or couldn't be reached.
    NiriUnavailable,
    /// The Wayland display isn't served by the niri on niri's socket, so nothing that
    /// reaches the display may run.
    SessionMismatch,
    /// A request or wait passed its deadline.
    DeadlineExceeded,
    /// niri replied with an error, or with something this server can't read.
    UpstreamError,
    /// Noctalia is installed but didn't answer `status` with a JSON object.
    NoctaliaUnavailable,
    /// Another server holds the lease.
    LeaseHeld,
    /// An action tool was called without holding the lease.
    LeaseRequired,
    /// The stop flag is set.
    Stopped,
    /// The input-dirty marker says input may be stuck.
    RecoveryRequired,
    /// niri's version, its event schema or the policy file rules out acting.
    ReadOnly,
    /// The screen is locked, or its lock state is unknown.
    ScreenLocked,
    /// `launch` named a preset the policy file doesn't have.
    UnknownPreset,
    /// A pointer tool on an output setup no live test covers.
    UntestedOutputConfig,
    /// Input to a window whose `app_id` the policy denies.
    AppDenied,
    /// A keyboard tool's `expect` doesn't match the focused window.
    FocusMismatch,
    /// `type_text` over 1000 characters, or `paste` over 1 MiB.
    TextTooLong,
    /// A screenshot ref that is unknown, expired, for a changed output, or a pixel outside
    /// its image.
    RefInvalid,
    /// `shell_open` or `shell_close` named a panel outside the allowlist.
    PanelNotAllowed,
    /// `paste` couldn't save the clipboard whole, so it pasted nothing.
    ClipboardUnsaved,
    /// `screenshot` was asked to save without a `capture_dir` in the policy file.
    SaveNotEnabled,
    /// The window's application isn't on the accessibility bus, or has no window there.
    NotAccessible,
    /// Several of the application's accessible windows could be the window asked about.
    AmbiguousWindow,
    /// An element ref whose window, application or object is gone, or whose object is now
    /// something else.
    ElementStale,
    /// An element ref that is still there but can't be aimed at now: see `Unmappable`.
    ElementUnmappable,
    /// A gated `niri_action` while the policy's `unrestricted` is off.
    UnrestrictedRequired,
}

/// A failure, serialized as `{"error": <name>, "detail": <upstream detail>}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ToolError {
    #[serde(rename = "error")]
    pub(crate) name: ErrorName,
    pub(crate) detail: String,
}

impl ToolError {
    pub(crate) fn new(name: ErrorName, detail: impl Into<String>) -> Self {
        Self {
            name,
            detail: detail.into(),
        }
    }

    /// The MCP form: `isError: true`, with the error as structured content.
    pub(crate) fn into_result(self) -> CallToolResult {
        CallToolResult::structured_error(serde_json::json!(self))
    }
}

/// Why a request that changes something, to niri or Noctalia, got no answer to act on.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Unanswered {
    /// Nothing changed: the peer wasn't reached, the request wasn't sent whole, or the peer
    /// refused it.
    Refused(ToolError),
    /// Sent whole, but the reply was lost or unreadable: the peer may have carried it out.
    Lost(ToolError),
}

/// Why a tool call that takes arguments didn't succeed: the arguments don't fit the
/// desktop, which the tool reports as plain text, or a failure with a stable name.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CallError {
    InvalidArguments(String),
    Tool(ToolError),
}

impl From<ToolError> for CallError {
    fn from(error: ToolError) -> Self {
        Self::Tool(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_with_the_stable_name_and_detail() {
        let error = ToolError::new(ErrorName::NiriUnavailable, "NIRI_SOCKET is not set");
        let result = error.into_result();
        assert_eq!(result.is_error, Some(true));
        assert_eq!(
            result.structured_content,
            Some(serde_json::json!({
                "error": "niri_unavailable",
                "detail": "NIRI_SOCKET is not set"
            }))
        );
    }
}
