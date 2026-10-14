//! Keys niri takes for a physical keyboard's, in a headless run: a virtual keyboard on
//! cage, the compositor whose only window the nested niri is, so niri's winit backend gets
//! its keys as it would a host keyboard's and handles them on its physical path. A visible
//! run's parent is the user's own compositor, which the checks never send input to, so
//! there it is absent.

use std::fs::File;
use std::io::Write as _;
use std::os::fd::AsFd as _;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};

use rustix::fs::{MemfdFlags, memfd_create};
use rustix::io::Errno;
use rustix::net::{self, AddressFamily, SocketAddrUnix, SocketFlags, SocketType, socket_with};
use wayland_client::protocol::wl_callback::{self, WlCallback};
use wayland_client::protocol::wl_keyboard::KeymapFormat;
use wayland_client::protocol::wl_registry::{self, WlRegistry};
use wayland_client::protocol::wl_seat::WlSeat;
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, delegate_noop};
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1;
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1;

use crate::failure::{Context as _, Failure, Result};
use crate::session::{Session, pause};
use crate::window::dispatch_until;

/// cage's display, in the test directory, as the headless run starts it.
const CAGE_DISPLAY: &str = "cage/wayland-0";
/// For cage to take the connection, and to answer a round trip.
const DEADLINE: Duration = Duration::from_secs(2);
/// Between attempts to connect while cage's backlog is full.
const RETRY: Duration = Duration::from_millis(10);

/// Linux input event codes.
pub(super) const LEFT_SHIFT: u32 = 42;
pub(super) const CAPS_LOCK: u32 = 58;
pub(super) const NUM_LOCK: u32 = 69;
pub(super) const KEYPAD_1: u32 = 79;
pub(super) const A: u32 = 30;
pub(super) const F13: u32 = 183;

pub(super) struct Physical {
    cage: Cage,
    device: ZwpVirtualKeyboardV1,
    start: Instant,
}

/// A connection to cage, with the globals it announced.
struct Cage {
    connection: Connection,
    queue: EventQueue<State>,
    state: State,
    registry: WlRegistry,
}

#[derive(Debug, Default)]
struct State {
    /// Round trips cage has answered.
    synced: u64,
    /// Each global's name, interface and version.
    globals: Vec<(u32, String, u32)>,
}

impl Physical {
    /// The keyboard, or `None` in a visible run, which has no cage.
    pub(super) fn connect(session: &Session<'_>) -> Result<Option<Self>> {
        let display = session.test_dir().root().join(CAGE_DISPLAY);
        if !display.exists() {
            return Ok(None);
        }
        let keymap = keymap(session)?;
        let mut cage = Cage::open(&display)?;
        let seat: WlSeat = cage.bind()?;
        let manager: ZwpVirtualKeyboardManagerV1 = cage.bind()?;
        let device = manager.create_virtual_keyboard(&seat, &cage.queue.handle(), ());
        let size = u32::try_from(keymap.1).context("keymap size")?;
        device.keymap(KeymapFormat::XkbV1.into(), keymap.0.as_fd(), size);
        cage.sync()?;
        Ok(Some(Self {
            cage,
            device,
            start: Instant::now(),
        }))
    }

    /// Presses or releases `code`, once cage has it.
    pub(super) fn key(&mut self, code: u32, pressed: bool) -> Result<()> {
        let time = u32::try_from(self.start.elapsed().as_millis()).unwrap_or(u32::MAX);
        self.device.key(time, code, u32::from(pressed));
        self.cage.sync()
    }

    pub(super) fn tap(&mut self, code: u32) -> Result<()> {
        self.key(code, true)?;
        self.key(code, false)
    }
}

