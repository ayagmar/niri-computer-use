//! A fake niri for the input and clipboard code's Wayland side: a display and a niri socket
//! served from the test's own process, so the peer checks take them for niri's. It offers
//! the globals that code binds and models what niri 26.04's Smithay (`ff5fa7df`) does where
//! the code relies on it:
//!
//! - a new `wl_keyboard` gets the compositor's own keymap (`KeyboardHandle::new_kbd`);
//! - a virtual keyboard's key or modifiers make its keymap the active one when it isn't,
//!   compared by libxkbcommon serialization, and send it to every `wl_keyboard`, then the
//!   device's modifiers to the focus (`virtual_keyboard_handle.rs`, `send_keymap`); a keymap
//!   that doesn't compile is ignored and the device keeps its last one;
//! - a configuration change sends the new compositor keymap the same way (`change_keymap`),
//!   and a physical key sends the compositor's keymap first when another one is active;
//! - wlr data-control announces every selection to every data device, reads a client's
//!   source through its `send` event and cancels a source another selection replaced.
//!
//! The focused client is implicit: what the focus was sent is in `Seen`. niri's event
//! stream has one workspace with one focused window, `APP_ID`, and the keyboard layouts,
//! and then sends what a test has niri report (`Compositor::event`).

use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::Write as _;
use std::os::fd::{AsFd as _, OwnedFd};
use std::os::unix::fs::FileExt as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use std::time::Duration;

use niri_ipc::{
    Event, KeyboardLayouts, LogicalOutput, Output, Reply, Request, Response, Transform, Window,
    WindowLayout, Workspace,
};
use rustix::fs::{MemfdFlags, memfd_create};
use tokio::io::unix::AsyncFd;
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::net::UnixListener;
use tokio::sync::{broadcast, mpsc, oneshot};
use wayland_protocols_misc::zwp_virtual_keyboard_v1::server::{
    zwp_virtual_keyboard_manager_v1::{self, ZwpVirtualKeyboardManagerV1},
    zwp_virtual_keyboard_v1::{self, ZwpVirtualKeyboardV1},
};
use wayland_protocols_wlr::data_control::v1::server::{
    zwlr_data_control_device_v1::{self, ZwlrDataControlDeviceV1},
    zwlr_data_control_manager_v1::{self, ZwlrDataControlManagerV1},
    zwlr_data_control_offer_v1::{self, ZwlrDataControlOfferV1},
    zwlr_data_control_source_v1::{self, ZwlrDataControlSourceV1},
};
use wayland_protocols_wlr::virtual_pointer::v1::server::{
    zwlr_virtual_pointer_manager_v1::{self, ZwlrVirtualPointerManagerV1},
    zwlr_virtual_pointer_v1::{self, ZwlrVirtualPointerV1},
};
use wayland_server::backend::{ClientData, ClientId, DisconnectReason};
use wayland_server::protocol::wl_keyboard::{self, WlKeyboard};
use wayland_server::protocol::wl_output::{self, WlOutput};
use wayland_server::protocol::wl_pointer::{self, WlPointer};
use wayland_server::protocol::wl_seat::{self, WlSeat};
use wayland_server::{
    Client, DataInit, Dispatch, Display, DisplayHandle, GlobalDispatch, New, Resource as _,
};
use xkbcommon::xkb;

/// The output the fake offers, by name.
pub(crate) const OUTPUT: &str = "FAKE-1";
/// The `app_id` of the window niri's event stream has focused.
pub(crate) const APP_ID: &str = "org.fake.Focused";

/// A selection's contents: each MIME type with its bytes.
pub(crate) type Contents = Vec<(String, Vec<u8>)>;

/// What the fake was sent, in order.
#[derive(Debug, Default, Clone)]
pub(crate) struct Seen {
    /// Every keymap sent to the `wl_keyboard`s, serialized; the first `wl_keyboard`'s
    /// compositor keymap isn't one.
    pub(crate) keymaps: Vec<String>,
    /// Every keymap a virtual keyboard uploaded, serialized, in order; one that doesn't
    /// compile isn't one.
    pub(crate) uploads: Vec<String>,
    /// Keys sent to the focus: evdev code and whether pressed.
    pub(crate) keys: Vec<(u32, bool)>,
    /// Modifiers sent to the focus: depressed mask and layout.
    pub(crate) modifiers: Vec<(u32, u32)>,
    /// Buttons of virtual pointers: evdev code and whether pressed.
    pub(crate) buttons: Vec<(u32, bool)>,
    /// The keymap clients hold, serialized: the compositor's until another is sent.
    pub(crate) active: String,
    /// The compositor's keymap, serialized.
    pub(crate) compositor: String,
    /// The current selection's types, when something is selected.
    pub(crate) selection: Option<Vec<String>>,
    /// Whether a display client's connection is held unanswered (`Compositor::hold_client`).
    pub(crate) holding: bool,
}

