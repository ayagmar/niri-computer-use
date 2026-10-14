//! A fake niri on the fixture's socket. Single requests are answered from a shared
//! configuration; each event stream is handed to the test, which writes its events line by
//! line and closes it by dropping it. Actions are answered `Handled`, or held unanswered,
//! and handed to the test, which sends the events the action would cause, unless the test
//! set the fake to send them itself. Each request's reply passes the fixture's hold point
//! `niri:<request>`, and the events the fake sends for an action pass `niri:effect` once the server read
//! its answer.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use niri_ipc::{
    Action, LogicalOutput, Output, Overview, Reply, Request, Response, Transform, Window, Workspace,
};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{mpsc, oneshot};

use crate::client::WAIT;
use crate::fixture::Fixture;
use crate::holds::{Held, Points};

/// The events niri would send after carrying out an action.
type Effect = dyn Fn(&Action) -> Vec<Value> + Send + Sync;

#[derive(Clone)]
struct Effects(Arc<Effect>);

impl std::fmt::Debug for Effects {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Effects")
    }
}

#[derive(Debug, Default)]
struct Config {
    outputs: Vec<Output>,
    focused: Option<String>,
    /// The answers to `Windows` and `Workspaces`.
    windows: Vec<Window>,
    workspaces: Vec<Workspace>,
    /// Read requests but never answer them.
    silent: bool,
    /// Requests answered with niri's error, by name, with the error's text.
    failing: HashMap<String, String>,
    /// Whether `OverviewState` says the overview is open.
    overview_open: bool,
    /// Read actions but never answer them.
    hold_actions: bool,
    /// How much later every `Version` is answered.
    version_delay: Duration,
    /// What the fake sends on every open event stream once it answered an action.
    effects: Option<Effects>,
    /// The event streams' lines, for the effects, which don't keep a stream open.
    streams: Vec<mpsc::WeakUnboundedSender<String>>,
}

#[derive(Debug)]
pub(crate) struct Niri {
    config: Arc<Mutex<Config>>,
    streams: mpsc::UnboundedReceiver<Stream>,
    /// One message per unanswered request, when the server closes its connection.
    abandoned: mpsc::UnboundedReceiver<()>,
    /// One message per request the fake received but doesn't answer.
    held: mpsc::UnboundedReceiver<()>,
    actions: mpsc::UnboundedReceiver<Action>,
    /// One message per answered action, once the server read the answer.
    answers_read: mpsc::UnboundedReceiver<()>,
    /// The process ID of every connection's peer, in order.
    peers: Arc<Mutex<Vec<i32>>>,
    points: Points,
}

/// An event stream the server opened. Dropping it closes the connection.
#[derive(Debug)]
pub(crate) struct Stream {
    lines: mpsc::UnboundedSender<String>,
    closed: oneshot::Receiver<()>,
}

/// The fake's ends of the channels to the test.
#[derive(Clone)]
struct Channels {
    config: Arc<Mutex<Config>>,
    streams: mpsc::UnboundedSender<Stream>,
    abandoned: mpsc::UnboundedSender<()>,
    held: mpsc::UnboundedSender<()>,
    actions: mpsc::UnboundedSender<Action>,
    answers_read: mpsc::UnboundedSender<()>,
    points: Points,
}

impl Niri {
    /// Listens on the fixture's `NIRI_SOCKET` with one output, `DP-1`, focused, passing the
    /// fixture's hold points.
    pub(crate) fn start(fixture: &Fixture) -> Self {
        Self::listen_with(&fixture.niri_socket(), fixture.points())
    }

    /// The same, listening on `socket`, with hold points of its own.
    pub(crate) fn listen(socket: &std::path::Path) -> Self {
        Self::listen_with(socket, Points::default())
    }

    /// The same, listening on `socket`, which a `NiriProcess` relays the fixture's
    /// `NIRI_SOCKET` to, passing the fixture's hold points.
    pub(crate) fn behind(fixture: &Fixture, socket: &std::path::Path) -> Self {
        Self::listen_with(socket, fixture.points())
    }

