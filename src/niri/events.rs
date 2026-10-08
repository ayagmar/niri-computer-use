//! niri's event stream: one long-lived connection whose events feed niri-ipc's
//! `EventStreamState`, read by its own task.
//!
//! An ordinary disconnect drops the state and reconnects after a second. An event that
//! doesn't parse also drops the state and reconnects at once; a second one stops the
//! stream for good (`schema_incompatible`), because this build doesn't understand the
//! running niri. The task ends, closing its connection, once no tool holds the stream.

use std::path::PathBuf;
use std::time::Duration;

use niri_ipc::state::{EventStreamState, EventStreamStatePart as _};
use niri_ipc::{Event, KeyboardLayouts, Reply, Request, Response, Window, Workspace};
use serde::Serialize;
use tokio::io::{AsyncBufReadExt as _, AsyncRead, AsyncWrite, AsyncWriteExt as _, BufReader};
use tokio::net::UnixStream;
use tokio::sync::watch;

use crate::error::{ErrorName, ToolError};

/// For connecting, the request's reply, and a tool waiting for the initial state.
const DEADLINE: Duration = Duration::from_secs(2);
const RECONNECT_DELAY: Duration = Duration::from_secs(1);
/// Parse failures before the stream stops. Ordinary disconnects don't count.
const PARSE_FAILURES: u32 = 2;

/// The reader task's view of the stream.
#[derive(Debug)]
enum Connection {
    /// Why the last connection ended, with the name a tool should report.
    Connecting {
        last_error: Option<ToolError>,
    },
    Connected(Replica),
    SchemaIncompatible {
        event: String,
    },
}

/// niri's state as replayed from the stream, and which parts of niri's initial burst
/// have arrived. niri sends the workspaces, the windows, the keyboard layouts (if any)
/// and the overview state in that order (`EventStreamState::replicate`).
#[derive(Debug, Default)]
struct Replica {
    state: EventStreamState,
    workspaces: bool,
    windows: bool,
    overview: bool,
}

impl Replica {
    fn apply(&mut self, event: Event) {
        self.workspaces |= matches!(event, Event::WorkspacesChanged { .. });
        self.windows |= matches!(event, Event::WindowsChanged { .. });
        self.overview |= matches!(event, Event::OverviewOpenedOrClosed { .. });
        self.state.apply(event);
    }

    const fn initialized(&self) -> bool {
        self.workspaces && self.windows && self.overview
    }
}

/// What `status` reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum StreamState {
    Connected,
    Disconnected,
    SchemaIncompatible,
}

/// One snapshot of the desktop, taken from the replayed state at one moment.
#[derive(Debug, Serialize)]
pub(crate) struct DesktopState {
    /// By id.
    windows: Vec<Window>,
    /// By output, then index.
    workspaces: Vec<Workspace>,
    /// niri reports no focused window while keyboard focus is outside the layout, for
    /// example on a layer-shell panel, the lock screen or the overview.
    focused_window: Option<u64>,
    overview_open: bool,
    keyboard_layouts: Option<KeyboardLayouts>,
}

/// The tools' handle on the stream.
#[derive(Debug, Clone)]
pub(crate) struct EventStream {
    connection: watch::Receiver<Connection>,
}

impl EventStream {
    /// Starts the reader task on the current Tokio runtime.
    pub(crate) fn spawn(socket: PathBuf) -> Self {
        let (sender, connection) = watch::channel(Connection::Connecting { last_error: None });
        tokio::spawn(run(
            move || {
                let socket = socket.clone();
                async move { UnixStream::connect(&socket).await }
            },
            sender,
            RECONNECT_DELAY,
        ));
        Self { connection }
    }

    pub(crate) fn state(&self) -> StreamState {
        match &*self.connection.borrow() {
            Connection::Connected(replica) if replica.initialized() => StreamState::Connected,
            Connection::Connected(_) | Connection::Connecting { .. } => StreamState::Disconnected,
            Connection::SchemaIncompatible { .. } => StreamState::SchemaIncompatible,
        }
    }

    /// The current desktop. Waits up to two seconds for niri's initial state, for
    /// example right after the server starts or reconnects.
    pub(crate) async fn desktop(&self) -> Result<DesktopState, ToolError> {
        self.desktop_within(DEADLINE).await
    }

