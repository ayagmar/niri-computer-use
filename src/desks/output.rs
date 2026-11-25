//! A headless desk's size: cage's headless output starts at 1280x720 and cage has no flag
//! for it, and niri ignores `output "winit" { mode }`, so the desk sets a custom mode on
//! cage's one head through `zwlr_output_manager_v1` before niri starts (spike, 2026-11-25).
//! The nested niri's window fills cage's output, so its output takes the size.

use std::os::fd::OwnedFd;
use std::path::Path;
use std::time::Duration;

use tokio::io::Interest;
use tokio::io::unix::AsyncFd;
use tokio::net::UnixStream;
use wayland_client::backend::WaylandError;
use wayland_client::protocol::wl_registry::{self, WlRegistry};
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle, event_created_child};
use wayland_protocols_wlr::output_management::v1::client::zwlr_output_configuration_head_v1::ZwlrOutputConfigurationHeadV1;
use wayland_protocols_wlr::output_management::v1::client::zwlr_output_configuration_v1::{
    self, ZwlrOutputConfigurationV1,
};
use wayland_protocols_wlr::output_management::v1::client::zwlr_output_head_v1::{
    self, ZwlrOutputHeadV1,
};
use wayland_protocols_wlr::output_management::v1::client::zwlr_output_manager_v1::{
    self, ZwlrOutputManagerV1,
};
use wayland_protocols_wlr::output_management::v1::client::zwlr_output_mode_v1::ZwlrOutputModeV1;

use super::record::Size;

/// How long setting the mode may take, connection included.
pub(crate) const DEADLINE: Duration = Duration::from_secs(3);

#[derive(Debug, Default)]
struct State {
    globals: Vec<(u32, String, u32)>,
    synced: bool,
    heads: Vec<ZwlrOutputHeadV1>,
    /// The serial of the heads' last `done`.
    serial: Option<u32>,
    /// Whether the configuration succeeded, once cage answered.
    applied: Option<Result<(), &'static str>>,
}

/// Sets every head of the compositor at `display` to `size`, within `DEADLINE`.
pub(crate) async fn set(display: &Path, size: Size) -> Result<(), String> {
    tokio::time::timeout(DEADLINE, configure(display, size))
        .await
        .unwrap_or_else(|_| {
            Err(format!(
                "cage didn't take the output size {size} within {DEADLINE:?}"
            ))
        })
}

async fn configure(display: &Path, size: Size) -> Result<(), String> {
    let stream = UnixStream::connect(display)
        .await
        .and_then(UnixStream::into_std)
        .map_err(|error| format!("connect to cage at {}: {error}", display.display()))?;
    let connection = Connection::from_socket(stream)
        .map_err(|error| format!("start Wayland with cage: {error}"))?;
    let fd = connection
        .backend()
        .poll_fd()
        .try_clone_to_owned()
        .map_err(|error| format!("copy cage's Wayland socket: {error}"))?;
    #[expect(
        deprecated,
        reason = "the AsyncFd owns its OwnedFd, so nothing can close or replace the descriptor while it is registered (RUSTSEC-2026-0386)"
    )]
    let readable = AsyncFd::with_interest(fd, Interest::READABLE)
        .map_err(|error| format!("watch cage's Wayland socket: {error}"))?;
    let mut queue = connection.new_event_queue();
    let handle = queue.handle();
    let registry = connection.display().get_registry(&handle, ());
    let mut state = State::default();
    connection.display().sync(&handle, ());
    until(&mut queue, &mut state, &readable, |state| state.synced).await?;
    let (name, version) = state
        .globals
        .iter()
        .find(|(_, interface, _)| interface == "zwlr_output_manager_v1")
        .map(|(name, _, version)| (*name, (*version).min(4)))
        .ok_or("cage has no zwlr_output_manager_v1, so the desk's size can't be set")?;
    let manager: ZwlrOutputManagerV1 = registry.bind(name, version, &handle, ());
    until(&mut queue, &mut state, &readable, |state| {
        state.serial.is_some()
    })
    .await?;
    let serial = state.serial.ok_or("cage sent no heads")?;
    if state.heads.is_empty() {
        return Err("cage has no output to size".to_owned());
    }
    let configuration = manager.create_configuration(serial, &handle, ());
    for head in &state.heads {
        let head = configuration.enable_head(head, &handle, ());
        head.set_custom_mode(signed(size.width)?, signed(size.height)?, 0);
    }
    configuration.apply();
    until(&mut queue, &mut state, &readable, |state| {
        state.applied.is_some()
    })
    .await?;
    let applied = state.applied.unwrap_or(Err("no answer"));
    manager.stop();
    queue.flush().ok();
    applied
        .map_err(|why| format!("cage refused the output size {size}: the configuration was {why}"))
}

fn signed(side: u32) -> Result<i32, String> {
    i32::try_from(side).map_err(|_| format!("{side} pixels is too many"))
}

/// Dispatches cage's events until `done` holds. The caller bounds the wait.
async fn until(
    queue: &mut EventQueue<State>,
    state: &mut State,
    readable: &AsyncFd<OwnedFd>,
    done: impl Fn(&State) -> bool,
) -> Result<(), String> {
    let broken = |error: &dyn std::fmt::Display| format!("cage's Wayland display: {error}");
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
        let mut ready = readable.readable().await.map_err(|error| broken(&error))?;
        match guard.read() {
            Ok(_) => {}
            Err(WaylandError::Io(error)) if error.kind() == std::io::ErrorKind::WouldBlock => {
                ready.clear_ready();
            }
            Err(error) => return Err(broken(&error)),
        }
    }
}

impl Dispatch<WlRegistry, ()> for State {
    fn event(
        state: &mut Self,
        _: &WlRegistry,
        event: wl_registry::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            state.globals.push((name, interface, version));
        }
    }
}

impl Dispatch<wayland_client::protocol::wl_callback::WlCallback, ()> for State {
    fn event(
        state: &mut Self,
        _: &wayland_client::protocol::wl_callback::WlCallback,
        _: wayland_client::protocol::wl_callback::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        state.synced = true;
    }
}

impl Dispatch<ZwlrOutputManagerV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ZwlrOutputManagerV1,
        event: zwlr_output_manager_v1::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwlr_output_manager_v1::Event::Head { head } = event {
            state.heads.push(head);
        } else if let zwlr_output_manager_v1::Event::Done { serial } = event {
            state.serial = Some(serial);
        }
    }

    event_created_child!(State, ZwlrOutputManagerV1, [
        zwlr_output_manager_v1::EVT_HEAD_OPCODE => (ZwlrOutputHeadV1, ()),
    ]);
}

impl Dispatch<ZwlrOutputHeadV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &ZwlrOutputHeadV1,
        _: zwlr_output_head_v1::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }

    event_created_child!(State, ZwlrOutputHeadV1, [
        zwlr_output_head_v1::EVT_MODE_OPCODE => (ZwlrOutputModeV1, ()),
    ]);
}

impl Dispatch<ZwlrOutputConfigurationV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ZwlrOutputConfigurationV1,
        event: zwlr_output_configuration_v1::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        state.applied = Some(match event {
            zwlr_output_configuration_v1::Event::Succeeded => Ok(()),
            zwlr_output_configuration_v1::Event::Failed => Err("failed"),
            zwlr_output_configuration_v1::Event::Cancelled => Err("cancelled"),
            _ => Err("answered with an event this build doesn't know"),
        });
    }
}

wayland_client::delegate_noop!(State: ignore ZwlrOutputModeV1);
wayland_client::delegate_noop!(State: ignore ZwlrOutputConfigurationHeadV1);