impl Seen {
    /// Keys pressed and not released since.
    pub(crate) fn held_keys(&self) -> Vec<u32> {
        held(&self.keys)
    }

    /// Buttons pressed and not released since.
    pub(crate) fn held_buttons(&self) -> Vec<u32> {
        held(&self.buttons)
    }

    /// The modifiers the focus was sent last, if any.
    pub(crate) fn last_modifiers(&self) -> Option<(u32, u32)> {
        self.modifiers.last().copied()
    }
}

fn held(events: &[(u32, bool)]) -> Vec<u32> {
    let mut pressed: BTreeMap<u32, bool> = BTreeMap::new();
    for &(code, down) in events {
        pressed.insert(code, down);
    }
    pressed
        .into_iter()
        .filter_map(|(code, down)| down.then_some(code))
        .collect()
}

#[derive(Debug)]
enum Command {
    /// niri's keyboard configuration changed to this keymap.
    Configure(String),
    /// niri switched to this layout.
    Switch(u32),
    /// A physical key: its evdev code.
    Physical(u32),
    /// Another client copied this, or cleared the selection.
    Copy(Selection),
    /// Copies this once the next read of a selection the fake owns has begun.
    CopyOnRead(Selection),
    /// Handles each virtual key press only after this long, as a niri busy elsewhere.
    SlowKeys(Duration),
    /// Handles each virtual keyboard's modifiers only after this long.
    SlowModifiers(Duration),
    /// Holds the next display client's connection unanswered.
    HoldClient,
    /// Serves the held connection, or the next one if it hasn't come yet.
    ServeHeld,
    /// Runs the hook at the virtual keyboards' press with this number, counted from 1,
    /// before handling anything after it.
    OnPress(usize, Hook),
}

/// What runs when niri next gets a `FocusedWindow` request (`Compositor::on_focus_request`),
/// and how long niri then takes to answer it (`Compositor::answer_focus_late`).
type Asked = Arc<Mutex<Option<(Hook, Duration)>>>;

/// What a test runs at a virtual key press (`Compositor::on_press`).
pub(crate) struct Hook(Box<dyn FnOnce() + Send>);

impl std::fmt::Debug for Hook {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Hook")
    }
}