    async fn desktop_within(&self, deadline: Duration) -> Result<DesktopState, ToolError> {
        let mut connection = self.connection.clone();
        let settled = |current: &Connection| match current {
            Connection::Connected(replica) => replica.initialized(),
            Connection::SchemaIncompatible { .. } => true,
            Connection::Connecting { .. } => false,
        };
        // Whatever the wait ends with, the current value below says what to report. Drop
        // the guard it may return before reading that value.
        drop(tokio::time::timeout(deadline, connection.wait_for(settled)).await);
        let current = connection.borrow();
        match &*current {
            Connection::Connected(replica) if replica.initialized() => Ok(snapshot(&replica.state)),
            Connection::Connected(_) => Err(ToolError::new(
                ErrorName::DeadlineExceeded,
                format!("niri's event stream sent no initial state within {deadline:?}"),
            )),
            Connection::Connecting {
                last_error: Some(error),
            } => Err(ToolError::new(
                error.name,
                format!(
                    "niri's event stream is reconnecting after: {}",
                    error.detail
                ),
            )),
            // Still on the first connect or its reply, which have the same deadline.
            Connection::Connecting { last_error: None } => Err(ToolError::new(
                ErrorName::DeadlineExceeded,
                format!("niri's event stream didn't connect within {deadline:?}"),
            )),
            Connection::SchemaIncompatible { event } => Err(ToolError::new(
                ErrorName::UpstreamError,
                format!(
                    "niri sent an event this build can't parse ({event}); the event stream stays stopped until the server restarts"
                ),
            )),
        }
    }
}

fn snapshot(state: &EventStreamState) -> DesktopState {
    let mut windows: Vec<Window> = state.windows.windows.values().cloned().collect();
    windows.sort_by_key(|window| window.id);
    let mut workspaces: Vec<Workspace> = state.workspaces.workspaces.values().cloned().collect();
    workspaces.sort_by(|a, b| (&a.output, a.idx).cmp(&(&b.output, b.idx)));
    DesktopState {
        focused_window: windows
            .iter()
            .find(|window| window.is_focused)
            .map(|window| window.id),
        windows,
        workspaces,
        overview_open: state.overview.is_open,
        keyboard_layouts: state.keyboard_layouts.keyboard_layouts.clone(),
    }
}

/// How one connection ended.
#[derive(Debug, PartialEq, Eq)]
enum Ended {
    Disconnected(ToolError),
    Unparsable(String),
}

/// Connects, reads until the connection ends, and reconnects, until a second
/// unparsable event or until no tool holds the stream any more.
async fn run<S, F>(
    mut connect: impl FnMut() -> F,
    sender: watch::Sender<Connection>,
    reconnect_delay: Duration,
) where
    S: AsyncRead + AsyncWrite + Unpin,
    F: Future<Output = std::io::Result<S>>,
{
    let mut parse_failures = 0;
    loop {
        let ended = tokio::select! {
            () = sender.closed() => return,
            ended = session(&mut connect, &sender) => ended,
        };
        match ended {
            Ended::Unparsable(event) => {
                parse_failures += 1;
                if parse_failures >= PARSE_FAILURES {
                    sender.send_replace(Connection::SchemaIncompatible { event });
                    return;
                }
                sender.send_replace(Connection::Connecting {
                    last_error: Some(ToolError::new(
                        ErrorName::UpstreamError,
                        format!("unparsable event {event}"),
                    )),
                });
            }
            Ended::Disconnected(error) => {
                sender.send_replace(Connection::Connecting {
                    last_error: Some(error),
                });
                tokio::select! {
                    () = sender.closed() => return,
                    () = tokio::time::sleep(reconnect_delay) => {}
                }
            }
        }
    }
}

/// One connection, from connecting until it ends.
async fn session<S, F>(connect: &mut impl FnMut() -> F, sender: &watch::Sender<Connection>) -> Ended
where
    S: AsyncRead + AsyncWrite + Unpin,
    F: Future<Output = std::io::Result<S>>,
{
    match tokio::time::timeout(DEADLINE, connect()).await {
        Ok(Ok(stream)) => read(stream, sender).await,
        Ok(Err(error)) => Ended::Disconnected(ToolError::new(
            ErrorName::NiriUnavailable,
            format!("connect: {error}"),
        )),
        Err(_) => Ended::Disconnected(ToolError::new(
            ErrorName::DeadlineExceeded,
            format!("connect: no answer within {DEADLINE:?}"),
        )),
    }
}