impl Cage {
    /// Connects to the display at `path` and reads its globals, each within the deadline.
    fn open(path: &Path) -> Result<Self> {
        let stream = connect_within(path, Instant::now() + DEADLINE)
            .context(format!("connect to cage at {}", path.display()))?;
        let connection = Connection::from_socket(stream).context("connect to cage")?;
        let queue = connection.new_event_queue();
        let registry = connection.display().get_registry(&queue.handle(), ());
        let mut cage = Self {
            connection,
            queue,
            state: State::default(),
            registry,
        };
        cage.sync().context("read cage's globals")?;
        Ok(cage)
    }

    /// Binds version 1 of the global `I`.
    fn bind<I: Proxy + 'static>(&self) -> Result<I>
    where
        State: Dispatch<I, ()>,
    {
        let interface = I::interface().name;
        let name = self
            .state
            .globals
            .iter()
            .find(|(_, offered, version)| offered == interface && *version >= 1)
            .map(|(name, _, _)| *name)
            .ok_or_else(|| Failure::new(format!("cage has no {interface}")))?;
        Ok(self.registry.bind(name, 1, &self.queue.handle(), ()))
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

/// Connects without blocking: a connect to a Unix socket waits only while the
/// listener's backlog is full, and is retried until `end`.
fn connect_within(path: &Path, end: Instant) -> Result<UnixStream> {
    let socket = socket_with(
        AddressFamily::UNIX,
        SocketType::STREAM,
        SocketFlags::CLOEXEC | SocketFlags::NONBLOCK,
        None,
    )
    .context("socket")?;
    let address = SocketAddrUnix::new(path).context("address")?;
    loop {
        match net::connect(&socket, &address) {
            Ok(()) => return Ok(UnixStream::from(socket)),
            Err(Errno::AGAIN) if Instant::now() < end => pause(RETRY),
            Err(Errno::AGAIN) => {
                return Err(Failure::new(format!(
                    "its backlog stayed full for {DEADLINE:?}"
                )));
            }
            Err(error) => return Err(Failure::new(error.to_string())),
        }
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

delegate_noop!(State: ignore WlSeat);
delegate_noop!(State: ignore ZwpVirtualKeyboardManagerV1);
delegate_noop!(State: ignore ZwpVirtualKeyboardV1);

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::net::UnixListener;
    use std::sync::mpsc;
    use std::thread;

    use rustix::net::{bind, listen};

    use super::*;

    fn socket_dir(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("harness-physical-{name}-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Opens on a thread, so a connection that blocks fails the test rather than hanging it.
    fn fails_within_the_deadline(path: &Path, expected: &str) {
        let (sender, receiver) = mpsc::channel();
        let path = path.to_owned();
        thread::spawn(move || sender.send(Cage::open(&path).map(|_| ())));
        let opened = receiver
            .recv_timeout(DEADLINE * 2)
            .expect("still opening after twice the deadline");
        let failure = opened.expect_err("a peer that never answers was opened");
        assert!(failure.to_string().contains(expected), "{failure}");
    }

    #[test]
    fn a_peer_that_takes_the_connection_and_never_answers_fails_in_time() {
        let dir = socket_dir("silent");
        let path = dir.join("wayland-0");
        let listener = UnixListener::bind(&path).unwrap();
        let peer = thread::spawn(move || listener.accept().map(|(stream, _)| stream));

        fails_within_the_deadline(&path, "read cage's globals");
        drop(peer.join());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_peer_whose_backlog_is_full_fails_in_time() {
        let dir = socket_dir("backlog");
        let path = dir.join("wayland-0");
        let listener = socket_with(
            AddressFamily::UNIX,
            SocketType::STREAM,
            SocketFlags::CLOEXEC,
            None,
        )
        .unwrap();
        bind(&listener, &SocketAddrUnix::new(&path).unwrap()).unwrap();
        listen(&listener, 0).unwrap();
        let queued = UnixStream::connect(&path).unwrap();

        fails_within_the_deadline(&path, "backlog stayed full");
        drop(queued);
        fs::remove_dir_all(dir).unwrap();
    }
}