/// The fake niri, served from a thread of its own until dropped.
#[derive(Debug)]
pub(crate) struct Compositor {
    display: PathBuf,
    socket: PathBuf,
    seen: Arc<Mutex<Seen>>,
    /// What niri answers `FocusedWindow` with: the window, or its error.
    focused: Arc<Mutex<Reply>>,
    /// What runs when niri next gets a `FocusedWindow` request, before it answers.
    asked: Asked,
    commands: Option<mpsc::UnboundedSender<(Command, oneshot::Sender<()>)>>,
    /// What the event stream sends after its initial events.
    events: broadcast::Sender<Event>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Compositor {
    /// Serves a display and a niri socket in `dir`, with `keymap` as the compositor's
    /// keymap and nothing copied.
    pub(crate) fn start(dir: &Path, keymap: &str) -> Self {
        let display = dir.join("wayland-fake");
        let socket = dir.join("niri.test.sock");
        let compositor = serialize(keymap).expect("the fake's keymap compiles");
        let seen = Arc::new(Mutex::new(Seen {
            active: compositor.clone(),
            compositor,
            ..Seen::default()
        }));
        let (commands, received) = mpsc::unbounded_channel();
        let listeners = (
            std::os::unix::net::UnixListener::bind(&display).unwrap(),
            std::os::unix::net::UnixListener::bind(&socket).unwrap(),
        );
        let state = State::new(Arc::clone(&seen));
        let focused = Arc::new(Mutex::new(Ok(Response::FocusedWindow(Some(
            focused_window(1),
        )))));
        let answered = Arc::clone(&focused);
        let asked = Arc::new(Mutex::new(None));
        let hooked = Arc::clone(&asked);
        let (events, _) = broadcast::channel(16);
        let reported = events.clone();
        let thread = std::thread::spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_io()
                .build()
                .unwrap()
                .block_on(serve(
                    listeners,
                    state,
                    (answered, hooked, reported),
                    received,
                ));
        });
        Self {
            display,
            socket,
            seen,
            focused,
            asked,
            commands: Some(commands),
            events,
            thread: Some(thread),
        }
    }

    pub(crate) fn display(&self) -> &Path {
        &self.display
    }

    /// niri's socket: it answers the keyboard layouts, the outputs and the focused window,
    /// and serves the event stream.
    pub(crate) fn niri_socket(&self) -> crate::niri::Socket {
        crate::niri::Socket::at(self.socket.clone())
    }

    /// The environment of a process in `dir`, the directory the fake was started in, whose
    /// niri and display are the fake's.
    pub(crate) fn env(&self, dir: &Path) -> crate::Env {
        crate::Env {
            display: crate::niri::Display::new(Ok(self.display.clone())),
            ..super::niri_env(dir)
        }
    }

    /// What the fake was sent so far.
    pub(crate) fn seen(&self) -> Seen {
        self.seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Changes niri's keyboard configuration to `keymap`, which it sends to clients.
    pub(crate) async fn configure(&self, keymap: &str) {
        self.send(Command::Configure(keymap.to_owned())).await;
    }

    /// Switches niri to layout `group`.
    pub(crate) async fn switch(&self, group: u32) {
        self.send(Command::Switch(group)).await;
    }

    /// Presses and releases the physical key `code`.
    pub(crate) async fn physical(&self, code: u32) {
        self.send(Command::Physical(code)).await;
    }

    /// Another client copies `contents`, or clears the selection with `None`.
    pub(crate) async fn copy(&self, contents: Option<Contents>) {
        self.send(Command::Copy(Selection::of(contents))).await;
    }

    /// Copies `contents` as soon as a client begins reading a selection the fake owns.
    pub(crate) async fn copy_on_read(&self, contents: Option<Contents>) {
        self.send(Command::CopyOnRead(Selection::of(contents)))
            .await;
    }

    /// Makes the fake handle each virtual key press, and what follows it, `delay` late.
    pub(crate) async fn slow_keys(&self, delay: Duration) {
        self.send(Command::SlowKeys(delay)).await;
    }

    /// Makes the fake handle each virtual keyboard's modifiers `delay` late.
    pub(crate) async fn slow_modifiers(&self, delay: Duration) {
        self.send(Command::SlowModifiers(delay)).await;
    }

    /// Holds the next client that connects to the display unanswered, as a niri busy with
    /// it would, until `serve_held`. `Seen::holding` says when it has connected.
    pub(crate) async fn hold_client(&self) {
        self.send(Command::HoldClient).await;
    }

    /// Serves the client `hold_client` held.
    pub(crate) async fn serve_held(&self) {
        self.send(Command::ServeHeld).await;
    }

    /// Runs `hook` at the virtual keyboards' `press`-th key press, counted from 1, before
    /// the fake handles anything after it, so whatever it does happens before the press is
    /// acknowledged: a test can have niri report an event to a waiter, or set the stop flag.
    pub(crate) async fn on_press(&self, press: usize, hook: impl FnOnce() + Send + 'static) {
        self.send(Command::OnPress(press, Hook(Box::new(hook))))
            .await;
    }

    /// Moves focus to window `id` while niri handles the `press`-th key press, as
    /// `on_press` runs its hook: niri answers `FocusedWindow` with it from then on, but its
    /// event stream never says so, as a stream trailing niri wouldn't yet.
    pub(crate) async fn focus_quietly_at(&self, press: usize, id: Option<u64>) {
        self.answer_focus_at(press, Ok(Response::FocusedWindow(id.map(focused_window))))
            .await;
    }

    /// Moves focus to window `id` now: niri answers `FocusedWindow` with it, but its event
    /// stream never says so.
    pub(crate) fn focus_quietly(&self, id: Option<u64>) {
        *self.focused.lock().unwrap_or_else(PoisonError::into_inner) =
            Ok(Response::FocusedWindow(id.map(focused_window)));
    }

    /// Runs `hook` when niri next gets a `FocusedWindow` request, before it answers, so
    /// whatever it does happens while the request is outstanding.
    pub(crate) fn on_focus_request(&self, hook: impl FnOnce() + Send + 'static) {
        *self.asked.lock().unwrap_or_else(PoisonError::into_inner) =
            Some((Hook(Box::new(hook)), Duration::ZERO));
    }

    /// Makes niri answer its next `FocusedWindow` request only `late` after it came, as a busy
    /// niri does.
    pub(crate) fn answer_focus_late(&self, late: Duration) {
        *self.asked.lock().unwrap_or_else(PoisonError::into_inner) =
            Some((Hook(Box::new(|| {})), late));
    }

    /// Makes niri answer `FocusedWindow` with `message` as its error from the `press`-th
    /// key press on.
    pub(crate) async fn fail_focus_at(&self, press: usize, message: &str) {
        self.answer_focus_at(press, Err(message.to_owned())).await;
    }

    async fn answer_focus_at(&self, press: usize, reply: Reply) {
        let focused = Arc::clone(&self.focused);
        self.on_press(press, move || {
            *focused.lock().unwrap_or_else(PoisonError::into_inner) = reply;
        })
        .await;
    }

    /// Has niri report `event` on every event stream open now.
    pub(crate) fn event(&self, event: Event) {
        self.events.send(event).ok();
    }

    /// niri's event stream from the fake's socket.
    pub(crate) fn events(&self) -> crate::niri::events::EventStream {
        crate::niri::events::EventStream::spawn(self.socket.clone())
    }

    /// Sends `command` and waits until the fake has sent what it causes.
    async fn send(&self, command: Command) {
        let (done, applied) = oneshot::channel();
        self.commands
            .as_ref()
            .unwrap()
            .send((command, done))
            .unwrap();
        applied.await.unwrap();
    }
}