    fn listen_with(socket: &std::path::Path, points: Points) -> Self {
        // In place of the socket file the fixture leaves, as a restarted niri does.
        std::fs::remove_file(socket).ok();
        let listener = UnixListener::bind(socket).unwrap();
        let config = Arc::new(Mutex::new(Config {
            outputs: vec![output("DP-1", Some((0, 0, 2560, 1440, 1.0)))],
            focused: Some("DP-1".to_owned()),
            windows: Vec::new(),
            workspaces: Vec::new(),
            silent: false,
            failing: HashMap::new(),
            overview_open: false,
            hold_actions: false,
            version_delay: Duration::ZERO,
            effects: None,
            streams: Vec::new(),
        }));
        let (streams_to, streams) = mpsc::unbounded_channel();
        let (abandoned_to, abandoned) = mpsc::unbounded_channel();
        let (held_to, held) = mpsc::unbounded_channel();
        let (actions_to, actions) = mpsc::unbounded_channel();
        let (answers_read_to, answers_read) = mpsc::unbounded_channel();
        let channels = Channels {
            config: Arc::clone(&config),
            streams: streams_to,
            abandoned: abandoned_to,
            held: held_to,
            actions: actions_to,
            answers_read: answers_read_to,
            points: points.clone(),
        };
        let peers = Arc::new(Mutex::new(Vec::new()));
        tokio::spawn(accept(listener, channels, Arc::clone(&peers)));
        Self {
            config,
            streams,
            abandoned,
            held,
            actions,
            answers_read,
            peers,
            points,
        }
    }

    /// Whether process `pid` has connected.
    pub(crate) fn connected_from(&self, pid: i32) -> bool {
        self.peers.lock().unwrap().contains(&pid)
    }

    pub(crate) fn hold_actions(&self, hold: bool) {
        self.config.lock().unwrap().hold_actions = hold;
    }

    /// Holds the answer to the next `FocusWindow` action until the test releases it.
    pub(crate) fn hold_focus(&self) -> Held {
        self.points.hold_next("niri:Action:FocusWindow")
    }

    /// Holds the reply to the next `Version` request until the test releases it.
    pub(crate) fn hold_version(&self) -> Held {
        self.points.hold_next("niri:Version")
    }

    /// From now on, sends `effects`' events for each action on every open event stream
    /// once it answered the action, as niri would after carrying it out.
    pub(crate) fn effects(&self, effects: impl Fn(&Action) -> Vec<Value> + Send + Sync + 'static) {
        self.config.lock().unwrap().effects = Some(Effects(Arc::new(effects)));
    }

    /// Answers every `Version` request `delay` later.
    pub(crate) fn delay_version(&self, delay: Duration) {
        self.config.lock().unwrap().version_delay = delay;
    }

    /// The next action the server sent, within `WAIT`.
    pub(crate) async fn action(&mut self) -> Action {
        tokio::time::timeout(WAIT, self.actions.recv())
            .await
            .expect("the server sent no action")
            .unwrap()
    }

    /// Waits until the server read niri's answer to the next action, within `WAIT`. The
    /// test gets the action as it arrives, before it is answered.
    pub(crate) async fn answer_read(&mut self) {
        tokio::time::timeout(WAIT, self.answers_read.recv())
            .await
            .expect("the server read no answer to an action")
            .unwrap();
    }

    /// Whether the server has sent an action the test hasn't taken yet.
    pub(crate) fn sent_action(&mut self) -> bool {
        self.actions.try_recv().is_ok()
    }

    pub(crate) fn set_outputs(&self, outputs: Vec<Output>, focused: Option<&str>) {
        let mut config = self.config.lock().unwrap();
        config.outputs = outputs;
        config.focused = focused.map(str::to_owned);
    }

