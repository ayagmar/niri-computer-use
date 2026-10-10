//! An experimental virtual keyboard with the compositor's keymap, or for one call an
//! extended copy of it. niri sends a virtual keyboard's map to every `wl_keyboard` when
//! the device next sends input, this one's included, so this keyboard sees what clients
//! were sent. Destruction does not release input in niri; callers must acknowledge
//! explicit releases.
//!
//! Maps are told apart by their libxkbcommon serialization, since niri re-serializes the
//! map a device uploads. niri sends a device's map again whenever the device's key or
//! modifiers find another map active, so one that matches the map installed on this
//! device, or an extension it installed before, is its echo; any other is a base map, the
//! first one niri sent or a later compositor map, after a layout or configuration change.
//! Restoring uploads the latest base map whenever the device holds another one: an
//! extension, or the base it was bound with after the compositor's changed.

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
/// How many times one restore uploads the base map: once, and once more for a compositor
/// map that arrived while the first went out.
const UPLOADS: usize = 2;

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
    /// The map installed on the device, serialized: the base it was bound with, an
    /// extension, or a base uploaded since. `None` before the device has one.
    installed: Option<String>,
    /// Every extension this device uploaded, serialized, so that no echo of one, however
    /// late, counts as a base map.
    echoes: Vec<String>,
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
        let Some(base) = state.uploading() else {
            return Err(upstream(
                state
                    .error
                    .as_deref()
                    .unwrap_or("niri supplied no keyboard keymap"),
            ));
        };
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

    /// Uploads the latest base map when the device holds another one, with zero modifiers
    /// in layout `group` so niri sends it to clients, and waits for niri; once more for a
    /// base map that arrived meanwhile, up to `UPLOADS` uploads. `restored` tells whether
    /// clients hold it.
    pub(crate) async fn put_back(&mut self, group: u32) -> Result<(), ToolError> {
        for _ in 0..UPLOADS {
            let Some(base) = self.state.uploading() else {
                return Ok(());
            };
            self.device.keymap(1, base.file.as_fd(), base.size);
            self.device.modifiers(0, 0, 0, group);
            self.sync().await?;
        }
        Ok(())
    }

    /// Whether clients hold the latest base map and the device holds it too, so that
    /// nothing it sends puts another map back.
    pub(crate) fn restored(&self) -> bool {
        self.state.restored()
    }

    /// `put_back`, then the proof `restored` gives.
    pub(crate) async fn restore(&mut self, group: u32) -> Result<(), ToolError> {
        self.put_back(group).await?;
        if self.restored() {
            return Ok(());
        }
        Err(upstream(
            "the last keymap niri sent clients isn't its latest compositor keymap, or that changed again while it was put back",
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
        let Some(base) = self.base.as_ref().and_then(|base| base.serialized.as_ref()) else {
            return false;
        };
        self.clients.as_ref() == Some(base) && self.installed.as_ref() == Some(base)
    }

    /// The latest base map, noted as installed, when the device holds another map or none;
    /// `None` when it holds that one, or there is none.
    fn uploading(&mut self) -> Option<&Map> {
        let base = self.base.as_ref()?;
        if self.installed == base.serialized {
            return None;
        }
        self.installed.clone_from(&base.serialized);
        Some(base)
    }

    /// Notes the extension `map` as installed, and its echo as this device's.
    fn extending(&mut self, map: &str) {
        let map = serialized(map).unwrap_or_else(|| map.to_owned());
        self.echoes.push(map.clone());
        self.installed = Some(map);
    }

    /// Classifies a map niri sent: an echo of a map this device installed, the base
    /// again, or a new base map. One that doesn't compile is none of them, and leaves
    /// clients' map unknown; the first map not compiling is an error.
    fn received(&mut self, map: Map) {
        self.clients.clone_from(&map.serialized);
        let Some(serialized) = &map.serialized else {
            if self.base.is_none() {
                self.error =
                    Some("niri's keyboard keymap doesn't compile with libxkbcommon".into());
            }
            return;
        };
        if self.installed.as_ref() == Some(serialized) || self.echoes.contains(serialized) {
            return;
        }
        match &self.base {
            None => self.base = Some(map),
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

    /// Bound to `BASE`: niri sent it, and the device installed it.
    fn bound() -> State {
        let mut state = State::default();
        state.received(map(BASE));
        assert_eq!(uploaded(&mut state).as_deref(), Some(BASE));
        state
    }

    /// Bound to `BASE`, with the extension `extension` uploaded.
    fn extended(extension: &str) -> State {
        let mut state = bound();
        state.extending(extension);
        state
    }

    /// The text of the base map a restore uploads now, if it uploads one.
    fn uploaded(state: &mut State) -> Option<String> {
        state.uploading().map(|base| base.text.clone())
    }

    #[test]
    fn a_call_that_changed_nothing_is_restored_without_an_upload() {
        let mut state = bound();
        assert!(state.restored());
        assert_eq!(uploaded(&mut state), None);
    }

    #[test]
    fn the_same_base_spelled_otherwise_changes_nothing() {
        let mut state = bound();
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
        assert_eq!(uploaded(&mut state).as_deref(), Some(BASE));
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
        assert_eq!(uploaded(&mut state), Some(new.clone()));
        state.received(map(&new));
        assert!(state.restored());
    }

    /// A configuration change during a call that needed no extension: the device still
    /// holds the old base, so the new one goes up, and the call can still be proved
    /// restored rather than leave its marker for `recover`.
    #[test]
    fn a_new_base_with_no_extension_is_uploaded_and_restored() {
        let mut state = bound();
        let new = BASE.replace("[Return]", "[KP_Enter]");
        state.received(map(&new));
        assert_eq!(state.revision, 1);
        assert!(!state.restored(), "the device still holds the old base");
        assert_eq!(uploaded(&mut state), Some(new.clone()));
        state.received(map(&new));
        assert!(state.restored());
    }

    /// The device's key or modifiers after the change send its own map, the old base,
    /// back to clients. That is its echo, not the compositor going back.
    #[test]
    fn the_old_base_echoed_after_a_new_one_is_not_a_new_base() {
        let mut state = bound();
        let new = BASE.replace("[Return]", "[KP_Enter]");
        state.received(map(&new));
        state.received(map(BASE));
        assert_eq!(state.revision, 1, "an echo is never a new base");
        assert_eq!(state.base.as_ref().unwrap().text, new);
        assert!(!state.restored(), "clients hold the old base again");
        assert_eq!(uploaded(&mut state), Some(new.clone()));
        state.received(map(&new));
        assert!(state.restored());
    }

    /// A new compositor map arriving after the restore went out is uploaded in turn.
    #[test]
    fn a_new_base_after_the_restore_went_out_is_uploaded_in_turn() {
        let extension = with("key <I60> { [eacute] };");
        let mut state = extended(&extension);
        state.received(map(&extension));
        assert_eq!(uploaded(&mut state).as_deref(), Some(BASE));
        let new = BASE.replace("[Return]", "[KP_Enter]");
        state.received(map(&new));
        assert!(!state.restored(), "the device holds the old base");
        // The restore's own echo, late, doesn't hide the new base.
        state.received(map(BASE));
        assert_eq!(state.base.as_ref().unwrap().text, new);
        assert_eq!(uploaded(&mut state), Some(new.clone()));
        state.received(map(&new));
        assert!(state.restored());
    }

    #[test]
    fn a_late_echo_of_an_old_extension_is_not_a_restore() {
        let mut state = extended(&with("key <I60> { [eacute] };"));
        uploaded(&mut state);
        state.received(map(BASE));
        assert!(state.restored());
        let second = with("key <I61> { [ssharp] };");
        state.extending(&second);
        uploaded(&mut state);
        state.received(map(&with("key <I60> { [eacute] };")));
        assert!(!state.restored());
        assert_eq!(state.revision, 0, "an echo is never a new base");
        assert_eq!(state.base.as_ref().unwrap().text, BASE);
    }

    #[test]
    fn a_map_that_doesnt_compile_is_never_a_restore() {
        let mut state = extended(&with("key <I60> { [eacute] };"));
        uploaded(&mut state);
        state.received(map("not a keymap"));
        assert!(!state.restored());
        assert_eq!(state.base.as_ref().unwrap().text, BASE);
    }

    #[test]
    fn a_first_map_that_doesnt_compile_says_so() {
        let mut state = State::default();
        state.received(map("not a keymap"));
        assert!(state.uploading().is_none());
        assert!(
            state
                .error
                .as_deref()
                .unwrap_or_default()
                .contains("doesn't compile"),
            "{:?}",
            state.error
        );
    }
}