impl Drop for Compositor {
    fn drop(&mut self) {
        drop(self.commands.take());
        if let Some(thread) = self.thread.take() {
            thread.join().ok();
        }
    }
}

async fn serve(
    listeners: (
        std::os::unix::net::UnixListener,
        std::os::unix::net::UnixListener,
    ),
    mut state: State,
    (focused, asked, events): (Arc<Mutex<Reply>>, Asked, broadcast::Sender<Event>),
    mut commands: mpsc::UnboundedReceiver<(Command, oneshot::Sender<()>)>,
) {
    let (display_listener, niri_listener) = listeners;
    display_listener.set_nonblocking(true).unwrap();
    niri_listener.set_nonblocking(true).unwrap();
    let display_listener = UnixListener::from_std(display_listener).unwrap();
    let niri_listener = UnixListener::from_std(niri_listener).unwrap();
    let mut display = Display::<State>::new().unwrap();
    State::globals(&display.handle());
    let readable = AsyncFd::new(display.backend().poll_fd().try_clone_to_owned().unwrap()).unwrap();
    loop {
        tokio::select! {
            accepted = display_listener.accept() => {
                let stream = accepted.unwrap().0.into_std().unwrap();
                state.admit(&display.handle(), stream);
            }
            accepted = niri_listener.accept() => {
                let group = state.group;
                let focused = focused.lock().unwrap_or_else(PoisonError::into_inner).clone();
                let answered = (focused, Arc::clone(&asked), events.subscribe());
                tokio::spawn(answer(accepted.unwrap().0, group, answered));
            }
            ready = readable.readable() => {
                let mut ready = ready.unwrap();
                display.dispatch_clients(&mut state).unwrap();
                ready.clear_ready();
            }
            command = commands.recv() => {
                let Some((command, done)) = command else { break };
                state.apply(&display.handle(), command);
                display.flush_clients().ok();
                done.send(()).ok();
            }
        }
        display.flush_clients().ok();
    }
}

/// The output the fake's display offers, 1920×1080 at scale 1.
fn output() -> Output {
    Output {
        name: OUTPUT.to_owned(),
        make: "Fake".to_owned(),
        model: "Fake".to_owned(),
        serial: None,
        physical_size: None,
        modes: vec![super::output_mode::from_logical(1920, 1080, 1.0)],
        current_mode: Some(0),
        is_custom_mode: false,
        vrr_supported: false,
        vrr_enabled: false,
        logical: Some(LogicalOutput {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
            scale: 1.0,
            transform: Transform::Normal,
        }),
    }
}

/// niri's keyboard layouts, with `group` active.
fn layouts(group: u32) -> KeyboardLayouts {
    KeyboardLayouts {
        names: vec!["first".into(), "second".into()],
        current_idx: u8::try_from(group).unwrap(),
    }
}

/// What niri's event stream starts with: one workspace with one focused window, the
/// keyboard layouts with `group` active, and the overview closed.
pub(crate) fn initial_events(group: u32) -> Vec<Event> {
    let workspace = Workspace {
        id: 1,
        idx: 1,
        name: None,
        output: Some(OUTPUT.to_owned()),
        is_urgent: false,
        is_active: true,
        is_focused: true,
        active_window_id: Some(1),
    };
    vec![
        Event::WorkspacesChanged {
            workspaces: vec![workspace],
        },
        Event::WindowsChanged {
            windows: vec![focused_window(1)],
        },
        Event::KeyboardLayoutsChanged {
            keyboard_layouts: layouts(group),
        },
        Event::OverviewOpenedOrClosed { is_open: false },
    ]
}

/// Window `id`, focused, with `APP_ID`, filling the output.
fn focused_window(id: u64) -> Window {
    Window {
        id,
        title: Some("fake".to_owned()),
        app_id: Some(APP_ID.to_owned()),
        pid: None,
        workspace_id: Some(1),
        is_focused: true,
        is_floating: false,
        is_urgent: false,
        layout: WindowLayout {
            pos_in_scrolling_layout: Some((1, 1)),
            tile_size: (1920.0, 1080.0),
            window_size: (1920, 1080),
            tile_pos_in_workspace_view: None,
            window_offset_in_tile: (0.0, 0.0),
        },
        focus_timestamp: None,
    }
}

