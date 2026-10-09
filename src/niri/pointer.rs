//! A virtual pointer (`zwlr_virtual_pointer_v1`), bound to one output, on a Wayland
//! connection of its own to niri. niri maps its absolute motion onto that output (plan
//! §8). Every round trip has a deadline, and dropping the pointer releases the buttons it
//! still holds before destroying it: niri releases nothing when a device goes (M0, C8).

use std::os::fd::OwnedFd;
use std::path::Path;
use std::time::Duration;

use rustix::time::{ClockId, clock_gettime};
use tokio::io::Interest;
use tokio::io::unix::AsyncFd;
use tokio::net::UnixStream;
use tokio::time::Instant;
use wayland_client::backend::WaylandError;
use wayland_client::protocol::wl_callback::{self, WlCallback};
use wayland_client::protocol::wl_output::{self, WlOutput};
use wayland_client::protocol::wl_pointer::{self, AxisSource, ButtonState};
use wayland_client::protocol::wl_registry::{self, WlRegistry};
use wayland_client::protocol::wl_seat::WlSeat;
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle, delegate_noop};
use wayland_protocols_wlr::virtual_pointer::v1::client::zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1;
use wayland_protocols_wlr::virtual_pointer::v1::client::zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1;

use crate::coords::ProtocolPt;
use crate::error::{ErrorName, ToolError};

/// For connecting and for each round trip.
const DEADLINE: Duration = Duration::from_secs(2);
/// niri's own scroll distance per wheel notch (plan §3), which C12 confirmed.
const PER_NOTCH: f64 = 15.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Axis {
    Vertical,
    Horizontal,
}

/// One piece of input, sent as one pointer frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Step {
    Motion(ProtocolPt),
    /// An evdev button code, such as 272 for the left button.
    Press(u32),
    Release(u32),
    /// Notches of a wheel; positive is down or right.
    Wheel(Axis, i32),
}

/// The requests one step becomes, in order.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Wire {
    MotionAbsolute(ProtocolPt),
    Button(u32, ButtonState),
    AxisDiscrete(Axis, f64, i32),
    AxisSourceWheel,
    Frame,
}

/// `axis_source` comes after `axis_discrete`: niri applies it only to the frame that
/// `axis_discrete` has started (plan §3, C12).
fn wire(step: Step) -> Vec<Wire> {
    let request = match step {
        Step::Motion(point) => Wire::MotionAbsolute(point),
        Step::Press(button) => Wire::Button(button, ButtonState::Pressed),
        Step::Release(button) => Wire::Button(button, ButtonState::Released),
        Step::Wheel(axis, notches) => {
            return vec![
                Wire::AxisDiscrete(axis, PER_NOTCH * f64::from(notches), notches),
                Wire::AxisSourceWheel,
                Wire::Frame,
            ];
        }
    };
    vec![request, Wire::Frame]
}

/// What the connection's events have told so far.
#[derive(Debug, Default)]
struct State {
    /// The registry's globals: name, interface and version.
    globals: Vec<(u32, String, u32)>,
    outputs: Vec<(WlOutput, String)>,
    synced: bool,
}

#[derive(Debug)]
pub(crate) struct Pointer {
    connection: Connection,
    queue: EventQueue<State>,
    state: State,
    readable: AsyncFd<OwnedFd>,
    device: ZwlrVirtualPointerV1,
    /// Buttons pressed and not released yet.
    pressed: Vec<u32>,
    /// Whether any step has reached the socket.
    sent: bool,
}