/// Requests the event stream on `stream` and applies every event to a fresh replica.
async fn read(
    stream: impl AsyncRead + AsyncWrite + Unpin,
    sender: &watch::Sender<Connection>,
) -> Ended {
    let mut stream = BufReader::new(stream);
    if let Err(error) = start(&mut stream).await {
        return Ended::Disconnected(error);
    }
    sender.send_replace(Connection::Connected(Replica::default()));
    let mut line = String::new();
    loop {
        line.clear();
        match stream.read_line(&mut line).await {
            Ok(0) => return Ended::Disconnected(unavailable("niri closed the event stream")),
            Ok(_) => {}
            Err(error) => return Ended::Disconnected(unavailable(format!("read event: {error}"))),
        }
        let event = match parse(&line) {
            Ok(event) => event,
            Err(name) => return Ended::Unparsable(name),
        };
        sender.send_modify(|connection| {
            if let Connection::Connected(replica) = connection {
                replica.apply(event);
            }
        });
    }
}

/// Sends `Request::EventStream` and reads niri's `Handled` reply within the deadline. The
/// error names match the per-request client's.
async fn start(
    stream: &mut BufReader<impl AsyncRead + AsyncWrite + Unpin>,
) -> Result<(), ToolError> {
    let upstream = |detail: String| ToolError::new(ErrorName::UpstreamError, detail);
    let exchange = async {
        let mut request = serde_json::to_vec(&Request::EventStream)
            .map_err(|error| upstream(format!("encode the event stream request: {error}")))?;
        request.push(b'\n');
        stream
            .get_mut()
            .write_all(&request)
            .await
            .map_err(|error| unavailable(format!("request the event stream: {error}")))?;
        let mut reply = String::new();
        let read = stream
            .read_line(&mut reply)
            .await
            .map_err(|error| unavailable(format!("read the event stream reply: {error}")))?;
        if read == 0 {
            return Err(unavailable(
                "niri closed the connection without an event stream reply",
            ));
        }
        match serde_json::from_str::<Reply>(&reply) {
            Ok(Ok(Response::Handled)) => Ok(()),
            Ok(Err(message)) => Err(upstream(format!("niri replied: {message}"))),
            Ok(Ok(_)) => Err(upstream(
                "niri answered the event stream request with another response".to_owned(),
            )),
            Err(error) => Err(upstream(format!("unreadable event stream reply: {error}"))),
        }
    };
    tokio::time::timeout(DEADLINE, exchange)
        .await
        .unwrap_or_else(|_| {
            Err(ToolError::new(
                ErrorName::DeadlineExceeded,
                format!("no event stream reply within {DEADLINE:?}"),
            ))
        })
}

fn unavailable(detail: impl Into<String>) -> ToolError {
    ToolError::new(ErrorName::NiriUnavailable, detail)
}

