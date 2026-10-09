//! An experimental virtual keyboard with the compositor's unchanged keymap.
//! Destruction does not release input in niri; callers must acknowledge explicit releases.

use std::fs::File;
use std::io::Read as _;
use std::os::fd::{AsFd as _, OwnedFd};
use std::path::Path;

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
    map: Option<(File, String, u32)>,
    error: Option<String>,
    revision: u64,
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
        let find = |name: &str| {
            state
                .globals
                .iter()
                .find(|(_, interface, version)| interface == name && *version >= 1)
                .map(|(id, ..)| *id)
                .ok_or_else(|| upstream(&format!("niri offers no {name}")))
        };
        let seat = registry.bind::<WlSeat, _, _>(find("wl_seat")?, 1, &handle, ());
        seat.get_keyboard(&handle, ());
        let manager = registry.bind::<ZwpVirtualKeyboardManagerV1, _, _>(
            find("zwp_virtual_keyboard_manager_v1")?,
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
    let mut file = File::from(fd);
    let metadata = file
        .metadata()
        .map_err(|error| format!("inspect keyboard keymap: {error}"))?;
    if !metadata.is_file() || metadata.len() < u64::from(size) {
        return Err("keyboard keymap is not a sufficiently large regular file".into());
    }
    let mut bytes = Vec::new();
    (&mut file)
        .take(u64::from(size))
        .read_to_end(&mut bytes)
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
                Ok(map) => {
                    state.revision += u64::from(
                        state
                            .map
                            .as_ref()
                            .is_none_or(|(_, previous, _)| previous != &map.1),
                    );
                    state.map = Some(map);
                }
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

delegate_noop!(State: WlSeat);
delegate_noop!(State: ZwpVirtualKeyboardManagerV1);
delegate_noop!(State: ZwpVirtualKeyboardV1);