/// Answers one niri request: the keyboard layouts, with `group` active, the outputs, the
/// focused window with `focused`, or the event stream, which then sends `events` and stays
/// open.
async fn answer(
    stream: tokio::net::UnixStream,
    group: u32,
    (focused, asked, mut events): (Reply, Asked, broadcast::Receiver<Event>),
) {
    let (reader, mut writer) = stream.into_split();
    let mut line = String::new();
    if BufReader::new(reader).read_line(&mut line).await.is_err() {
        return;
    }
    let request = serde_json::from_str(&line);
    let reply: Reply = match request {
        Ok(Request::KeyboardLayouts) => Ok(Response::KeyboardLayouts(layouts(group))),
        Ok(Request::Outputs) => Ok(Response::Outputs(HashMap::from([(
            OUTPUT.to_owned(),
            output(),
        )]))),
        Ok(Request::FocusedWindow) => {
            let hook = asked.lock().unwrap_or_else(PoisonError::into_inner).take();
            if let Some((Hook(hook), late)) = hook {
                hook();
                stall(late);
            }
            focused
        }
        Ok(Request::EventStream) => Ok(Response::Handled),
        _ => Err(
            "the fake answers only KeyboardLayouts, Outputs, FocusedWindow and EventStream".into(),
        ),
    };
    let mut lines = vec![serde_json::to_string(&reply).unwrap()];
    if matches!(request, Ok(Request::EventStream)) {
        lines.extend(
            initial_events(group)
                .iter()
                .map(|event| serde_json::to_string(event).unwrap()),
        );
    }
    for text in lines {
        writer.write_all(format!("{text}\n").as_bytes()).await.ok();
    }
    if matches!(request, Ok(Request::EventStream)) {
        while let Ok(event) = events.recv().await {
            let text = serde_json::to_string(&event).unwrap();
            writer.write_all(format!("{text}\n").as_bytes()).await.ok();
        }
        std::future::pending::<()>().await;
    }
}

/// Holds up the fake's own thread, and with it every client, as a busy niri does.
#[expect(
    clippy::disallowed_methods,
    reason = "this blocks the fake niri's thread on purpose, to delay every answer"
)]
fn stall(delay: Duration) {
    std::thread::sleep(delay);
}

#[derive(Debug)]
struct Peer;

impl ClientData for Peer {
    fn initialized(&self, _: ClientId) {}
    fn disconnected(&self, _: ClientId, _: DisconnectReason) {}
}

/// A virtual keyboard's keymap, serialized, and its modifiers: depressed mask and layout.
#[derive(Debug, Default)]
struct Virtual {
    keymap: Option<String>,
    modifiers: (u32, u32),
}

/// What a selection offer reads from.
#[derive(Debug)]
enum Owner {
    Fake(Contents),
    Source(ZwlrDataControlSourceV1),
}

#[derive(Debug)]
enum Selection {
    Empty,
    Fake(Contents),
    Source(ZwlrDataControlSourceV1),
}

impl Selection {
    /// Another client's copy, or nothing.
    fn of(contents: Option<Contents>) -> Self {
        contents.map_or(Self::Empty, Self::Fake)
    }
}

#[derive(Debug)]
struct State {
    seen: Arc<Mutex<Seen>>,
    keyboards: Vec<WlKeyboard>,
    /// The layout niri has active.
    group: u32,
    devices: Vec<ZwlrDataControlDeviceV1>,
    selection: Selection,
    copy_on_read: Option<Selection>,
    /// How late each virtual key press is handled.
    key_delay: Duration,
    /// How late each virtual keyboard's modifiers are handled.
    modifier_delay: Duration,
    /// Whether the next display client is held (`Command::HoldClient`).
    hold_next: bool,
    /// The display client held, unanswered.
    held: Option<std::os::unix::net::UnixStream>,
    /// What to run at which press (`Command::OnPress`).
    on_press: Option<(usize, Hook)>,
}

impl State {
    fn new(seen: Arc<Mutex<Seen>>) -> Self {
        Self {
            seen,
            keyboards: Vec::new(),
            group: 0,
            devices: Vec::new(),
            selection: Selection::Empty,
            copy_on_read: None,
            key_delay: Duration::ZERO,
            modifier_delay: Duration::ZERO,
            hold_next: false,
            held: None,
            on_press: None,
        }
    }

    /// Serves a new display client, unless it is the one to hold.
    fn admit(&mut self, handle: &DisplayHandle, stream: std::os::unix::net::UnixStream) {
        if std::mem::take(&mut self.hold_next) {
            self.held = Some(stream);
            self.seen().holding = true;
            return;
        }
        handle
            .clone()
            .insert_client(stream, Arc::new(Peer))
            .unwrap();
    }

    fn globals(handle: &DisplayHandle) {
        handle.create_global::<Self, WlSeat, ()>(7, ());
        handle.create_global::<Self, WlOutput, ()>(4, ());
        handle.create_global::<Self, ZwpVirtualKeyboardManagerV1, ()>(1, ());
        handle.create_global::<Self, ZwlrVirtualPointerManagerV1, ()>(2, ());
        handle.create_global::<Self, ZwlrDataControlManagerV1, ()>(2, ());
    }

