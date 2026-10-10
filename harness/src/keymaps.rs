//! `harness keymaps`: an unfocused client that saves every keymap niri sends its
//! `wl_keyboard`, in order, as `keymap-<n>.xkb` in a directory, so a check can compare
//! the maps' contents rather than wev's format and size. Each file appears whole, by
//! rename. It exits when niri goes away or at its deadline.

use std::fs::{self, File};
use std::os::fd::OwnedFd;
use std::os::unix::fs::FileExt as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_keyboard::{self, WlKeyboard};
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::protocol::wl_seat::WlSeat;
use wayland_client::{Connection, Dispatch, QueueHandle, delegate_noop};

use crate::failure::{Context as _, Failure, Result};
use crate::nested::Nested;
use crate::test_dir::TestDir;
use crate::window::dispatch_until;

pub(crate) const USAGE: &str = "usage: harness keymaps <TEST_DIR> <directory> <deadline-ms>";
const MAX_MAP: u32 = 1024 * 1024;

#[derive(Debug)]
struct State {
    directory: PathBuf,
    saved: usize,
    error: Option<Failure>,
}

pub(crate) fn run(args: &[&str]) -> Result<()> {
    let [test_dir, directory, deadline] = args else {
        return Err(Failure::new(USAGE));
    };
    let end = Instant::now() + Duration::from_millis(deadline.parse().context(USAGE)?);
    Nested::from_env(&TestDir::open(PathBuf::from(test_dir))?)?;
    let connection = Connection::connect_to_env().context("connect to the nested niri")?;
    let (globals, mut queue) =
        registry_queue_init::<State>(&connection).context("read the globals")?;
    let seat: WlSeat = globals
        .bind(&queue.handle(), 7..=7, ())
        .context("bind wl_seat")?;
    seat.get_keyboard(&queue.handle(), ());
    let mut state = State {
        directory: PathBuf::from(directory),
        saved: 0,
        error: None,
    };
    dispatch_until(&mut queue, &mut state, end, |state| state.error.is_some())?;
    state.error.map_or(Ok(()), Err)
}

fn save(directory: &Path, index: usize, fd: OwnedFd, size: u32) -> Result<()> {
    if size == 0 || size > MAX_MAP {
        return Err(Failure::new(format!("keymap size {size}")));
    }
    let mut bytes = vec![0; usize::try_from(size).context("keymap size")?];
    File::from(fd)
        .read_exact_at(&mut bytes, 0)
        .context("read a keymap")?;
    if bytes.pop() != Some(0) {
        return Err(Failure::new("keymap without its NUL terminator"));
    }
    let partial = directory.join(format!(".keymap-{index}"));
    fs::write(&partial, bytes).context(format!("write {}", partial.display()))?;
    let path = directory.join(format!("keymap-{index}.xkb"));
    fs::rename(&partial, &path).context(format!("rename to {}", path.display()))
}

/// The maps saved so far, in the order niri sent them.
pub(crate) fn saved(directory: &Path) -> Result<Vec<Vec<u8>>> {
    let mut maps = Vec::new();
    loop {
        let path = directory.join(format!("keymap-{}.xkb", maps.len()));
        match fs::read(&path) {
            Ok(map) => maps.push(map),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(maps),
            Err(error) => return Err(Failure::new(format!("read {}: {error}", path.display()))),
        }
    }
}

impl Dispatch<WlKeyboard, ()> for State {
    fn event(
        state: &mut Self,
        _: &WlKeyboard,
        event: wl_keyboard::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let wl_keyboard::Event::Keymap { fd, size, .. } = event else {
            return;
        };
        match save(&state.directory, state.saved, fd, size) {
            Ok(()) => state.saved += 1,
            Err(error) => state.error = Some(error),
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
