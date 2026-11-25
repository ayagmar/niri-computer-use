//! `wl_display.sync` round trips to niri while niri redraws every frame, for a desk's
//! readiness check (`desks::check`). niri answers a request only between its swaps, so a
//! swap that waits for the parent's frame shows here. An idle niri doesn't swap, so the
//! probe keeps it drawing: a transparent 64x64 surface on the background layer, damaged and
//! committed again on every frame callback. Nothing sees it, and it ends with the probe.

use std::os::fd::{AsFd as _, OwnedFd};
use std::path::Path;
use std::time::Duration;

use tokio::io::Interest;
use tokio::io::unix::AsyncFd;
use tokio::time::Instant;
use wayland_client::protocol::wl_buffer::WlBuffer;
use wayland_client::protocol::wl_callback::{self, WlCallback};
use wayland_client::protocol::wl_compositor::WlCompositor;
use wayland_client::protocol::wl_registry::{self, WlRegistry};
use wayland_client::protocol::wl_shm::{self, WlShm};
use wayland_client::protocol::wl_shm_pool::WlShmPool;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle, delegate_noop};
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::{self, ZwlrLayerShellV1};
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::{
    self, ZwlrLayerSurfaceV1,
};

use super::wayland::{DEADLINE, connect, dispatch_until, upstream};
use crate::error::{ErrorName, ToolError};

const SIDE: i32 = 64;
const STRIDE: i32 = SIDE * 4;
/// The gap between round trips, so they fall anywhere in niri's frames.
const GAP: Duration = Duration::from_millis(3);

/// What one burst asks for.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Burst {
    pub(crate) count: usize,
    /// A round trip slower than this is slow.
    pub(crate) slow: Duration,
    /// The burst ends early once this many are slow: the verdict is known by then.
    pub(crate) enough_slow: usize,
    /// The longest one round trip may take; one that takes longer ends the burst with its
    /// time as this.
    pub(crate) each: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Callback {
    Sync,
    Frame,
}

#[derive(Debug, Default)]
struct State {
    globals: Vec<(u32, String, u32)>,
    synced: bool,
    configured: Option<u32>,
    closed: bool,
    /// A frame callback came, so the next frame is due.
    frame_due: bool,
}

impl State {
    fn global(&self, interface: &str) -> Option<(u32, u32)> {
        self.globals
            .iter()
            .find(|(_, name, _)| name == interface)
            .map(|(name, _, version)| (*name, *version))
    }
}

/// The probe's connection and its drawing surface.
struct Probe {
    connection: Connection,
    queue: EventQueue<State>,
    state: State,
    readable: AsyncFd<OwnedFd>,
    surface: WlSurface,
    layer: ZwlrLayerSurfaceV1,
    buffer: WlBuffer,
}

/// Measures `burst.count` round trips to the niri with `niri_pid` at `display` while it
/// redraws, unless `burst.enough_slow` of them are slow first.
pub(crate) async fn measure(
    display: &Path,
    niri_pid: u32,
    burst: Burst,
) -> Result<Vec<Duration>, ToolError> {
    let mut probe = Probe::open(display, niri_pid).await?;
    let mut samples = Vec::with_capacity(burst.count);
    let mut slow = 0;
    while samples.len() < burst.count && slow < burst.enough_slow {
        let taken = probe.round_trip(burst.each).await?;
        slow += usize::from(taken > burst.slow);
        samples.push(taken);
        tokio::time::sleep(GAP).await;
    }
    probe.close();
    Ok(samples)
}