impl Pointer {
    /// Connects to the Wayland display at `display`, checks that `niri_pid` serves it, and
    /// creates a virtual pointer bound to the output named `output`.
    pub(crate) async fn bind(
        display: &Path,
        niri_pid: u32,
        output: &str,
    ) -> Result<Self, ToolError> {
        let deadline = Instant::now() + DEADLINE;
        let connection = connect(display, niri_pid, deadline).await?;
        let fd = connection
            .backend()
            .poll_fd()
            .try_clone_to_owned()
            .map_err(|error| upstream(&format!("copy the Wayland socket: {error}")))?;
        let readable = AsyncFd::with_interest(fd, Interest::READABLE)
            .map_err(|error| upstream(&format!("watch the Wayland socket: {error}")))?;
        let mut queue = connection.new_event_queue();
        let handle = queue.handle();
        let registry = connection.display().get_registry(&handle, ());
        let mut state = State::default();
        roundtrip(&connection, &mut queue, &mut state, &readable, deadline).await?;
        let (seat, manager) = bind_globals(&registry, &state, &handle)?;
        roundtrip(&connection, &mut queue, &mut state, &readable, deadline).await?;
        let found = state
            .outputs
            .iter()
            .find(|(_, name)| name == output)
            .map(|(found, _)| found)
            .ok_or_else(|| upstream(&format!("niri's Wayland display has no output {output:?}")))?;
        let device =
            manager.create_virtual_pointer_with_output(Some(&seat), Some(found), &handle, ());
        Ok(Self {
            connection,
            queue,
            state,
            readable,
            device,
            pressed: Vec::new(),
            sent: false,
        })
    }

    /// Sends `step` as one frame.
    pub(crate) fn send(&mut self, step: Step) -> Result<(), ToolError> {
        let time = now_ms();
        for request in wire(step) {
            match request {
                Wire::MotionAbsolute(at) => {
                    self.device
                        .motion_absolute(time, at.x, at.y, at.x_extent, at.y_extent);
                }
                Wire::Button(button, state) => self.device.button(time, button, state),
                Wire::AxisDiscrete(axis, value, notches) => {
                    self.device.axis_discrete(time, axis.into(), value, notches);
                }
                Wire::AxisSourceWheel => self.device.axis_source(AxisSource::Wheel),
                Wire::Frame => self.device.frame(),
            }
        }
        let flushed = self
            .queue
            .flush()
            .map_err(|error| upstream(&format!("send to niri's Wayland display: {error}")));
        self.sent |= flushed.is_ok();
        // A press counts as held even if it may not have reached niri; a release counts
        // only once it has reached the socket.
        match step {
            Step::Press(button) => self.pressed.push(button),
            Step::Release(button) if flushed.is_ok() => {
                self.pressed.retain(|held| *held != button);
            }
            Step::Release(_) | Step::Motion(_) | Step::Wheel(..) => {}
        }
        flushed
    }

    /// Whether any step has reached niri's socket, so that niri may have acted on it.
    pub(crate) const fn sent(&self) -> bool {
        self.sent
    }

    /// Waits until niri has handled everything sent so far.
    pub(crate) async fn sync(&mut self) -> Result<(), ToolError> {
        let deadline = Instant::now() + DEADLINE;
        roundtrip(
            &self.connection,
            &mut self.queue,
            &mut self.state,
            &self.readable,
            deadline,
        )
        .await
    }

    /// Whether a button counts as pressed: sent, and its release not yet sent.
    pub(crate) const fn holding(&self) -> bool {
        !self.pressed.is_empty()
    }

    /// Releases every button still pressed. Returns whether the releases reached the
    /// socket.
    pub(crate) fn release_all(&mut self) -> bool {
        for button in self.pressed.clone() {
            // The release is sent even after an earlier one failed.
            self.send(Step::Release(button)).ok();
        }
        self.pressed.is_empty()
    }
}

impl Drop for Pointer {
    fn drop(&mut self) {
        self.release_all();
        self.device.destroy();
        // Nothing more can be done if niri is gone.
        self.queue.flush().ok();
    }
}

