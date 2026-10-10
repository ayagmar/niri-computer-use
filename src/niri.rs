//! The only module that talks to niri.

pub(crate) mod events;
pub(crate) mod keyboard;
pub(crate) mod pointer;
mod request;
pub(crate) mod selection;
pub(crate) mod version;
pub(crate) mod waiter;
mod wayland;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use niri_ipc::{Action, Output, Request, Response, Window, Workspace};

use crate::error::{ErrorName, ToolError, Unanswered};
use events::{DesktopState, EventStream};
use waiter::Waiter;

/// The server's event stream, or why it has none: the socket's error.
pub(crate) type Events<'a> = Result<&'a EventStream, &'a ToolError>;

/// niri's IPC socket, or why it is unknown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Socket(Result<PathBuf, ToolError>);

impl Socket {
    pub(crate) const fn at(path: PathBuf) -> Self {
        Self(Ok(path))
    }

    /// No socket, for the reason `detail` gives.
    pub(crate) fn unknown(detail: impl Into<String>) -> Self {
        Self(Err(ToolError::new(ErrorName::NiriUnavailable, detail)))
    }

    /// The socket's path, or the `niri_unavailable` error that says why there is none.
    pub(crate) fn path(&self) -> Result<&Path, &ToolError> {
        self.0.as_deref()
    }
}

impl Default for Socket {
    fn default() -> Self {
        Self::unknown("NIRI_SOCKET is not set")
    }
}

/// The Wayland display's socket, checked to be served by the niri on niri's socket, or why
/// it can't be used. wtype, grim and wl-paste find the display by name alone, so nothing
/// that reaches the display starts without this check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Display(Result<PathBuf, ToolError>);

impl Display {
    /// Checks once, within the connections' deadlines, that the niri on `socket` serves
    /// the display at `display`.
    pub(crate) async fn check(socket: &Socket, display: Result<PathBuf, String>) -> Self {
        let checked = async {
            let display =
                display.map_err(|detail| ToolError::new(ErrorName::UpstreamError, detail))?;
            wayland::niri_stream(&display, pid(socket).await?).await?;
            Ok(display)
        };
        Self(checked.await)
    }

    /// The display's socket, or the error that says why it can't be used.
    pub(crate) fn path(&self) -> Result<&Path, &ToolError> {
        self.0.as_deref()
    }
}

impl Default for Display {
    fn default() -> Self {
        Self(Err(ToolError::new(
            ErrorName::UpstreamError,
            "the Wayland display hasn't been checked against niri",
        )))
    }
}

/// niri's version string, such as `26.04 (8ed0da4)`.
pub(crate) async fn version(socket: &Socket) -> Result<String, ToolError> {
    let Response::Version(version) = request::send(known(socket)?, &Request::Version).await? else {
        return Err(unexpected("Version"));
    };
    Ok(version)
}

/// The process ID of the niri listening on the socket.
pub(crate) async fn pid(socket: &Socket) -> Result<u32, ToolError> {
    request::peer_pid(known(socket)?, request::DEADLINE).await
}

/// The process ID of whatever listens on the socket at `path`, if it accepts a connection
/// within `limit`. A socket a crashed niri left accepts none.
pub(crate) async fn listener_pid(path: &Path, limit: Duration) -> Result<u32, ToolError> {
    request::peer_pid(path, limit).await
}

/// niri's outputs by connector name, in name order.
pub(crate) async fn outputs(socket: &Socket) -> Result<BTreeMap<String, Output>, ToolError> {
    let Response::Outputs(outputs) = request::send(known(socket)?, &Request::Outputs).await? else {
        return Err(unexpected("Outputs"));
    };
    Ok(outputs.into_iter().collect())
}

/// niri's windows, as it reports them right now.
pub(crate) async fn windows(socket: &Socket) -> Result<Vec<Window>, ToolError> {
    let Response::Windows(windows) = request::send(known(socket)?, &Request::Windows).await? else {
        return Err(unexpected("Windows"));
    };
    Ok(windows)
}

/// niri's workspaces, as it reports them right now.
pub(crate) async fn workspaces(socket: &Socket) -> Result<Vec<Workspace>, ToolError> {
    let Response::Workspaces(workspaces) =
        request::send(known(socket)?, &Request::Workspaces).await?
    else {
        return Err(unexpected("Workspaces"));
    };
    Ok(workspaces)
}

/// The output with keyboard focus, if niri reports one.
pub(crate) async fn focused_output(socket: &Socket) -> Result<Option<Output>, ToolError> {
    let Response::FocusedOutput(output) =
        request::send(known(socket)?, &Request::FocusedOutput).await?
    else {
        return Err(unexpected("FocusedOutput"));
    };
    Ok(output)
}

/// One snapshot of niri's replayed state. There is no stream without niri's socket, and
/// the error says why.
pub(crate) async fn desktop(events: Events<'_>) -> Result<DesktopState, ToolError> {
    events.map_err(Clone::clone)?.desktop().await
}

/// Registers a waiter on niri's event stream, before an action is dispatched.
pub(crate) async fn waiter(events: Events<'_>) -> Result<Waiter, ToolError> {
    events.map_err(Clone::clone)?.waiter().await
}

/// Asks niri to carry out `action`. niri replies once it has.
pub(crate) async fn act(socket: &Socket, action: Action) -> Result<(), Unanswered> {
    let socket = known(socket).map_err(Unanswered::Refused)?;
    let Response::Handled = request::dispatch(socket, &Request::Action(action)).await? else {
        return Err(Unanswered::Lost(unexpected("Action")));
    };
    Ok(())
}

fn known(socket: &Socket) -> Result<&Path, ToolError> {
    socket.path().map_err(Clone::clone)
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
        let socket = Socket::unknown("no niri here");
        let not_set = ToolError::new(ErrorName::NiriUnavailable, "no niri here");
        assert_eq!(outputs(&socket).await.unwrap_err(), not_set);
        assert_eq!(desktop(Err(&not_set)).await.unwrap_err(), not_set);
    }
}
