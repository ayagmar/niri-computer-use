//! An experimental virtual keyboard with the compositor's keymap, or for one call an
//! extended copy of it. niri sends a virtual keyboard's map to every `wl_keyboard` when
//! the device next sends input, this one's included, so this keyboard sees what clients
//! were sent. Destruction does not release input in niri; callers must acknowledge
//! explicit releases.

use std::fs::File;
use std::io::Write as _;
use std::os::fd::{AsFd as _, OwnedFd};
use std::os::unix::fs::FileExt as _;
use std::path::Path;

use rustix::fs::{MemfdFlags, memfd_create};
use tokio::io::{Interest, unix::AsyncFd};
use tokio::time::Instant;
use wayland_client::protocol::{
    wl_callback::{self, WlCallback},
    wl_keyboard::{self, WlKeyboard},
    wl_registry::{self, WlRegistry},
    wl_seat::WlSeat,
};
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle, WEnum, delegate_noop};
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::{
    zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1,
    zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1,
};

use super::wayland::{DEADLINE, Synced, connect, roundtrip, upstream};
use crate::error::ToolError;

const MAX_MAP: u32 = 1024 * 1024;

#[derive(Debug, Default)]
struct State {
    globals: Vec<(u32, String, u32)>,
    synced: bool,
    /// The compositor's map when this keyboard was bound.
    map: Option<(File, String, u32)>,
    error: Option<String>,
    /// Counts maps niri sent that are neither the compositor's nor this device's.
    revision: u64,
    /// The extended map this device uploaded, until it uploads the compositor's again.
    extended: Option<String>,
    /// Whether niri sent a map other than the compositor's while an extension was
    /// uploaded, and hasn't sent the compositor's byte for byte since.
    unrestored: bool,
}

#[derive(Debug)]
pub(crate) struct Keyboard {
    connection: Connection,
    queue: EventQueue<State>,
    state: State,
    readable: AsyncFd<OwnedFd>,
    device: ZwpVirtualKeyboardV1,
    pressed: Vec<u32>,
}

impl Keyboard {
    pub(crate) async fn bind(display: &Path, pid: u32) -> Result<Self, ToolError> {
        let connection = connect(display, pid).await?;
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
        roundtrip(
            &connection,
            &mut queue,
            &mut state,
            &readable,
            Instant::now() + DEADLINE,
        )
        .await?;
        let find = |name: &str, minimum: u32| {
            state
                .globals
                .iter()
                .find(|(_, interface, version)| interface == name && *version >= minimum)
                .map(|(id, ..)| *id)
                .ok_or_else(|| upstream(&format!("niri offers no {name}")))
        };
        let seat = registry.bind::<WlSeat, _, _>(find("wl_seat", 7)?, 7, &handle, ());
        seat.get_keyboard(&handle, ());
        let manager = registry.bind::<ZwpVirtualKeyboardManagerV1, _, _>(
            find("zwp_virtual_keyboard_manager_v1", 1)?,
            1,
            &handle,
            (),
        );
        roundtrip(
            &connection,
            &mut queue,
            &mut state,
            &readable,
            Instant::now() + DEADLINE,
        )
        .await?;
        let (file, _, size) = state.map.as_ref().ok_or_else(|| {
            upstream(
                state
                    .error
                    .as_deref()
                    .unwrap_or("niri supplied no keyboard keymap"),
            )
        })?;
        let device = manager.create_virtual_keyboard(&seat, &handle, ());
        device.keymap(1, file.as_fd(), *size);
        Ok(Self {
            connection,
            queue,
            state,
            readable,
            device,
            pressed: Vec::new(),
        })
    }

    pub(crate) fn map(&self) -> Result<&str, ToolError> {
        self.state
            .map
            .as_ref()
            .map(|(_, text, _)| text.as_str())
            .ok_or_else(|| upstream("the keyboard map is unavailable"))
    }

    pub(crate) const fn revision(&self) -> u64 {
        self.state.revision
    }

    /// Uploads `map` for the following keys. niri sends it to clients with the next key.
    pub(crate) fn extend(&mut self, map: &str) -> Result<(), ToolError> {
        let fd = memfd_create("niri-computer-use-keymap", MemfdFlags::CLOEXEC)
            .map_err(|error| upstream(&format!("create the extended keymap: {error}")))?;
        let mut file = File::from(fd);
        file.write_all(map.as_bytes())
            .and_then(|()| file.write_all(&[0]))
            .map_err(|error| upstream(&format!("write the extended keymap: {error}")))?;
        let size = u32::try_from(map.len() + 1)
            .ok()
            .filter(|size| *size <= MAX_MAP)
            .ok_or_else(|| upstream("the extended keymap is too large"))?;
        self.device.keymap(1, file.as_fd(), size);
        self.state.extended = Some(map.to_owned());
        self.flush()
    }

    /// Uploads the compositor's map again after `extend`, and sends zero modifiers so niri
    /// sends it to clients. `restored` tells after a sync whether niri did.
    pub(crate) fn restore_now(&mut self, group: u32) -> Result<(), ToolError> {
        if self.state.extended.take().is_none() {
            return Ok(());
        }
        let (file, _, size) = self
            .state
            .map
            .as_ref()
            .ok_or_else(|| upstream("the keyboard map is unavailable"))?;
        self.device.keymap(1, file.as_fd(), *size);
        self.device.modifiers(0, 0, 0, group);
        self.flush()
    }

    /// Whether clients hold the compositor's map: since an extension was uploaded, niri
    /// sent no other map, or sent the compositor's byte for byte after it.
    pub(crate) const fn restored(&self) -> bool {
        self.state.restored()
    }

