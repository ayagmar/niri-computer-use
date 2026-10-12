//! Keys niri takes for a physical keyboard's, in a headless run: a virtual keyboard on
//! cage, the compositor whose only window the nested niri is, so niri's winit backend gets
//! its keys as it would a host keyboard's and handles them on its physical path. A visible
//! run's parent is the user's own compositor, which the checks never send input to, so
//! there it is absent.

use std::fs::File;
use std::io::Write as _;
use std::os::fd::AsFd as _;
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use rustix::fs::{MemfdFlags, memfd_create};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_callback::{self, WlCallback};
use wayland_client::protocol::wl_keyboard::KeymapFormat;
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::protocol::wl_seat::WlSeat;
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle, delegate_noop};
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1;
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1;

use crate::failure::{Context as _, Failure, Result};
use crate::session::Session;
use crate::window::dispatch_until;

/// cage's display, in the test directory, as the headless run starts it.
const CAGE_DISPLAY: &str = "cage/wayland-0";
/// For cage to answer a round trip.
const DEADLINE: Duration = Duration::from_secs(2);

/// Linux input event codes.
pub(super) const LEFT_SHIFT: u32 = 42;
pub(super) const CAPS_LOCK: u32 = 58;
pub(super) const A: u32 = 30;

pub(super) struct Physical {
    connection: Connection,
    queue: EventQueue<State>,
    state: State,
    device: ZwpVirtualKeyboardV1,
    start: Instant,
}

#[derive(Debug, Default)]
struct State {
    /// Round trips cage has answered.
    synced: u64,
}

impl Physical {
    /// The keyboard, or `None` in a visible run, which has no cage.
    pub(super) fn connect(session: &Session<'_>) -> Result<Option<Self>> {
        let display = session.test_dir().root().join(CAGE_DISPLAY);
        if !display.exists() {
            return Ok(None);
        }
        let keymap = keymap(session)?;
        let stream = UnixStream::connect(&display)
            .context(format!("connect to cage at {}", display.display()))?;
        let connection = Connection::from_socket(stream).context("connect to cage")?;
        let (globals, queue) =
            registry_queue_init::<State>(&connection).context("read cage's globals")?;
        let handle = queue.handle();
        let seat: WlSeat = globals
            .bind(&handle, 1..=1, ())
            .context("bind cage's wl_seat")?;
        let manager: ZwpVirtualKeyboardManagerV1 = globals
            .bind(&handle, 1..=1, ())
            .context("bind cage's zwp_virtual_keyboard_manager_v1")?;
        let device = manager.create_virtual_keyboard(&seat, &handle, ());
        let size = u32::try_from(keymap.1).context("keymap size")?;
        device.keymap(KeymapFormat::XkbV1.into(), keymap.0.as_fd(), size);
        let mut physical = Self {
            connection,
            queue,
            state: State::default(),
            device,
            start: Instant::now(),
        };
        physical.sync()?;
        Ok(Some(physical))
    }

    /// Presses or releases `code`, once cage has it.
    pub(super) fn key(&mut self, code: u32, pressed: bool) -> Result<()> {
        let time = u32::try_from(self.start.elapsed().as_millis()).unwrap_or(u32::MAX);
        self.device.key(time, code, u32::from(pressed));
        self.sync()
    }

    pub(super) fn tap(&mut self, code: u32) -> Result<()> {
        self.key(code, true)?;
        self.key(code, false)
    }

    fn sync(&mut self) -> Result<()> {
        let wanted = self.state.synced + 1;
        self.connection.display().sync(&self.queue.handle(), ());
        let end = Instant::now() + DEADLINE;
        dispatch_until(&mut self.queue, &mut self.state, end, |state| {
            state.synced >= wanted
        })?;
        if self.state.synced < wanted {
            return Err(Failure::new(format!(
                "cage didn't answer a round trip within {DEADLINE:?}"
            )));
        }
        Ok(())
    }
}

/// A US keymap, NUL-terminated, in a sealed file. niri reads only the keycodes; cage
/// and niri's winit backend need a map that compiles.
fn keymap(session: &Session<'_>) -> Result<(File, usize)> {
    let args = ["compile-keymap".into(), "--layout".into(), "us".into()];
    let mut text = session.run("xkbcli", &args)?.stdout;
    text.push(0);
    let fd =
        memfd_create("harness-physical-keymap", MemfdFlags::CLOEXEC).context("memfd_create")?;
    let mut file = File::from(fd);
    file.write_all(&text).context("write the keymap")?;
    Ok((file, text.len()))
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
        if matches!(event, wl_callback::Event::Done { .. }) {
            state.synced += 1;
        }
    }
}

impl Dispatch<WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: <WlRegistry as wayland_client::Proxy>::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

delegate_noop!(State: ignore WlSeat);
delegate_noop!(State: ignore ZwpVirtualKeyboardManagerV1);
delegate_noop!(State: ignore ZwpVirtualKeyboardV1);