impl Probe {
    async fn open(display: &Path, niri_pid: u32) -> Result<Self, ToolError> {
        let deadline = Instant::now() + DEADLINE;
        let connection = connect(display, niri_pid).await?;
        let fd = connection
            .backend()
            .poll_fd()
            .try_clone_to_owned()
            .map_err(|error| upstream(&format!("copy the Wayland socket: {error}")))?;
        #[expect(
            deprecated,
            reason = "the AsyncFd owns its OwnedFd, so nothing can close or replace the descriptor while it is registered (RUSTSEC-2026-0386)"
        )]
        let readable = AsyncFd::with_interest(fd, Interest::READABLE)
            .map_err(|error| upstream(&format!("watch the Wayland socket: {error}")))?;
        let mut queue = connection.new_event_queue();
        let handle = queue.handle();
        let registry = connection.display().get_registry(&handle, ());
        let mut state = State::default();
        sync(&connection, &mut queue, &mut state, &readable, deadline).await?;
        let bind = |interface: &str, most: u32| {
            state
                .global(interface)
                .map(|(name, version)| (name, version.min(most)))
                .ok_or_else(|| upstream(&format!("niri's Wayland display has no {interface}")))
        };
        let (compositor_name, compositor_version) = bind("wl_compositor", 4)?;
        let compositor: WlCompositor =
            registry.bind(compositor_name, compositor_version, &handle, ());
        let (shm_name, shm_version) = bind("wl_shm", 1)?;
        let shm: WlShm = registry.bind(shm_name, shm_version, &handle, ());
        let (shell_name, shell_version) = bind("zwlr_layer_shell_v1", 1)?;
        let shell: ZwlrLayerShellV1 = registry.bind(shell_name, shell_version, &handle, ());
        let buffer = transparent(&shm, &handle)?;
        let surface = compositor.create_surface(&handle, ());
        let layer = shell.get_layer_surface(
            &surface,
            None,
            zwlr_layer_shell_v1::Layer::Background,
            "niri-computer-use-probe".to_owned(),
            &handle,
            (),
        );
        layer.set_size(SIDE.unsigned_abs(), SIDE.unsigned_abs());
        layer.set_anchor(zwlr_layer_surface_v1::Anchor::Top | zwlr_layer_surface_v1::Anchor::Left);
        surface.commit();
        dispatch_until(&mut queue, &mut state, &readable, Some(deadline), |state| {
            state.configured.is_some() || state.closed
        })
        .await?;
        let mut probe = Self {
            connection,
            queue,
            state,
            readable,
            surface,
            layer,
            buffer,
        };
        let serial = probe.state.configured.ok_or_else(|| {
            upstream("niri closed the probe's layer surface before configuring it")
        })?;
        probe.layer.ack_configure(serial);
        probe.surface.attach(Some(&probe.buffer), 0, 0);
        probe.draw();
        Ok(probe)
    }

    /// Damages and commits the surface, asking for the next frame callback.
    fn draw(&mut self) {
        self.state.frame_due = false;
        self.surface.damage_buffer(0, 0, SIDE, SIDE);
        self.surface.frame(&self.queue.handle(), Callback::Frame);
        self.surface.commit();
    }

    /// One round trip's time, drawing the next frame whenever one is due meanwhile. One
    /// slower than `each` counts as `each`.
    async fn round_trip(&mut self, each: Duration) -> Result<Duration, ToolError> {
        if self.state.frame_due {
            self.draw();
        }
        self.state.synced = false;
        self.connection
            .display()
            .sync(&self.queue.handle(), Callback::Sync);
        let start = Instant::now();
        let waited = dispatch_until(
            &mut self.queue,
            &mut self.state,
            &self.readable,
            Some(start + each),
            |state| state.synced || state.closed,
        )
        .await;
        match waited {
            Ok(()) if self.state.closed => Err(upstream("niri closed the probe's layer surface")),
            Ok(()) => Ok(start.elapsed()),
            Err(error) if error.name == ErrorName::DeadlineExceeded => Ok(each),
            Err(error) => Err(error),
        }
    }

    fn close(self) {
        self.layer.destroy();
        self.surface.destroy();
        self.buffer.destroy();
        self.queue.flush().ok();
    }
}

async fn sync(
    connection: &Connection,
    queue: &mut EventQueue<State>,
    state: &mut State,
    readable: &AsyncFd<OwnedFd>,
    deadline: Instant,
) -> Result<(), ToolError> {
    state.synced = false;
    connection.display().sync(&queue.handle(), Callback::Sync);
    dispatch_until(queue, state, readable, Some(deadline), |state| state.synced).await
}

/// A fully transparent buffer: a zeroed shared-memory file.
fn transparent(shm: &WlShm, handle: &QueueHandle<State>) -> Result<WlBuffer, ToolError> {
    let size = STRIDE * SIDE;
    let file = rustix::fs::memfd_create("niri-computer-use-probe", rustix::fs::MemfdFlags::CLOEXEC)
        .map_err(|error| upstream(&format!("make the probe's buffer: {error}")))?;
    rustix::fs::ftruncate(&file, u64::from(size.unsigned_abs()))
        .map_err(|error| upstream(&format!("size the probe's buffer: {error}")))?;
    let pool = shm.create_pool(file.as_fd(), size, handle, ());
    let buffer = pool.create_buffer(0, SIDE, SIDE, STRIDE, wl_shm::Format::Argb8888, handle, ());
    pool.destroy();
    Ok(buffer)
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

impl Dispatch<WlCallback, Callback> for State {
    fn event(
        state: &mut Self,
        _: &WlCallback,
        event: wl_callback::Event,
        callback: &Callback,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if matches!(event, wl_callback::Event::Done { .. }) {
            match callback {
                Callback::Sync => state.synced = true,
                Callback::Frame => state.frame_due = true,
            }
        }
    }
}

impl Dispatch<ZwlrLayerSurfaceV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwlr_layer_surface_v1::Event::Configure { serial, .. } = event {
            state.configured = Some(serial);
        } else if matches!(event, zwlr_layer_surface_v1::Event::Closed) {
            state.closed = true;
        }
    }
}

delegate_noop!(State: ignore WlCompositor);
delegate_noop!(State: ignore WlShm);
delegate_noop!(State: ignore WlShmPool);
delegate_noop!(State: ignore WlBuffer);
delegate_noop!(State: ignore WlSurface);
delegate_noop!(State: ignore ZwlrLayerShellV1);