/// Parses one event line. On failure, returns the event's type name: the single key of
/// the JSON object, as niri serializes its `Event` enum.
fn parse(line: &str) -> Result<Event, String> {
    let value: serde_json::Value =
        serde_json::from_str(line).map_err(|_| "a line that isn't JSON".to_owned())?;
    let name = value
        .as_object()
        .and_then(|object| object.keys().next().cloned())
        .unwrap_or_else(|| "a value that isn't an object".to_owned());
    serde_json::from_value(value).map_err(|_| name)
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    use tokio::io::{AsyncReadExt as _, DuplexStream, duplex};

    use super::*;

    const HANDLED: &str = "{\"Ok\":\"Handled\"}\n";
    const WORKSPACES: &str = "{\"WorkspacesChanged\":{\"workspaces\":[{\"id\":2,\"idx\":2,\"name\":null,\"output\":\"DP-1\",\"is_urgent\":false,\"is_active\":false,\"is_focused\":false,\"active_window_id\":null},{\"id\":1,\"idx\":1,\"name\":null,\"output\":\"DP-1\",\"is_urgent\":false,\"is_active\":true,\"is_focused\":true,\"active_window_id\":7}]}}\n";
    const WINDOWS: &str = "{\"WindowsChanged\":{\"windows\":[{\"id\":9,\"title\":\"b\",\"app_id\":\"foot\",\"pid\":2,\"workspace_id\":1,\"is_focused\":false,\"is_floating\":true,\"is_urgent\":false,\"layout\":{\"pos_in_scrolling_layout\":null,\"tile_size\":[400.0,300.0],\"window_size\":[400,300],\"tile_pos_in_workspace_view\":[0.0,0.0],\"window_offset_in_tile\":[0.0,0.0]}},{\"id\":7,\"title\":\"a\",\"app_id\":\"firefox\",\"pid\":1,\"workspace_id\":1,\"is_focused\":true,\"is_floating\":false,\"is_urgent\":false,\"layout\":{\"pos_in_scrolling_layout\":[1,1],\"tile_size\":[800.0,600.0],\"window_size\":[800,600],\"tile_pos_in_workspace_view\":null,\"window_offset_in_tile\":[0.0,0.0]}}]}}\n";
    const LAYOUTS: &str = "{\"KeyboardLayoutsChanged\":{\"keyboard_layouts\":{\"names\":[\"English (US)\"],\"current_idx\":0}}}\n";
    const OVERVIEW: &str = "{\"OverviewOpenedOrClosed\":{\"is_open\":false}}\n";
    const UNKNOWN: &str = "{\"NotARealEvent\":{}}\n";
    /// Longer than any wait in these tests, so the task doesn't reconnect during one.
    const NO_RETRY: Duration = Duration::from_secs(600);
    /// Longer than the two-second handshake deadline. Paused time makes it instant.
    const WAIT: Duration = Duration::from_secs(5);

    /// Connections the reader task gets, in order. With none left, connecting fails.
    fn connector(
        streams: Vec<DuplexStream>,
    ) -> impl FnMut() -> std::future::Ready<std::io::Result<DuplexStream>> {
        let streams = Arc::new(Mutex::new(VecDeque::from(streams)));
        move || {
            std::future::ready(
                streams
                    .lock()
                    .unwrap()
                    .pop_front()
                    .ok_or_else(|| std::io::Error::other("no niri")),
            )
        }
    }

    /// The niri end of a connection, for tests that drive it step by step.
    struct Niri(DuplexStream);

    impl Niri {
        fn pair() -> (DuplexStream, Self) {
            let (client, niri) = duplex(1 << 16);
            (client, Self(niri))
        }

        async fn expect_request(&mut self) {
            let mut request = String::new();
            tokio::time::timeout(WAIT, BufReader::new(&mut self.0).read_line(&mut request))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(request, "\"EventStream\"\n");
        }

        async fn send(&mut self, lines: &[&str]) {
            for line in lines {
                self.0.write_all(line.as_bytes()).await.unwrap();
            }
        }

        /// Whether the client closed its end within `WAIT`.
        async fn closed(mut self) -> bool {
            let mut rest = Vec::new();
            tokio::time::timeout(WAIT, self.0.read_to_end(&mut rest))
                .await
                .is_ok()
        }
    }

    /// A niri that reads the request, writes `lines`, then closes if `close`.
    fn niri(lines: Vec<&'static str>, close: bool) -> DuplexStream {
        let (client, mut niri) = Niri::pair();
        tokio::spawn(async move {
            niri.expect_request().await;
            niri.send(&lines).await;
            if !close {
                std::future::pending::<()>().await;
            }
        });
        client
    }

    fn stream(streams: Vec<DuplexStream>) -> EventStream {
        stream_retrying_after(streams, Duration::from_millis(10))
    }

    fn stream_retrying_after(streams: Vec<DuplexStream>, delay: Duration) -> EventStream {
        let (sender, connection) = watch::channel(Connection::Connecting { last_error: None });
        tokio::spawn(run(connector(streams), sender, delay));
        EventStream { connection }
    }

    async fn reconnecting(events: &EventStream) {
        let mut connection = events.connection.clone();
        tokio::time::timeout(
            WAIT,
            connection.wait_for(|current| {
                matches!(
                    current,
                    Connection::Connecting {
                        last_error: Some(_)
                    }
                )
            }),
        )
        .await
        .unwrap()
        .unwrap();
    }

    #[test]
    fn unknown_and_incomplete_events_fail_to_parse() {
        // C11: an unknown variant, and a known one missing a required field.
        assert_eq!(
            parse("{\"NotARealEvent\":{}}").unwrap_err(),
            "NotARealEvent"
        );
        assert_eq!(parse("{\"WindowClosed\":{}}").unwrap_err(), "WindowClosed");
        assert_eq!(parse("not json").unwrap_err(), "a line that isn't JSON");
        // A missing `Option` field is allowed: this is "focus cleared".
        assert!(matches!(
            parse("{\"WindowFocusChanged\":{}}"),
            Ok(Event::WindowFocusChanged { id: None })
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn the_initial_burst_gives_a_sorted_snapshot() {
        let events = stream(vec![niri(
            vec![HANDLED, WORKSPACES, WINDOWS, LAYOUTS, OVERVIEW],
            false,
        )]);
        let desktop = serde_json::to_value(events.desktop().await.unwrap()).unwrap();
        assert_eq!(events.state(), StreamState::Connected);
        assert_eq!(desktop["focused_window"], 7);
        assert_eq!(desktop["overview_open"], false);
        assert_eq!(desktop["keyboard_layouts"]["names"][0], "English (US)");
        let ids = |list: &str| {
            desktop[list]
                .as_array()
                .unwrap()
                .iter()
                .map(|item| item["id"].as_u64().unwrap())
                .collect::<Vec<_>>()
        };
        assert_eq!(ids("windows"), [7, 9]);
        assert_eq!(ids("workspaces"), [1, 2]);
    }

    #[tokio::test(start_paused = true)]
    async fn later_events_update_the_snapshot() {
        let (client, mut niri) = Niri::pair();
        let events = stream(vec![client]);
        niri.expect_request().await;
        niri.send(&[HANDLED, WORKSPACES, WINDOWS, OVERVIEW]).await;
        assert_eq!(events.desktop().await.unwrap().focused_window, Some(7));
        niri.send(&[
            "{\"WindowFocusChanged\":{\"id\":null}}\n",
            "{\"WindowClosed\":{\"id\":9}}\n",
        ])
        .await;
        let mut connection = events.connection.clone();
        tokio::time::timeout(
            WAIT,
            connection.wait_for(|current| match current {
                Connection::Connected(replica) => !replica.state.windows.windows.contains_key(&9),
                Connection::Connecting { .. } | Connection::SchemaIncompatible { .. } => false,
            }),
        )
        .await
        .unwrap()
        .unwrap();
        let desktop = events.desktop().await.unwrap();
        assert_eq!(desktop.focused_window, None);
        assert_eq!(desktop.windows.len(), 1);
        assert_eq!(desktop.keyboard_layouts, None);
    }

    #[tokio::test(start_paused = true)]
    async fn a_disconnect_drops_the_state_instead_of_serving_it_stale() {
        let events = stream_retrying_after(
            vec![niri(vec![HANDLED, WORKSPACES, WINDOWS, OVERVIEW], true)],
            NO_RETRY,
        );
        reconnecting(&events).await;
        assert_eq!(events.state(), StreamState::Disconnected);
        assert_eq!(
            events.desktop().await.unwrap_err(),
            ToolError::new(
                ErrorName::NiriUnavailable,
                "niri's event stream is reconnecting after: niri closed the event stream"
            )
        );
    }

    #[tokio::test(start_paused = true)]
    async fn one_unparsable_event_reconnects_and_a_second_stops_the_stream() {
        let once = stream(vec![
            niri(vec![HANDLED, WORKSPACES, UNKNOWN], false),
            niri(vec![HANDLED, WORKSPACES, WINDOWS, OVERVIEW], false),
        ]);
        assert!(once.desktop().await.is_ok());

        // The count lasts for the stream's lifetime: a healthy connection between the two
        // failures doesn't reset it.
        let (client, mut second) = Niri::pair();
        let twice = stream(vec![niri(vec![HANDLED, UNKNOWN], false), client]);
        second.expect_request().await;
        second.send(&[HANDLED, WORKSPACES, WINDOWS, OVERVIEW]).await;
        assert!(twice.desktop().await.is_ok());
        second.send(&[UNKNOWN]).await;
        let mut connection = twice.connection.clone();
        tokio::time::timeout(
            WAIT,
            connection.wait_for(|current| matches!(current, Connection::SchemaIncompatible { .. })),
        )
        .await
        .unwrap()
        .unwrap();
        let error = twice.desktop().await.unwrap_err();
        assert_eq!(error.name, ErrorName::UpstreamError);
        assert!(error.detail.contains("(NotARealEvent)"), "{error:?}");
        assert_eq!(twice.state(), StreamState::SchemaIncompatible);
    }

    #[tokio::test(start_paused = true)]
    async fn ordinary_disconnects_reconnect_without_counting_as_parse_failures() {
        let events = stream(vec![
            niri(vec![HANDLED, WORKSPACES], true),
            niri(vec![HANDLED, UNKNOWN], false),
            niri(vec![HANDLED, WORKSPACES, WINDOWS, OVERVIEW], false),
        ]);
        assert!(events.desktop().await.is_ok());
        assert_eq!(events.state(), StreamState::Connected);
    }

    #[tokio::test(start_paused = true)]
    async fn handshake_failures_keep_their_error_names() {
        let unreachable = stream(Vec::new());
        assert_eq!(
            unreachable.desktop().await.unwrap_err(),
            ToolError::new(
                ErrorName::NiriUnavailable,
                "niri's event stream is reconnecting after: connect: no niri"
            )
        );
        assert_eq!(unreachable.state(), StreamState::Disconnected);

        for (reply, name, detail) in [
            (
                "{\"Err\":\"no\"}\n",
                ErrorName::UpstreamError,
                "niri replied: no",
            ),
            (
                "not json\n",
                ErrorName::UpstreamError,
                "unreadable event stream reply",
            ),
        ] {
            let refused = stream_retrying_after(vec![niri(vec![reply], false)], NO_RETRY);
            reconnecting(&refused).await;
            let error = refused.desktop().await.unwrap_err();
            assert_eq!(error.name, name, "{error:?}");
            assert!(error.detail.contains(detail), "{error:?}");
        }

        // niri reads the request but never answers: the handshake's own deadline.
        let silent = stream_retrying_after(vec![niri(Vec::new(), false)], NO_RETRY);
        reconnecting(&silent).await;
        let error = silent.desktop().await.unwrap_err();
        assert_eq!(error.name, ErrorName::DeadlineExceeded, "{error:?}");
        assert!(error.detail.contains("no event stream reply"), "{error:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn a_snapshot_waits_for_the_initial_state_within_its_deadline() {
        let partial = stream(vec![niri(vec![HANDLED, WORKSPACES], false)]);
        assert_eq!(
            partial
                .desktop_within(Duration::from_millis(100))
                .await
                .unwrap_err()
                .name,
            ErrorName::DeadlineExceeded
        );
    }

    #[tokio::test(start_paused = true)]
    async fn dropping_a_waiting_snapshot_leaves_the_stream_running() {
        let (client, mut niri) = Niri::pair();
        let events = stream(vec![client]);
        niri.expect_request().await;
        niri.send(&[HANDLED, WORKSPACES]).await;
        assert!(
            tokio::time::timeout(Duration::from_millis(50), events.desktop())
                .await
                .is_err()
        );
        niri.send(&[WINDOWS, OVERVIEW]).await;
        assert!(events.desktop().await.is_ok());
    }

    #[tokio::test(start_paused = true)]
    async fn the_task_closes_the_connection_when_the_last_handle_goes() {
        let (client, mut niri) = Niri::pair();
        let events = stream(vec![client]);
        niri.expect_request().await;
        niri.send(&[HANDLED, WORKSPACES, WINDOWS, OVERVIEW]).await;
        assert!(events.desktop().await.is_ok());
        let copy = events.clone();
        drop(events);
        assert!(copy.desktop().await.is_ok());
        drop(copy);
        assert!(niri.closed().await);
    }
}