    /// What `Windows` and `Workspaces` are answered with, as the event stream describes
    /// them.
    pub(crate) fn set_windows(&self, windows: &[Value], workspaces: &[Value]) {
        let mut config = self.config.lock().unwrap();
        config.windows = parsed(windows);
        config.workspaces = parsed(workspaces);
    }

    /// Opens or closes the overview, as `OverviewState` answers it.
    pub(crate) fn set_overview(&self, open: bool) {
        self.config.lock().unwrap().overview_open = open;
    }

    /// Answers every later `request`, such as `Workspaces`, with niri's error `message`.
    pub(crate) fn fail(&self, request: &str, message: &str) {
        let mut config = self.config.lock().unwrap();
        config
            .failing
            .insert(request.to_owned(), message.to_owned());
    }

    pub(crate) fn set_silent(&self, silent: bool) {
        self.config.lock().unwrap().silent = silent;
    }

    /// The next event stream the server opens, within `WAIT`.
    pub(crate) async fn stream(&mut self) -> Stream {
        self.stream_within(WAIT)
            .await
            .expect("the server opened no event stream")
    }

    pub(crate) async fn stream_within(&mut self, limit: Duration) -> Option<Stream> {
        tokio::time::timeout(limit, self.streams.recv())
            .await
            .ok()
            .flatten()
    }

    /// Waits until a request arrives that the fake holds without answering.
    pub(crate) async fn held(&mut self) {
        tokio::time::timeout(WAIT, self.held.recv())
            .await
            .unwrap()
            .unwrap();
    }

    /// Whether the server closed a held request's connection within `limit`.
    pub(crate) async fn abandoned_within(&mut self, limit: Duration) -> bool {
        tokio::time::timeout(limit, self.abandoned.recv())
            .await
            .is_ok_and(|message| message.is_some())
    }
}

fn parsed<T: serde::de::DeserializeOwned>(values: &[Value]) -> Vec<T> {
    values
        .iter()
        .map(|value| serde_json::from_value(value.clone()).unwrap())
        .collect()
}

/// Serves each connection to `listener`, noting its peer's process ID in `peers`.
async fn accept(listener: UnixListener, channels: Channels, peers: Arc<Mutex<Vec<i32>>>) {
    while let Ok((connection, _)) = listener.accept().await {
        if let Some(pid) = connection.peer_cred().ok().and_then(|cred| cred.pid()) {
            peers.lock().unwrap().push(pid);
        }
        tokio::spawn(serve(connection, channels.clone()));
    }
}

impl Stream {
    pub(crate) fn send(&self, event: &Value) {
        self.lines.send(event.to_string()).unwrap();
    }

    /// Closes the connection, as niri does when it exits.
    pub(crate) fn close(&mut self) {
        self.lines = mpsc::unbounded_channel().0;
    }

    /// niri's initial burst: workspaces, the given windows, keyboard layouts and the
    /// overview state, which makes the server's replica complete.
    pub(crate) fn initial(&self, windows: &[Value]) {
        self.send(&json!({"WorkspacesChanged": {"workspaces": [{
            "id": 1, "idx": 1, "name": null, "output": "DP-1", "is_urgent": false,
            "is_active": true, "is_focused": true, "active_window_id": null
        }]}}));
        self.send(&json!({"WindowsChanged": {"windows": windows}}));
        self.send(&json!({"KeyboardLayoutsChanged": {"keyboard_layouts": {
            "names": ["English (US)"], "current_idx": 0
        }}}));
        self.send(&json!({"OverviewOpenedOrClosed": {"is_open": false}}));
    }

    /// Workspaces 1 to `count` on `DP-1`, with 1 focused.
    pub(crate) fn workspaces(&self, count: u64) {
        let workspaces: Vec<Value> = (1..=count)
            .map(|id| {
                json!({
                    "id": id, "idx": id, "name": null, "output": "DP-1", "is_urgent": false,
                    "is_active": id == 1, "is_focused": id == 1, "active_window_id": null
                })
            })
            .collect();
        self.send(&json!({"WorkspacesChanged": {"workspaces": workspaces}}));
    }