/// Connects to `display` and checks that the process serving it is niri.
async fn connect(
    display: &Path,
    niri_pid: u32,
    deadline: Instant,
) -> Result<Connection, ToolError> {
    let stream = tokio::time::timeout_at(deadline, UnixStream::connect(display))
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

/// Binds the seat, the virtual pointer manager (version 2, which binds a pointer to an
/// output) and every output that reports its name (version 4).
fn bind_globals(
    registry: &WlRegistry,
    state: &State,
    handle: &QueueHandle<State>,
) -> Result<(WlSeat, ZwlrVirtualPointerManagerV1), ToolError> {
    let find = |interface: &str, version: u32| {
        state
            .globals
            .iter()
            .find(|(_, name, offered)| name == interface && *offered >= version)
            .map(|(id, ..)| *id)
            .ok_or_else(|| upstream(&format!("niri offers no {interface} version {version}")))
    };
    let seat = registry.bind::<WlSeat, _, _>(find("wl_seat", 1)?, 1, handle, ());
    let manager = registry.bind::<ZwlrVirtualPointerManagerV1, _, _>(
        find("zwlr_virtual_pointer_manager_v1", 2)?,
        2,
        handle,
        (),
    );
    for (id, interface, version) in &state.globals {
        if interface == "wl_output" && *version >= 4 {
            registry.bind::<WlOutput, _, _>(*id, 4, handle, ());
        }
    }
    Ok((seat, manager))
}

/// Asks for a callback and dispatches events until it arrives or `deadline` passes.
async fn roundtrip(
    connection: &Connection,
    queue: &mut EventQueue<State>,
    state: &mut State,
    readable: &AsyncFd<OwnedFd>,
    deadline: Instant,
) -> Result<(), ToolError> {
    state.synced = false;
    connection.display().sync(&queue.handle(), ());
    let broken =
        |error: &dyn std::fmt::Display| upstream(&format!("niri's Wayland display: {error}"));
    queue.flush().map_err(|error| broken(&error))?;
    loop {
        queue
            .dispatch_pending(state)
            .map_err(|error| broken(&error))?;
        if state.synced {
            return Ok(());
        }
        let Some(guard) = queue.prepare_read() else {
            continue;
        };
        let mut ready = tokio::time::timeout_at(deadline, readable.readable())
            .await
            .map_err(|_| {
                ToolError::new(
                    ErrorName::DeadlineExceeded,
                    format!("niri's Wayland display didn't answer within {DEADLINE:?}"),
                )
            })?
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

/// The event time: milliseconds on the monotonic clock, as input devices report it,
/// wrapping like Wayland's 32-bit times.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "Wayland event times are milliseconds modulo 2^32"
)]
fn now_ms() -> u32 {
    let now = clock_gettime(ClockId::Monotonic);
    (now.tv_sec as u64)
        .wrapping_mul(1000)
        .wrapping_add(now.tv_nsec as u64 / 1_000_000) as u32
}

fn upstream(detail: &str) -> ToolError {
    ToolError::new(ErrorName::UpstreamError, detail)
}

impl From<Axis> for wl_pointer::Axis {
    fn from(axis: Axis) -> Self {
        match axis {
            Axis::Vertical => Self::VerticalScroll,
            Axis::Horizontal => Self::HorizontalScroll,
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

impl Dispatch<WlOutput, ()> for State {
    fn event(
        state: &mut Self,
        output: &WlOutput,
        event: wl_output::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_output::Event::Name { name } = event {
            state.outputs.push((output.clone(), name));
        }
    }
}

impl Dispatch<WlCallback, ()> for State {
    fn event(
        state: &mut Self,
        _: &WlCallback,
        event: wl_callback::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_callback::Event::Done { .. } = event {
            state.synced = true;
        }
    }
}

delegate_noop!(State: ignore WlSeat);
delegate_noop!(State: ZwlrVirtualPointerManagerV1);
delegate_noop!(State: ZwlrVirtualPointerV1);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wheel_notch_is_discrete_then_its_source_then_the_frame() {
        assert_eq!(
            wire(Step::Wheel(Axis::Vertical, 2)),
            [
                Wire::AxisDiscrete(Axis::Vertical, 30.0, 2),
                Wire::AxisSourceWheel,
                Wire::Frame
            ]
        );
        assert_eq!(
            wire(Step::Wheel(Axis::Horizontal, -1)),
            [
                Wire::AxisDiscrete(Axis::Horizontal, -15.0, -1),
                Wire::AxisSourceWheel,
                Wire::Frame
            ]
        );
        assert_eq!(
            wire(Step::Press(272)),
            [Wire::Button(272, ButtonState::Pressed), Wire::Frame]
        );
    }
}