    /// `restore_now`, then the proof `restored` gives.
    pub(crate) async fn restore(&mut self, group: u32) -> Result<(), ToolError> {
        self.restore_now(group)?;
        self.sync().await?;
        if self.restored() {
            return Ok(());
        }
        Err(upstream(
            "niri didn't send the compositor's keymap back after the extended one",
        ))
    }

    pub(crate) fn key(&mut self, code: u32, pressed: bool) -> Result<(), ToolError> {
        if pressed && !self.pressed.contains(&code) {
            self.pressed.push(code);
        }
        self.device.key(0, code, u32::from(pressed));
        self.flush()?;
        if !pressed {
            self.pressed.retain(|held| *held != code);
        }
        Ok(())
    }

    pub(crate) fn modifiers(&self, depressed: u32, group: u32) -> Result<(), ToolError> {
        self.device.modifiers(depressed, 0, 0, group);
        self.flush()
    }

    fn flush(&self) -> Result<(), ToolError> {
        self.queue
            .flush()
            .map_err(|error| upstream(&format!("send keyboard input to niri: {error}")))
    }

    pub(crate) async fn sync(&mut self) -> Result<(), ToolError> {
        roundtrip(
            &self.connection,
            &mut self.queue,
            &mut self.state,
            &self.readable,
            Instant::now() + DEADLINE,
        )
        .await?;
        if let Some(error) = &self.state.error {
            return Err(upstream(error));
        }
        Ok(())
    }

    pub(crate) async fn release(&mut self, codes: &[u32], group: u32) -> Result<(), ToolError> {
        self.release_now(codes, group)?;
        self.sync().await
    }

    pub(crate) fn release_now(&mut self, codes: &[u32], group: u32) -> Result<(), ToolError> {
        for &code in codes.iter().chain(&self.pressed).rev() {
            self.device.key(0, code, 0);
        }
        self.device.modifiers(0, 0, 0, group);
        self.flush()?;
        self.pressed.clear();
        Ok(())
    }
}

impl Drop for Keyboard {
    fn drop(&mut self) {
        self.device.destroy();
        self.queue.flush().ok();
    }
}

impl State {
    const fn restored(&self) -> bool {
        !self.unrestored
    }

    /// Any map but the compositor's counts while an extension is uploaded or unrestored,
    /// even one that differs from the extension only in serialization. Maps other than
    /// the compositor's and the extension also count as compositor keymap changes.
    fn received(&mut self, map: (File, String, u32)) {
        let Some((_, original, _)) = &self.map else {
            self.map = Some(map);
            return;
        };
        if *original == map.1 {
            self.unrestored = false;
            return;
        }
        self.unrestored |= self.extended.is_some();
        if self.extended.as_ref() != Some(&map.1) {
            self.revision += 1;
        }
    }
}

impl Synced for State {
    fn synced(&self) -> bool {
        self.synced
    }
    fn reset(&mut self) {
        self.synced = false;
    }
}

fn read_map(fd: OwnedFd, size: u32) -> Result<(File, String, u32), String> {
    if size == 0 || size > MAX_MAP {
        return Err(format!(
            "keyboard keymap size {size} is outside 1..={MAX_MAP}"
        ));
    }
    let file = File::from(fd);
    let metadata = file
        .metadata()
        .map_err(|error| format!("inspect keyboard keymap: {error}"))?;
    if !metadata.is_file() || metadata.len() < u64::from(size) {
        return Err("keyboard keymap is not a sufficiently large regular file".into());
    }
    let mut bytes = vec![0; usize::try_from(size).map_err(|error| error.to_string())?];
    file.read_exact_at(&mut bytes, 0)
        .map_err(|error| format!("read keyboard keymap: {error}"))?;
    if bytes.len() != usize::try_from(size).map_err(|error| error.to_string())?
        || bytes.pop() != Some(0)
    {
        return Err("keyboard keymap is truncated or lacks its NUL terminator".into());
    }
    let text = String::from_utf8(bytes)
        .map_err(|error| format!("keyboard keymap isn't UTF-8: {error}"))?;
    Ok((file, text, size))
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
        if let wl_keyboard::Event::Keymap { format, fd, size } = event {
            if format != WEnum::Value(wl_keyboard::KeymapFormat::XkbV1) {
                state.error = Some("unsupported compositor keymap format".into());
                return;
            }
            match read_map(fd, size) {
                Ok(map) => state.received(map),
                Err(error) => state.error = Some(error),
            }
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
delegate_noop!(State: ZwpVirtualKeyboardManagerV1);
delegate_noop!(State: ZwpVirtualKeyboardV1);

#[cfg(test)]
mod tests {
    use rustix::fs::{MemfdFlags, memfd_create};

    use super::*;

    fn map(text: &str) -> (File, String, u32) {
        let fd = memfd_create("keymap-test", MemfdFlags::CLOEXEC).unwrap();
        (File::from(fd), text.to_owned(), 0)
    }

    #[test]
    fn any_other_map_after_an_extension_needs_the_compositors_back() {
        let mut state = State::default();
        state.received(map("compositor"));
        state.extended = Some("extension".into());
        state.received(map("extension, re-serialized differently"));
        assert!(!state.restored());
        assert_eq!(state.revision, 1);
        state.extended = None;
        state.received(map("compositor"));
        assert!(state.restored());
    }

    #[test]
    fn without_an_extension_a_foreign_map_only_counts_as_a_change() {
        let mut state = State::default();
        state.received(map("compositor"));
        state.received(map("new layout"));
        assert!(state.restored());
        assert_eq!(state.revision, 1);
    }
}