    /// Whether the server closed this stream within `limit`.
    pub(crate) async fn closed_within(&mut self, limit: Duration) -> bool {
        tokio::time::timeout(limit, &mut self.closed).await.is_ok()
    }
}

/// A window with `app_id` on `workspace`, as niri's event stream describes it.
pub(crate) fn window_on(id: u64, app_id: Option<&str>, workspace: u64, focused: bool) -> Value {
    let mut window = window(id, "t");
    window["app_id"] = json!(app_id);
    window["workspace_id"] = json!(workspace);
    window["is_focused"] = json!(focused);
    window
}

/// A window as niri's event stream describes it.
pub(crate) fn window(id: u64, title: &str) -> Value {
    json!({
        "id": id, "title": title, "app_id": "foot", "pid": 100, "workspace_id": 1,
        "is_focused": true, "is_floating": false, "is_urgent": false,
        "layout": {
            "pos_in_scrolling_layout": [1, 1], "tile_size": [800.0, 600.0],
            "window_size": [800, 600], "tile_pos_in_workspace_view": null,
            "window_offset_in_tile": [0.0, 0.0]
        }
    })
}

/// An output, enabled with `(x, y, width, height, scale)` or disabled with `None`.
pub(crate) fn output(name: &str, logical: Option<(i32, i32, u32, u32, f64)>) -> Output {
    Output {
        name: name.to_owned(),
        make: "Test".to_owned(),
        model: "Fake".to_owned(),
        serial: None,
        physical_size: None,
        modes: logical
            .map(|(_, _, w, h, scale)| crate::output_mode::from_logical(w, h, scale))
            .into_iter()
            .collect(),
        current_mode: logical.map(|_| 0),
        is_custom_mode: false,
        vrr_supported: false,
        vrr_enabled: false,
        logical: logical.map(|(x, y, width, height, scale)| LogicalOutput {
            x,
            y,
            width,
            height,
            scale,
            transform: Transform::Normal,
        }),
    }
}

async fn serve(connection: UnixStream, channels: Channels) {
    let (read, mut write) = connection.into_split();
    let mut read = BufReader::new(read);
    let mut line = String::new();
    if read.read_line(&mut line).await.unwrap_or(0) == 0 {
        return;
    }
    let request: Request = serde_json::from_str(&line).unwrap();
    if matches!(request, Request::EventStream) {
        reply(&mut write, &Ok(Response::Handled)).await;
        stream(read, write, &channels).await;
        return;
    }
    respond(read, write, &channels, &request).await;
}

/// Answers one request that isn't the event stream, or holds it unanswered until the server
/// closes the connection.
async fn respond(
    mut read: BufReader<OwnedReadHalf>,
    mut write: OwnedWriteHalf,
    channels: &Channels,
    request: &Request,
) {
    if let Request::Action(action) = request {
        channels.actions.send(action.clone()).ok();
    }
    let _busy = channels.points.pass(point(request)).await;
    if matches!(request, Request::Version) {
        let delay = channels.config.lock().unwrap().version_delay;
        tokio::time::sleep(delay).await;
    }
    let answer = {
        let config = channels.config.lock().unwrap();
        if let Request::Action(_) = request {
            (!config.hold_actions).then_some(Ok(Response::Handled))
        } else {
            (!config.silent)
                .then(|| failure(&config, request).unwrap_or_else(|| answer(&config, request)))
        }
    };
    if let Some(answer) = answer {
        reply(&mut write, &answer).await;
        if let Request::Action(action) = request {
            // The server closes the connection once it read the answer, so from here on it
            // knows niri carried the action out.
            let mut rest = Vec::new();
            read.read_to_end(&mut rest).await.ok();
            channels.answers_read.send(()).ok();
            effect(channels, action).await;
        }
        return;
    }
    channels.held.send(()).ok();
    let mut rest = Vec::new();
    read.read_to_end(&mut rest).await.ok();
    channels.abandoned.send(()).ok();
}

