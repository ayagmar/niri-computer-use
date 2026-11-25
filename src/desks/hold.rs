//! `desk hold`: cage's one client, which keeps cage running, since cage exits when its
//! client does. It connects to cage's display and waits until cage closes the connection:
//! cage, told to stop, closes its clients and then waits for this one to exit before it
//! exits itself. If cage is killed instead, the kernel kills this process.

use std::path::PathBuf;

use tokio::io::AsyncReadExt as _;
use tokio::net::UnixStream;

use crate::runner::desk::die_with_parent_by;

/// Waits, with no deadline, for as long as cage keeps its display open.
pub(crate) async fn run(var: impl Fn(&str) -> Option<std::ffi::OsString>) -> Result<(), String> {
    die_with_parent_by(rustix::process::Signal::KILL)?;
    let (Some(dir), Some(display)) = (var("XDG_RUNTIME_DIR"), var("WAYLAND_DISPLAY")) else {
        return Err("cage gave its client no XDG_RUNTIME_DIR or WAYLAND_DISPLAY".to_owned());
    };
    let socket = PathBuf::from(dir).join(display);
    let mut stream = UnixStream::connect(&socket)
        .await
        .map_err(|error| format!("connect to cage at {}: {error}", socket.display()))?;
    // A Wayland compositor sends nothing until its client asks, so the first read ends
    // when cage closes the connection.
    let mut buffer = [0; 64];
    while matches!(stream.read(&mut buffer).await, Ok(1..)) {}
    Ok(())
}
