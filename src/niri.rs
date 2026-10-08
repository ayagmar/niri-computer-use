//! The only module that talks to niri.

pub(crate) mod events;
mod request;
pub(crate) mod version;

use std::collections::BTreeMap;
use std::path::Path;

use niri_ipc::{Output, Request, Response};

use crate::error::{ErrorName, ToolError};
use events::{DesktopState, EventStream};

/// niri's version string, such as `26.04 (8ed0da4)`.
pub(crate) async fn version(socket: Option<&Path>) -> Result<String, ToolError> {
    let Response::Version(version) = request::send(known(socket)?, &Request::Version).await? else {
        return Err(unexpected("Version"));
    };
    Ok(version)
}

/// niri's outputs by connector name, in name order.
pub(crate) async fn outputs(socket: Option<&Path>) -> Result<BTreeMap<String, Output>, ToolError> {
    let Response::Outputs(outputs) = request::send(known(socket)?, &Request::Outputs).await? else {
        return Err(unexpected("Outputs"));
    };
    Ok(outputs.into_iter().collect())
}

/// One snapshot of niri's replayed state. There is no stream without `NIRI_SOCKET`.
pub(crate) async fn desktop(events: Option<&EventStream>) -> Result<DesktopState, ToolError> {
    events.ok_or_else(not_set)?.desktop().await
}

fn known(socket: Option<&Path>) -> Result<&Path, ToolError> {
    socket.ok_or_else(not_set)
}

fn not_set() -> ToolError {
    ToolError::new(ErrorName::NiriUnavailable, "NIRI_SOCKET is not set")
}

fn unexpected(request: &str) -> ToolError {
    ToolError::new(
        ErrorName::UpstreamError,
        format!("niri {request}: niri answered with another response type"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn without_a_socket_niri_is_unavailable() {
        let not_set = ToolError::new(ErrorName::NiriUnavailable, "NIRI_SOCKET is not set");
        assert_eq!(outputs(None).await.unwrap_err(), not_set);
        assert_eq!(desktop(None).await.unwrap_err(), not_set);
    }
}