/// niri's error for `request`, if the test made it fail.
fn failure(config: &Config, request: &Request) -> Option<Reply> {
    let named = point(request);
    let message = config.failing.get(named.strip_prefix("niri:")?)?;
    Some(Err(message.clone()))
}

/// The hold point `request`'s reply passes: `niri:` and the request's name, with the
/// action's for an action, as `niri:Action:FocusWindow`.
fn point(request: &Request) -> String {
    let request = serde_json::to_value(request).unwrap();
    let named = name(&request);
    let action = request.get("Action").map(name);
    action.map_or_else(
        || format!("niri:{named}"),
        |action| format!("niri:{named}:{action}"),
    )
}

/// The variant a serialized niri enum names: the string itself, or the object's one key.
fn name(value: &Value) -> String {
    match value {
        Value::String(name) => name.clone(),
        Value::Object(object) => object.keys().next().cloned().unwrap_or_default(),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::Array(_) => String::new(),
    }
}

/// Sends the events the test set for `action` on every open event stream, past the hold
/// point `niri:effect`, once the server has read the answer.
async fn effect(channels: &Channels, action: &Action) {
    let effects = channels.config.lock().unwrap().effects.clone();
    let Some(effects) = effects else { return };
    let events = (effects.0)(action);
    let _busy = channels.points.pass("niri:effect".to_owned()).await;
    let config = channels.config.lock().unwrap();
    for event in events {
        for lines in config
            .streams
            .iter()
            .filter_map(mpsc::WeakUnboundedSender::upgrade)
        {
            lines.send(event.to_string()).ok();
        }
    }
}

#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "the server only sends these six; any other request is an error, as from niri"
)]
fn answer(config: &Config, request: &Request) -> Reply {
    match request {
        Request::Version => Ok(Response::Version("26.04 (protocol-test)".to_owned())),
        Request::Windows => Ok(Response::Windows(config.windows.clone())),
        Request::Workspaces => Ok(Response::Workspaces(config.workspaces.clone())),
        Request::OverviewState => Ok(Response::OverviewState(Overview {
            is_open: config.overview_open,
        })),
        Request::Outputs => Ok(Response::Outputs(
            config
                .outputs
                .iter()
                .map(|output| (output.name.clone(), output.clone()))
                .collect::<HashMap<_, _>>(),
        )),
        Request::FocusedOutput => Ok(Response::FocusedOutput(
            config
                .focused
                .as_ref()
                .and_then(|name| config.outputs.iter().find(|output| &output.name == name))
                .cloned(),
        )),
        other => Err(format!("the fake niri doesn't answer {other:?}")),
    }
}

async fn reply(write: &mut OwnedWriteHalf, reply: &Reply) {
    let mut line = serde_json::to_vec(reply).unwrap();
    line.push(b'\n');
    write.write_all(&line).await.ok();
}

/// Hands the stream to the test and writes what it sends, until the test drops it or the
/// server closes the connection.
async fn stream(
    mut read: BufReader<OwnedReadHalf>,
    mut write: OwnedWriteHalf,
    channels: &Channels,
) {
    let (lines_to, mut lines) = mpsc::unbounded_channel::<String>();
    let (closed_to, closed) = oneshot::channel();
    channels
        .config
        .lock()
        .unwrap()
        .streams
        .push(lines_to.downgrade());
    if channels
        .streams
        .send(Stream {
            lines: lines_to,
            closed,
        })
        .is_err()
    {
        return;
    }
    let mut rest = Vec::new();
    loop {
        tokio::select! {
            line = lines.recv() => {
                let Some(line) = line else { return };
                write.write_all(format!("{line}\n").as_bytes()).await.ok();
            }
            _ = read.read_to_end(&mut rest) => {
                closed_to.send(()).ok();
                return;
            }
        }
    }
}