    fn seen(&self) -> std::sync::MutexGuard<'_, Seen> {
        self.seen.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn apply(&mut self, handle: &DisplayHandle, command: Command) {
        match command {
            Command::Configure(keymap) => {
                let keymap = serialize(&keymap).expect("the new keymap compiles");
                self.seen().compositor.clone_from(&keymap);
                self.send_keymap(&keymap, (0, self.group));
            }
            Command::Switch(group) => {
                self.group = group;
                self.seen().modifiers.push((0, group));
            }
            Command::Physical(code) => {
                let compositor = self.seen().compositor.clone();
                self.send_keymap(&compositor, (0, self.group));
                let mut seen = self.seen();
                seen.keys.push((code, true));
                seen.keys.push((code, false));
            }
            Command::Copy(selection) => self.select(handle, selection),
            Command::CopyOnRead(selection) => self.copy_on_read = Some(selection),
            Command::SlowKeys(delay) => self.key_delay = delay,
            Command::SlowModifiers(delay) => self.modifier_delay = delay,
            Command::HoldClient => self.hold_next = true,
            Command::OnPress(press, hook) => self.on_press = Some((press, hook)),
            Command::ServeHeld => {
                self.hold_next = false;
                if let Some(stream) = self.held.take() {
                    self.seen().holding = false;
                    self.admit(handle, stream);
                }
            }
        }
    }

    /// Runs the hook `Command::OnPress` set for the press just seen, if it is that press.
    fn pressed(&mut self) {
        let presses = self.seen().keys.iter().filter(|(_, down)| *down).count();
        if let Some((_, Hook(hook))) = self.on_press.take_if(|(at, _)| *at == presses) {
            hook();
        }
    }

    /// Makes `keymap` the active keymap when it isn't, sends it to every `wl_keyboard` and
    /// `modifiers` to the focus. Returns whether it was sent.
    fn send_keymap(&self, keymap: &str, modifiers: (u32, u32)) -> bool {
        let mut seen = self.seen();
        if seen.active == keymap {
            return false;
        }
        keymap.clone_into(&mut seen.active);
        seen.keymaps.push(keymap.to_owned());
        seen.modifiers.push(modifiers);
        drop(seen);
        for keyboard in &self.keyboards {
            send_map(keyboard, keymap);
        }
        true
    }

    /// Makes `selection` the selection, cancels the source it replaced, and announces it on
    /// every data device.
    fn select(&mut self, handle: &DisplayHandle, selection: Selection) {
        self.seen().selection = match &selection {
            Selection::Empty => None,
            Selection::Fake(contents) => {
                Some(contents.iter().map(|(mime, _)| mime.clone()).collect())
            }
            Selection::Source(source) => Some(source_types(source)),
        };
        let previous = std::mem::replace(&mut self.selection, selection);
        if let Selection::Source(source) = previous
            && !matches!(&self.selection, Selection::Source(now) if *now == source)
        {
            source.cancelled();
        }
        for device in self.devices.clone() {
            self.announce(handle, &device);
        }
    }

    fn announce(&self, handle: &DisplayHandle, device: &ZwlrDataControlDeviceV1) {
        let (owner, types) = match &self.selection {
            Selection::Empty => {
                device.selection(None);
                return;
            }
            Selection::Fake(contents) => (
                Owner::Fake(contents.clone()),
                contents.iter().map(|(mime, _)| mime.clone()).collect(),
            ),
            Selection::Source(source) => (Owner::Source(source.clone()), source_types(source)),
        };
        let Some(client) = device.client() else {
            return;
        };
        let offer = client
            .create_resource::<ZwlrDataControlOfferV1, _, Self>(handle, device.version(), owner)
            .unwrap();
        device.data_offer(&offer);
        for mime in types {
            offer.offer(mime);
        }
        device.selection(Some(&offer));
    }
}

fn source_types(source: &ZwlrDataControlSourceV1) -> Vec<String> {
    source
        .data::<Mutex<Vec<String>>>()
        .map(|types| types.lock().unwrap_or_else(PoisonError::into_inner).clone())
        .unwrap_or_default()
}

/// Compiles `keymap` and serializes it as niri does what it sends.
fn serialize(keymap: &str) -> Option<String> {
    let context = xkb::Context::new(xkb::CONTEXT_NO_DEFAULT_INCLUDES);
    xkb::Keymap::new_from_string(
        &context,
        keymap.to_owned(),
        xkb::KEYMAP_FORMAT_TEXT_V1,
        xkb::KEYMAP_COMPILE_NO_FLAGS,
    )
    .map(|compiled| compiled.get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1))
}

