//! The only module that talks to niri.

pub(crate) mod events;
pub(crate) mod keyboard;
pub(crate) mod pointer;
mod request;
pub(crate) mod scene;
pub(crate) mod selection;
pub(crate) mod version;
pub(crate) mod waiter;
mod wayland;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use niri_ipc::{Action, LayerSurface, LogicalOutput, Output, Request, Response, Window, Workspace};
use serde::Serialize;

use crate::error::{ErrorName, ToolError, Unanswered};
use events::{DesktopState, EventStream};
use scene::Moves;
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

/// The Wayland display's socket, or why it is unknown. A path names no compositor for
/// good: niri can exit and another compositor take the name, so whatever reaches the
/// display checks who serves it at the moment of use, either on its own connection or,
/// for a child such as grim that finds the display by name, through
/// `checked` right before it starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Display(Result<PathBuf, ToolError>);

impl Display {
    /// The display at `path`, or none, for the reason the error gives.
    pub(crate) fn new(path: Result<PathBuf, String>) -> Self {
        Self(path.map_err(|detail| ToolError::new(ErrorName::UpstreamError, detail)))
    }

    /// The display's socket, unchecked, for a connection that checks its own peer.
    pub(crate) fn path(&self) -> Result<&Path, &ToolError> {
        self.0.as_deref()
    }

    /// The display's socket, if the niri on `socket` serves it right now, within the
    /// connections' deadlines: `niri_unavailable` once niri is gone, `session_mismatch`
    /// when another process serves the display.
    pub(crate) async fn checked(&self, socket: &Socket) -> Result<&Path, ToolError> {
        let display = self.path().map_err(Clone::clone)?;
        wayland::niri_stream(display, pid(socket).await?).await?;
        Ok(display)
    }
}

impl Default for Display {
    fn default() -> Self {
        Self::new(Err("WAYLAND_DISPLAY is not set".to_owned()))
    }
}

/// niri's version string, such as `26.04 (8ed0da4)`.
pub(crate) async fn version(socket: &Socket) -> Result<String, ToolError> {
    let Response::Version(version) = request::send(known(socket)?, &Request::Version).await? else {
        return Err(unexpected("Version"));
    };
    Ok(version)
}

/// The longest `pid` takes.
pub(crate) const PID_LIMIT: Duration = request::DEADLINE;

/// The process ID of the niri listening on the socket, within `PID_LIMIT`.
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

/// An output as `desktop_state` lists it: its connector name, what it is, and where it sits
/// in the layout, without niri's modes and hardware details.
#[derive(Debug, PartialEq, Serialize)]
pub(crate) struct OutputSummary {
    name: String,
    make: String,
    model: String,
    /// Position, logical size, scale and transform; null while the output is off.
    logical: Option<LogicalOutput>,
}

impl From<Output> for OutputSummary {
    fn from(output: Output) -> Self {
        Self {
            name: output.name,
            make: output.make,
            model: output.model,
            logical: output.logical,
        }
    }
}

/// The keyboard layout niri has active right now, as an index into its layouts: the one
/// the user's own keys type in. A virtual keyboard's group doesn't move it.
pub(crate) async fn keyboard_group(socket: &Socket) -> Result<u32, ToolError> {
    let Response::KeyboardLayouts(layouts) =
        request::send(known(socket)?, &Request::KeyboardLayouts).await?
    else {
        return Err(unexpected("KeyboardLayouts"));
    };
    Ok(u32::from(layouts.current_idx))
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

/// The window with keyboard focus, as niri reports it right now: `None` when focus isn't on
/// a window. niri answers from the state it refreshed before it last flushed its clients, so
/// the answer is at least as new as any Wayland reply a client got before asking, which its
/// event stream may not have caught up with yet.
pub(crate) async fn focused_window(socket: &Socket) -> Result<Option<u64>, ToolError> {
    let Response::FocusedWindow(window) =
        request::send(known(socket)?, &Request::FocusedWindow).await?
    else {
        return Err(unexpected("FocusedWindow"));
    };
    Ok(window.map(|window| window.id))
}

/// niri's layer-shell surfaces, such as bars and shell panels, as it reports them right now.
pub(crate) async fn layers(socket: &Socket) -> Result<Vec<LayerSurface>, ToolError> {
    let Response::Layers(layers) = request::send(known(socket)?, &Request::Layers).await? else {
        return Err(unexpected("Layers"));
    };
    Ok(layers)
}

/// Whether niri's overview is open, as niri reports it right now.
pub(crate) async fn overview_open(socket: &Socket) -> Result<bool, ToolError> {
    let Response::OverviewState(overview) =
        request::send(known(socket)?, &Request::OverviewState).await?
    else {
        return Err(unexpected("OverviewState"));
    };
    Ok(overview.is_open)
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

/// When the windows on `output` last moved, and how, on which connection
/// (`EventStream::shifted`).
pub(crate) async fn shifted(events: Events<'_>, output: &str) -> Result<Moves, ToolError> {
    events.map_err(Clone::clone)?.shifted(output).await
}

/// Registers a waiter on niri's event stream, before an action is dispatched.
pub(crate) async fn waiter(events: Events<'_>) -> Result<Waiter, ToolError> {
    events.map_err(Clone::clone)?.waiter().await
}

/// Asks niri to carry out `action`, calling `sending` right before the request is written,
/// which may still refuse it.
/// niri replies once it has.
pub(crate) async fn act(
    socket: &Socket,
    action: Action,
    sending: impl FnOnce() -> Result<(), ToolError>,
) -> Result<(), Unanswered> {
    let socket = known(socket).map_err(Unanswered::Refused)?;
    let Response::Handled = request::dispatch(socket, &Request::Action(action), sending).await?
    else {
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
