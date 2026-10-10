//! An experimental virtual keyboard with the compositor's keymap, or for one call an
//! extended copy of it. niri sends a virtual keyboard's map to every `wl_keyboard` when
//! the device next sends input, this one's included, so this keyboard sees what clients
//! were sent. Destruction does not release input in niri; callers must acknowledge
//! explicit releases.
//!
//! Maps are told apart by their libxkbcommon serialization, since niri re-serializes the
//! map a device uploads. One that matches an extension this device uploaded is its echo;
//! any other is a base map, the first one niri sent or a later compositor map, after a
//! layout or configuration change. Restoring uploads the latest base map.

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
use crate::input::keymap::serialized;

const MAX_MAP: u32 = 1024 * 1024;

/// A map niri sent: the file to upload it again from, its text and size, and its
/// serialization, if it compiles.
#[derive(Debug)]
struct Map {
    file: File,
    text: String,
    size: u32,
    serialized: Option<String>,
}

#[derive(Debug, Default)]
struct State {
    globals: Vec<(u32, String, u32)>,
    synced: bool,
    /// The latest base map.
    base: Option<Map>,
    error: Option<String>,
    /// Counts base maps after the first.
    revision: u64,
    /// The extended map this device uploaded, until it uploads the base again.
    extended: Option<String>,
    /// Every extension this device uploaded, serialized, so that no echo of one, however
    /// late, counts as a base map.
    echoes: Vec<String>,
    /// The base map this device last uploaded, serialized.
    uploaded: Option<String>,
    /// The last map niri sent, serialized: what clients hold. `None` before any, or when
    /// it didn't compile.
    clients: Option<String>,
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
        let base = state.base.as_ref().ok_or_else(|| {
            upstream(
                state
                    .error
                    .as_deref()
                    .unwrap_or("niri supplied no keyboard keymap"),
            )
        })?;
        let device = manager.create_virtual_keyboard(&seat, &handle, ());
        device.keymap(1, base.file.as_fd(), base.size);
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
            .base
            .as_ref()
            .map(|base| base.text.as_str())
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
        self.state.extending(map);
        self.flush()
    }

    /// Uploads the latest base map again after `extend`, and sends zero modifiers in
    /// layout `group` so niri sends it to clients. `restored` tells after a sync whether
    /// niri did.
    pub(crate) fn restore_now(&mut self, group: u32) -> Result<(), ToolError> {
        let Some(base) = self.state.restoring()? else {
            return Ok(());
        };
        self.device.keymap(1, base.file.as_fd(), base.size);
        self.device.modifiers(0, 0, 0, group);
        self.flush()
    }

    /// Whether clients hold the base map this device uploaded last: no extension is
    /// uploaded, and the last map niri sent is that base map.
    pub(crate) fn restored(&self) -> bool {
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
            "niri didn't send the latest compositor keymap back after the extended one",
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
    fn restored(&self) -> bool {
        self.extended.is_none()
            && self
                .clients
                .as_ref()
                .is_some_and(|clients| self.uploaded.as_ref() == Some(clients))
    }

    /// The latest base map to upload in place of an extension, noted as uploaded; `None`
    /// without an extension.
    fn restoring(&mut self) -> Result<Option<&Map>, ToolError> {
        if self.extended.take().is_none() {
            return Ok(None);
        }
        let base = self
            .base
            .as_ref()
            .ok_or_else(|| upstream("the keyboard map is unavailable"))?;
        self.uploaded.clone_from(&base.serialized);
        Ok(Some(base))
    }

    /// Notes the extension `map` as uploaded, and its echo as this device's.
    fn extending(&mut self, map: &str) {
        self.extended = Some(map.to_owned());
        self.echoes
            .push(serialized(map).unwrap_or_else(|| map.to_owned()));
    }

    /// Classifies a map niri sent: an echo of an extension, the base again, or a new base
    /// map. One that doesn't compile is none of them, and leaves clients' map unknown.
    fn received(&mut self, map: Map) {
        self.clients.clone_from(&map.serialized);
        let Some(serialized) = &map.serialized else {
            return;
        };
        if self.echoes.contains(serialized) {
            return;
        }
        match &self.base {
            None => {
                self.uploaded = Some(serialized.clone());
                self.base = Some(map);
            }
            Some(base) if base.serialized.as_ref() == Some(serialized) => {}
            Some(_) => {
                self.revision += 1;
                self.base = Some(map);
            }
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

fn read_map(fd: OwnedFd, size: u32) -> Result<Map, String> {
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
    Ok(Map {
        serialized: serialized(&text),
        file,
        text,
        size,
    })
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

    const BASE: &str = include_str!("../../tests/fixtures/us-de.xkb");

    fn map(text: &str) -> Map {
        let fd = memfd_create("keymap-test", MemfdFlags::CLOEXEC).unwrap();
        Map {
            file: File::from(fd),
            text: text.to_owned(),
            size: 0,
            serialized: serialized(text),
        }
    }

    /// The fixture with `entry` added to its symbols.
    fn with(entry: &str) -> String {
        BASE.replace(
            "    modifier_map Shift",
            &format!("    {entry}\n    modifier_map Shift"),
        )
    }

    /// Bound to `BASE`, with the extension `extension` uploaded.
    fn extended(extension: &str) -> State {
        let mut state = State::default();
        state.received(map(BASE));
        state.extending(extension);
        state
    }

    fn restoring(state: &mut State) -> String {
        state.restoring().unwrap().unwrap().text.clone()
    }

    #[test]
    fn the_same_base_spelled_otherwise_changes_nothing() {
        let mut state = State::default();
        state.received(map(BASE));
        state.received(map(&BASE.replace("// no system includes.\n", "")));
        assert!(state.restored());
        assert_eq!(state.revision, 0);
    }

    #[test]
    fn the_echo_of_an_extension_needs_the_base_back() {
        let extension = with("key <I60> { [eacute] };");
        let mut state = extended(&extension);
        // niri re-serializes what it echoes.
        state.received(map(&extension.replace("  ", "\t")));
        assert!(!state.restored());
        assert_eq!(state.revision, 0);
        assert_eq!(restoring(&mut state), BASE);
        assert!(!state.restored(), "the echo is still what clients hold");
        state.received(map(BASE));
        assert!(state.restored());
    }

    /// The compositor's map changed during an extension: the new one is restored, never
    /// the one the keyboard was bound with.
    #[test]
    fn a_new_base_map_is_the_one_restored() {
        let extension = with("key <I60> { [eacute] };");
        let mut state = extended(&extension);
        state.received(map(&extension));
        let new = BASE.replace("[Return]", "[KP_Enter]");
        state.received(map(&new));
        assert_eq!(state.revision, 1);
        assert_eq!(restoring(&mut state), new);
        state.received(map(BASE));
        assert!(!state.restored(), "the old base isn't the one uploaded");
        state.received(map(&new));
        assert!(state.restored());
    }

    #[test]
    fn a_late_echo_of_an_old_extension_is_not_a_restore() {
        let mut state = extended(&with("key <I60> { [eacute] };"));
        restoring(&mut state);
        state.received(map(BASE));
        assert!(state.restored());
        let second = with("key <I61> { [ssharp] };");
        state.extending(&second);
        restoring(&mut state);
        state.received(map(&with("key <I60> { [eacute] };")));
        assert!(!state.restored());
        assert_eq!(state.revision, 0, "an echo is never a new base");
        assert_eq!(state.base.as_ref().unwrap().text, BASE);
    }

    #[test]
    fn a_map_that_doesnt_compile_is_never_a_restore() {
        let mut state = extended(&with("key <I60> { [eacute] };"));
        restoring(&mut state);
        state.received(map("not a keymap"));
        assert!(!state.restored());
        assert_eq!(state.base.as_ref().unwrap().text, BASE);
    }
}