/// Sends `keymap` to `keyboard` as a sealed, NUL-terminated file, as Smithay does from
/// version 7.
fn send_map(keyboard: &WlKeyboard, keymap: &str) {
    let mut file = File::from(memfd_create("fake-keymap", MemfdFlags::CLOEXEC).unwrap());
    file.write_all(keymap.as_bytes()).unwrap();
    file.write_all(&[0]).unwrap();
    let size = u32::try_from(keymap.len() + 1).unwrap();
    keyboard.keymap(wl_keyboard::KeymapFormat::XkbV1, file.as_fd(), size);
}

/// The keymap a client uploaded, without its NUL.
fn read_keymap(fd: OwnedFd, size: u32) -> Option<String> {
    let mut bytes = vec![0; usize::try_from(size).ok()?];
    File::from(fd).read_exact_at(&mut bytes, 0).ok()?;
    while bytes.last() == Some(&0) {
        bytes.pop();
    }
    String::from_utf8(bytes).ok()
}

impl GlobalDispatch<WlSeat, ()> for State {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &Client,
        resource: New<WlSeat>,
        (): &(),
        init: &mut DataInit<'_, Self>,
    ) {
        let seat = init.init(resource, ());
        seat.capabilities(wl_seat::Capability::Keyboard | wl_seat::Capability::Pointer);
    }
}

impl Dispatch<WlSeat, ()> for State {
    fn request(
        state: &mut Self,
        _: &Client,
        _: &WlSeat,
        request: wl_seat::Request,
        (): &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        if let wl_seat::Request::GetKeyboard { id } = request {
            let keyboard = init.init(id, ());
            send_map(&keyboard, &state.seen().compositor);
            state.keyboards.push(keyboard);
        } else if let wl_seat::Request::GetPointer { id } = request {
            init.init(id, ());
        }
    }
}

impl Dispatch<WlKeyboard, ()> for State {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &WlKeyboard,
        _: wl_keyboard::Request,
        (): &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
    }

    fn destroyed(state: &mut Self, _: ClientId, keyboard: &WlKeyboard, (): &()) {
        state.keyboards.retain(|known| known != keyboard);
    }
}

impl Dispatch<WlPointer, ()> for State {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &WlPointer,
        _: wl_pointer::Request,
        (): &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
    }
}

impl GlobalDispatch<WlOutput, ()> for State {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &Client,
        resource: New<WlOutput>,
        (): &(),
        init: &mut DataInit<'_, Self>,
    ) {
        let output = init.init(resource, ());
        output.name(OUTPUT.to_owned());
        output.done();
    }
}

impl Dispatch<WlOutput, ()> for State {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &WlOutput,
        _: wl_output::Request,
        (): &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
    }
}

impl GlobalDispatch<ZwpVirtualKeyboardManagerV1, ()> for State {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &Client,
        resource: New<ZwpVirtualKeyboardManagerV1>,
        (): &(),
        init: &mut DataInit<'_, Self>,
    ) {
        init.init(resource, ());
    }
}

impl Dispatch<ZwpVirtualKeyboardManagerV1, ()> for State {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &ZwpVirtualKeyboardManagerV1,
        request: zwp_virtual_keyboard_manager_v1::Request,
        (): &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        if let zwp_virtual_keyboard_manager_v1::Request::CreateVirtualKeyboard { id, .. } = request
        {
            init.init(id, Mutex::new(Virtual::default()));
        }
    }
}

impl Dispatch<ZwpVirtualKeyboardV1, Mutex<Virtual>> for State {
    fn request(
        state: &mut Self,
        _: &Client,
        device: &ZwpVirtualKeyboardV1,
        request: zwp_virtual_keyboard_v1::Request,
        data: &Mutex<Virtual>,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        let mut data = data.lock().unwrap_or_else(PoisonError::into_inner);
        if let zwp_virtual_keyboard_v1::Request::Keymap { fd, size, .. } = request {
            if let Some(keymap) = read_keymap(fd, size).as_deref().and_then(serialize) {
                state.seen().uploads.push(keymap.clone());
                data.keymap = Some(keymap);
            }
            return;
        }
        if matches!(request, zwp_virtual_keyboard_v1::Request::Destroy) {
            return;
        }
        let Some(keymap) = data.keymap.clone() else {
            device.post_error(
                zwp_virtual_keyboard_v1::Error::NoKeymap,
                "input before a keymap",
            );
            return;
        };
        if let zwp_virtual_keyboard_v1::Request::Key {
            key,
            state: pressed,
            ..
        } = request
        {
            if pressed == 1 {
                stall(state.key_delay);
            }
            state.send_keymap(&keymap, data.modifiers);
            state.seen().keys.push((key, pressed == 1));
            if pressed == 1 {
                state.pressed();
            }
        } else if let zwp_virtual_keyboard_v1::Request::Modifiers {
            mods_depressed,
            group,
            ..
        } = request
        {
            stall(state.modifier_delay);
            data.modifiers = (mods_depressed, group);
            if !state.send_keymap(&keymap, data.modifiers) {
                state.seen().modifiers.push(data.modifiers);
            }
        }
    }
}

