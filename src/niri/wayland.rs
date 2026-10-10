//! Shared deadline-bound Wayland transport. Only niri's peer may serve the display.

use std::os::fd::OwnedFd;
use std::path::Path;
use std::time::Duration;

use tokio::io::unix::AsyncFd;
use tokio::net::UnixStream;
use tokio::time::Instant;
use wayland_client::backend::WaylandError;
use wayland_client::protocol::wl_callback::WlCallback;
use wayland_client::{Connection, Dispatch, EventQueue};

use crate::error::{ErrorName, ToolError};

pub(super) const DEADLINE: Duration = Duration::from_secs(2);

pub(super) trait Synced: Dispatch<WlCallback, ()> {
    fn synced(&self) -> bool;
    fn reset(&mut self);
}

pub(super) async fn connect(display: &Path, niri_pid: u32) -> Result<Connection, ToolError> {
    let stream = tokio::time::timeout(DEADLINE, UnixStream::connect(display))
        .await
        .map_err(|_| {
            ToolError::new(
                ErrorName::DeadlineExceeded,
                format!(
                    "connect to {}: no answer within {DEADLINE:?}",
                    display.display()
                ),
            )
        })?
        .map_err(|error| upstream(&format!("connect to {}: {error}", display.display())))?;
    let peer = stream
        .peer_cred()
        .map_err(|error| upstream(&format!("read the Wayland display's credentials: {error}")))?
        .pid();
    if peer != i32::try_from(niri_pid).ok() {
        return Err(upstream(&format!(
            "the Wayland display {} is served by PID {peer:?}, not niri's PID {niri_pid}",
            display.display()
        )));
    }
    let stream = stream
        .into_std()
        .map_err(|error| upstream(&format!("use the Wayland socket: {error}")))?;
    Connection::from_socket(stream).map_err(|error| upstream(&format!("start Wayland: {error}")))
}

pub(super) async fn roundtrip<S: Synced + 'static>(
    connection: &Connection,
    queue: &mut EventQueue<S>,
    state: &mut S,
    readable: &AsyncFd<OwnedFd>,
    deadline: Instant,
) -> Result<(), ToolError> {
    state.reset();
    connection.display().sync(&queue.handle(), ());
    dispatch_until(queue, state, readable, Some(deadline), S::synced).await
}

/// Dispatches events until `done` holds, or until `deadline`, if there is one. Dropping
/// the future loses nothing: events read are queued until the next dispatch.
pub(super) async fn dispatch_until<S: 'static>(
    queue: &mut EventQueue<S>,
    state: &mut S,
    readable: &AsyncFd<OwnedFd>,
    deadline: Option<Instant>,
    done: impl Fn(&S) -> bool,
) -> Result<(), ToolError> {
    let broken =
        |error: &dyn std::fmt::Display| upstream(&format!("niri's Wayland display: {error}"));
    queue.flush().map_err(|error| broken(&error))?;
    loop {
        queue
            .dispatch_pending(state)
            .map_err(|error| broken(&error))?;
        if done(state) {
            return Ok(());
        }
        let Some(guard) = queue.prepare_read() else {
            continue;
        };
        let mut ready = until(deadline, readable.readable())
            .await?
            .map_err(|error| broken(&error))?;
        match guard.read() {
            Ok(_) => {}
            Err(WaylandError::Io(error)) if error.kind() == std::io::ErrorKind::WouldBlock => {
                ready.clear_ready();
            }
            Err(error) => return Err(broken(&error)),
        }
    }
}

async fn until<T>(
    deadline: Option<Instant>,
    work: impl Future<Output = T>,
) -> Result<T, ToolError> {
    let Some(deadline) = deadline else {
        return Ok(work.await);
    };
    tokio::time::timeout_at(deadline, work).await.map_err(|_| {
        ToolError::new(
            ErrorName::DeadlineExceeded,
            format!("niri's Wayland display didn't answer within {DEADLINE:?}"),
        )
    })
}

pub(super) fn upstream(detail: &str) -> ToolError {
    ToolError::new(ErrorName::UpstreamError, detail)
}
