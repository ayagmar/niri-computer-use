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
    /// The longest one round trip may take. One that takes longer fails the burst: its
    /// answer may still come, and would then count for the next round trip.
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

/// The probe's connection, and its drawing surface once it has one.
struct Probe {
    connection: Connection,
    queue: EventQueue<State>,
    state: State,
    readable: AsyncFd<OwnedFd>,
    drawing: Option<Drawing>,
}

/// The surface the probe keeps niri drawing with.
struct Drawing {
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
    let mut probe = Probe::connect(display, niri_pid).await?;
    probe.draw_on().await?;
    let samples = sample(&mut probe, burst).await?;
    probe.close();
    Ok(samples)
}

/// The burst's round trips, one after another. One that times out fails the burst.
async fn sample(probe: &mut Probe, burst: Burst) -> Result<Vec<Duration>, ToolError> {
    let mut samples = Vec::with_capacity(burst.count);
    let mut slow = 0;
    while samples.len() < burst.count && slow < burst.enough_slow {
        let taken = probe.round_trip(burst.each).await.map_err(|error| {
            ToolError::new(
                error.name,
                format!(
                    "round trip {} of the burst: {}",
                    samples.len() + 1,
                    error.detail
                ),
            )
        })?;
        slow += usize::from(taken > burst.slow);
        samples.push(taken);
        tokio::time::sleep(GAP).await;
    }
    Ok(samples)
}

impl Probe {
    /// A connection to the niri with `niri_pid` at `display`, with its globals listed.
    async fn connect(display: &Path, niri_pid: u32) -> Result<Self, ToolError> {
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
        let queue = connection.new_event_queue();
        Ok(Self {
            connection,
            queue,
            state: State::default(),
            readable,
            drawing: None,
        })
    }

    /// Puts the probe's surface on niri's background layer and draws its first frame.
    async fn draw_on(&mut self) -> Result<(), ToolError> {
        let deadline = Instant::now() + DEADLINE;
        let handle = self.queue.handle();
        let registry = self.connection.display().get_registry(&handle, ());
        let (connection, queue, state, readable) = (
            &self.connection,
            &mut self.queue,
            &mut self.state,
            &self.readable,
        );
        sync(connection, queue, state, readable, deadline).await?;
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
        dispatch_until(queue, state, readable, Some(deadline), |state| {
            state.configured.is_some() || state.closed
        })
        .await?;
        let serial = self.state.configured.ok_or_else(|| {
            upstream("niri closed the probe's layer surface before configuring it")
        })?;
        layer.ack_configure(serial);
        surface.attach(Some(&buffer), 0, 0);
        self.drawing = Some(Drawing {
            surface,
            layer,
            buffer,
        });
        self.draw();
        Ok(())
    }

    /// Damages and commits the surface, asking for the next frame callback.
    fn draw(&mut self) {
        let Some(drawing) = &self.drawing else {
            return;
        };
        self.state.frame_due = false;
        drawing.surface.damage_buffer(0, 0, SIDE, SIDE);
        drawing.surface.frame(&self.queue.handle(), Callback::Frame);
        drawing.surface.commit();
    }

    /// One round trip's time, drawing the next frame whenever one is due meanwhile. One
    /// that takes longer than `each` is an error: its answer is still outstanding.
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
            Err(error) if error.name == ErrorName::DeadlineExceeded => Err(ToolError::new(
                ErrorName::DeadlineExceeded,
                format!("niri didn't answer within {each:?}"),
            )),
            Err(error) => Err(error),
        }
    }

    fn close(self) {
        if let Some(drawing) = self.drawing {
            drawing.layer.destroy();
            drawing.surface.destroy();
            drawing.buffer.destroy();
        }
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

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    use tokio::net::UnixListener;

    use super::*;

    const EACH: Duration = Duration::from_millis(100);

    /// A display that answers each `wl_display.sync` with its callback's `done` and the
    /// `delete_id` that frees it, the first `late` after `EACH * 3`, the rest at once, in
    /// order, as a compositor does.
    fn display(socket: &Path) {
        let listener = UnixListener::bind(socket).unwrap();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            answer(stream).await;
        });
    }

    async fn answer(mut stream: tokio::net::UnixStream) {
        let mut late = true;
        let mut message = [0_u8; 12];
        while stream.read_exact(&mut message).await.is_ok() {
            let Some(callback) = synced(&message) else {
                continue;
            };
            if std::mem::take(&mut late) {
                tokio::time::sleep(EACH * 3).await;
            }
            if stream.write_all(&replies(callback)).await.is_err() {
                return;
            }
        }
    }

    /// The callback of a `wl_display.sync`: object 1, request 0, a new ID.
    fn synced(message: &[u8; 12]) -> Option<u32> {
        let word = |at: usize| u32::from_ne_bytes(message[at..at + 4].try_into().unwrap());
        let opcode = word(4) & 0xffff;
        (word(0) == 1 && opcode == 0).then(|| word(8))
    }

    /// The callback's `done`, then `wl_display.delete_id` for it.
    fn replies(callback: u32) -> Vec<u8> {
        [(callback, 0, 0), (1, 1, callback)]
            .into_iter()
            .flat_map(|(object, opcode, argument)| [object, (12 << 16) | opcode, argument])
            .flat_map(u32::to_ne_bytes)
            .collect()
    }

    /// Catches a burst that counts a round trip that timed out as slow and goes on: the
    /// late answer then arrives during the next round trip and makes it look fast, so a
    /// compositor that stalls now and then could pass. Any timeout fails the burst.
    #[tokio::test]
    async fn a_late_answer_fails_the_burst_instead_of_counting_for_the_next_round_trip() {
        let dir = crate::test_support::short_dir("sync");
        let socket = dir.join("wayland-1");
        display(&socket);
        let mut probe = Probe::connect(&socket, std::process::id()).await.unwrap();
        let burst = Burst {
            count: 10,
            slow: Duration::from_millis(5),
            enough_slow: 3,
            each: EACH,
        };
        let error = sample(&mut probe, burst).await.unwrap_err();
        assert_eq!(error.name, ErrorName::DeadlineExceeded, "{error:?}");
        assert!(
            error.detail.contains("round trip 1 of the burst"),
            "{error:?}"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