impl GlobalDispatch<ZwlrVirtualPointerManagerV1, ()> for State {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &Client,
        resource: New<ZwlrVirtualPointerManagerV1>,
        (): &(),
        init: &mut DataInit<'_, Self>,
    ) {
        init.init(resource, ());
    }
}

impl Dispatch<ZwlrVirtualPointerManagerV1, ()> for State {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &ZwlrVirtualPointerManagerV1,
        request: zwlr_virtual_pointer_manager_v1::Request,
        (): &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        if let zwlr_virtual_pointer_manager_v1::Request::CreateVirtualPointer { id, .. }
        | zwlr_virtual_pointer_manager_v1::Request::CreateVirtualPointerWithOutput {
            id, ..
        } = request
        {
            init.init(id, ());
        }
    }
}

impl Dispatch<ZwlrVirtualPointerV1, ()> for State {
    fn request(
        state: &mut Self,
        _: &Client,
        _: &ZwlrVirtualPointerV1,
        request: zwlr_virtual_pointer_v1::Request,
        (): &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        if let zwlr_virtual_pointer_v1::Request::Button {
            button,
            state: pressed,
            ..
        } = request
        {
            let pressed = pressed == wayland_server::WEnum::Value(wl_pointer::ButtonState::Pressed);
            state.seen().buttons.push((button, pressed));
        }
    }
}

impl GlobalDispatch<ZwlrDataControlManagerV1, ()> for State {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &Client,
        resource: New<ZwlrDataControlManagerV1>,
        (): &(),
        init: &mut DataInit<'_, Self>,
    ) {
        init.init(resource, ());
    }
}

impl Dispatch<ZwlrDataControlManagerV1, ()> for State {
    fn request(
        state: &mut Self,
        _: &Client,
        _: &ZwlrDataControlManagerV1,
        request: zwlr_data_control_manager_v1::Request,
        (): &(),
        handle: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        if let zwlr_data_control_manager_v1::Request::CreateDataSource { id } = request {
            init.init(id, Mutex::new(Vec::<String>::new()));
        } else if let zwlr_data_control_manager_v1::Request::GetDataDevice { id, .. } = request {
            let device = init.init(id, ());
            state.announce(handle, &device);
            state.devices.push(device);
        }
    }
}

impl Dispatch<ZwlrDataControlDeviceV1, ()> for State {
    fn request(
        state: &mut Self,
        _: &Client,
        _: &ZwlrDataControlDeviceV1,
        request: zwlr_data_control_device_v1::Request,
        (): &(),
        handle: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        if let zwlr_data_control_device_v1::Request::SetSelection { source } = request {
            state.select(handle, source.map_or(Selection::Empty, Selection::Source));
        }
    }

    fn destroyed(state: &mut Self, _: ClientId, device: &ZwlrDataControlDeviceV1, (): &()) {
        state.devices.retain(|known| known != device);
    }
}

impl Dispatch<ZwlrDataControlSourceV1, Mutex<Vec<String>>> for State {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &ZwlrDataControlSourceV1,
        request: zwlr_data_control_source_v1::Request,
        types: &Mutex<Vec<String>>,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        if let zwlr_data_control_source_v1::Request::Offer { mime_type } = request {
            types
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(mime_type);
        }
    }

    fn destroyed(
        state: &mut Self,
        _: ClientId,
        source: &ZwlrDataControlSourceV1,
        _: &Mutex<Vec<String>>,
    ) {
        if matches!(&state.selection, Selection::Source(held) if held == source) {
            state.selection = Selection::Empty;
            state.seen().selection = None;
        }
    }
}

impl Dispatch<ZwlrDataControlOfferV1, Owner> for State {
    fn request(
        state: &mut Self,
        _: &Client,
        _: &ZwlrDataControlOfferV1,
        request: zwlr_data_control_offer_v1::Request,
        owner: &Owner,
        handle: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        let zwlr_data_control_offer_v1::Request::Receive { mime_type, fd } = request else {
            return;
        };
        match owner {
            Owner::Source(source) => {
                if source.is_alive() {
                    source.send(mime_type, fd.as_fd());
                }
            }
            Owner::Fake(contents) => {
                let bytes = contents
                    .iter()
                    .find(|(offered, _)| *offered == mime_type)
                    .map(|(_, bytes)| bytes.clone())
                    .unwrap_or_default();
                // A reader may stop early; the write then fails, as an owner's would.
                std::thread::spawn(move || File::from(fd).write_all(&bytes).ok());
                if let Some(copy) = state.copy_on_read.take() {
                    state.select(handle, copy);
                }
            }
        }
    }
}
