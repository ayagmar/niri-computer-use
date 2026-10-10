//! The bridge: in shared mode, `serve` relays its client's MCP lines to the engine of its
//! niri instance, and starts that engine when there is none. Which mode it serves in is
//! decided before it reads anything from the client. Once relaying, it never turns
//! standalone: if the engine goes, it answers what was in flight with `engine_lost`,
//! reaches a new engine on the next request, and answers `engine_unavailable` while it
//! can't.

pub(crate) mod envelope;

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions, TryLockError};
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::Path;
use std::time::Duration;

use serde_json::Value;
use tokio::io::{
    AsyncBufReadExt as _, AsyncRead, AsyncReadExt as _, AsyncWriteExt as _, BufReader,
};
use tokio::net::UnixStream;
use tokio::net::unix::OwnedWriteHalf;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::control::runtime::RuntimeDir;
use crate::engine::hello::{self, Exe, Hello, Reply};
use crate::engine::host::{LOG, SOCKET};
use crate::error::{ErrorName, ToolError};
use crate::session::{Given, LineLimit, MAX_LINE};
use crate::{Env, runner};
use envelope::Message;

/// Held by the one bridge starting an engine, so bridges that start at once start one.
const START_LOCK: &str = "engine.start.lock";
/// How long a bridge tries to reach an engine, starting one as needed.
const BUDGET: Duration = Duration::from_secs(5);
/// Engines one bridge starts within `BUDGET` at most.
const MAX_STARTS: usize = 3;
const CONNECT: Duration = Duration::from_millis(500);
const RETRY_FIRST: Duration = Duration::from_millis(20);
const RETRY_MAX: Duration = Duration::from_millis(200);
/// How long a bridge that started an engine waits for it to listen.
const LISTENING: Duration = Duration::from_secs(2);
/// An engine line longer than this is the engine's fault: it is taken as lost.
const MAX_ENGINE_LINE: usize = 256 * 1024 * 1024;
/// How long, after its client's end, the bridge still passes on what the engine sends.
const DRAIN: Duration = Duration::from_secs(6);
/// How long a write to the engine may wait for the engine to read. An engine that reads
/// nothing for this long, stopped or stuck, counts as lost.
const ENGINE_WRITE: Duration = Duration::from_secs(5);

/// The engine a bridge reaches, and the hello it sends.
#[derive(Debug)]
pub(crate) struct Target {
    runtime: RuntimeDir,
    hello: Vec<u8>,
}

impl Target {
    /// The engine of the niri instance `env` names, for a client whose environment gave
    /// `given`.
    pub(crate) fn new(env: &Env, given: Given) -> Result<Self, String> {
        let runtime = RuntimeDir::of(env)?;
        let niri = env.instance.socket().map_err(Clone::clone)?;
        let display = env.display.path().ok().map(hello::resolved);
        let hello = Hello::new(Exe::current()?, niri, display.as_deref(), given)?;
        let mut line =
            serde_json::to_vec(&hello).map_err(|error| format!("write the hello: {error}"))?;
        line.push(b'\n');
        Ok(Self {
            runtime,
            hello: line,
        })
    }
}

/// A connection to an engine that has taken this bridge's hello.
#[derive(Debug)]
pub(crate) struct Link {
    pid: u32,
    lines: mpsc::Receiver<Read>,
    output: OwnedWriteHalf,
    reader: JoinHandle<()>,
}

impl Drop for Link {
    fn drop(&mut self) {
        self.reader.abort();
    }
}

/// What one side sent.
#[derive(Debug)]
enum Read {
    Line(Vec<u8>),
    /// The side closed its end.
    End,
    /// A read failed, or a line was over the side's limit.
    Broken(String),
}

/// Why a hello didn't give a link.
enum Failed {
    /// No engine listens.
    Absent(String),
    /// An engine answered and refused this bridge, or none can ever be reached.
    Refused(String),
    /// Anything else, which a retry may get past.
    Retry(String),
}

/// Reaches the engine, starting one if there is none, within `BUDGET`.
pub(crate) async fn connect(target: &Target) -> Result<Link, String> {
    let deadline = Instant::now() + BUDGET;
    let mut starts = 0;
    let mut retry = RETRY_FIRST;
    loop {
        let detail = match say_hello(target).await {
            Ok(link) => return Ok(link),
            Err(Failed::Refused(detail)) => return Err(detail),
            Err(Failed::Absent(detail)) if starts < MAX_STARTS => {
                starts += 1;
                start(target, deadline).await?;
                detail
            }
            Err(Failed::Absent(detail) | Failed::Retry(detail)) => detail,
        };
        if Instant::now() + retry >= deadline {
            return Err(format!(
                "no shared engine answered within {} s: {detail}",
                BUDGET.as_secs()
            ));
        }
        tokio::time::sleep(retry).await;
        retry = (retry * 2).min(RETRY_MAX);
    }
}

async fn say_hello(target: &Target) -> Result<Link, Failed> {
    let socket = target.runtime.path().join(SOCKET);
    let stream = match tokio::time::timeout(CONNECT, UnixStream::connect(&socket)).await {
        Ok(Ok(stream)) => stream,
        Ok(Err(error)) => return Err(connect_failed(&socket, &error)),
        Err(_) => {
            return Err(Failed::Retry(format!(
                "connect to {}: timed out",
                socket.display()
            )));
        }
    };
    let (input, mut output) = stream.into_split();
    let mut input = BufReader::new(input);
    let exchange = async {
        output
            .write_all(&target.hello)
            .await
            .map_err(|error| error.to_string())?;
        match read_line(&mut input, hello::MAX_LINE).await {
            Read::Line(line) => Ok(line),
            Read::End => Err("the engine closed the connection".to_owned()),
            Read::Broken(detail) => Err(detail),
        }
    };
    let reply = tokio::time::timeout(hello::DEADLINE, exchange)
        .await
        .map_err(|_| Failed::Retry("the engine didn't answer the hello in time".to_owned()))?
        .map_err(|detail| Failed::Retry(format!("the hello: {detail}")))?;
    match serde_json::from_slice::<Reply>(&reply) {
        Ok(Reply::Engine { pid }) => Ok(Link::new(pid, input, output)),
        Ok(Reply::Refused { error, detail }) => Err(Failed::Refused(format!(
            "the shared engine refused this client ({}): {detail}",
            serde_json::to_value(error)
                .ok()
                .and_then(|name| name.as_str().map(str::to_owned))
                .unwrap_or_default()
        ))),
        Err(error) => Err(Failed::Retry(format!("read the engine's hello: {error}"))),
    }
}

/// What a failed connect to `socket` means: no engine listens, none ever can, such as on
/// a path over the 108 bytes a Unix socket's may have, or a retry may help.
fn connect_failed(socket: &Path, error: &std::io::Error) -> Failed {
    let detail = format!("connect to {}: {error}", socket.display());
    let kind = error.kind();
    if matches!(
        kind,
        std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
    ) {
        Failed::Absent(detail)
    } else if kind == std::io::ErrorKind::InvalidInput {
        Failed::Refused(detail)
    } else {
        Failed::Retry(detail)
    }
}

impl Link {
    fn new(
        pid: u32,
        input: BufReader<tokio::net::unix::OwnedReadHalf>,
        output: OwnedWriteHalf,
    ) -> Self {
        let (sender, lines) = mpsc::channel(1);
        let reader = tokio::spawn(pass_lines(input, MAX_ENGINE_LINE, sender));
        Self {
            pid,
            lines,
            output,
            reader,
        }
    }

    async fn next(&mut self) -> Read {
        self.lines.recv().await.unwrap_or(Read::End)
    }

    /// Sends the client's `initialize`, the request `id`, and waits for its response,
    /// which isn't passed on, then sends `notifications/initialized`.
    async fn initialize(&mut self, id: &Value, line: &[u8]) -> Result<(), String> {
        self.output
            .write_all(line)
            .await
            .map_err(|error| error.to_string())?;
        let response = Message::Response { id: id.clone() };
        loop {
            match self.next().await {
                Read::Line(answer) if envelope::read(&answer) == response => break,
                Read::Line(_) => {}
                Read::End => return Err("the engine closed the connection".to_owned()),
                Read::Broken(detail) => return Err(detail),
            }
        }
        let initialized = b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n";
        self.output
            .write_all(initialized)
            .await
            .map_err(|error| error.to_string())
    }
}

/// Starts an engine, unless another bridge has meanwhile, and waits for it to listen. The
/// runtime directory is created first: it may never have existed, or have been removed
/// since, which ended the engine this bridge served.
async fn start(target: &Target, deadline: Instant) -> Result<(), String> {
    let runtime = target.runtime.path();
    target
        .runtime
        .create()
        .map_err(|error| format!("create {}: {error}", runtime.display()))?;
    let Some(lock) = start_lock(&target.runtime, deadline).await? else {
        return Ok(());
    };
    let socket = target.runtime.path().join(SOCKET);
    if UnixStream::connect(&socket).await.is_err() {
        let path = target.runtime.path().join(LOG);
        let log = OpenOptions::new()
            .append(true)
            .create(true)
            .mode(0o600)
            .open(&path)
            .map_err(|error| format!("open {}: {error}", path.display()))?;
        runner::daemon("/proc/self/exe", &["engine".to_owned()], log)
            .map_err(|error| error.detail)?;
        let listening = deadline.min(Instant::now() + LISTENING);
        while UnixStream::connect(&socket).await.is_err() && Instant::now() < listening {
            tokio::time::sleep(RETRY_FIRST).await;
        }
    }
    drop(lock);
    Ok(())
}

/// Takes `engine.start.lock`, or `None` once `deadline` has passed.
async fn start_lock(runtime: &RuntimeDir, deadline: Instant) -> Result<Option<File>, String> {
    let path = runtime.path().join(START_LOCK);
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(&path)
        .map_err(|error| format!("open {}: {error}", path.display()))?;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(Some(file)),
            Err(TryLockError::WouldBlock) => {}
            Err(TryLockError::Error(error)) => {
                return Err(format!("lock {}: {error}", path.display()));
            }
        }
        if Instant::now() >= deadline {
            return Ok(None);
        }
        tokio::time::sleep(RETRY_FIRST).await;
    }
}

/// Reads lines of at most `max` bytes from `input` and passes them on, then how it ended.
async fn pass_lines(input: impl AsyncRead + Unpin, max: usize, lines: mpsc::Sender<Read>) {
    let mut input = BufReader::new(input);
    loop {
        let line = read_line(&mut input, max).await;
        let more = matches!(line, Read::Line(_));
        if lines.send(line).await.is_err() || !more {
            return;
        }
    }
}

async fn read_line(input: &mut (impl tokio::io::AsyncBufRead + Unpin), max: usize) -> Read {
    let mut line = Vec::new();
    let limit = u64::try_from(max).unwrap_or(u64::MAX).saturating_add(1);
    match (&mut *input).take(limit).read_until(b'\n', &mut line).await {
        Err(error) => Read::Broken(error.to_string()),
        Ok(0) => Read::End,
        Ok(_) if !LineLimit::new(max).count(&line) => {
            Read::Broken(format!("sent a line over {max} bytes"))
        }
        Ok(_) => Read::Line(line),
    }
}

/// Relays between the client on stdin and stdout and the engine `engine` reaches, until the
/// client's end.
pub(crate) async fn run(target: Target, engine: Link) -> Result<(), String> {
    let (sender, mut client) = mpsc::channel(1);
    tokio::spawn(pass_lines(tokio::io::stdin(), MAX_LINE, sender));
    let mut relay = Relay {
        target,
        engine: Some(engine),
        in_flight: BTreeMap::new(),
        initialize: None,
        unreported: false,
    };
    loop {
        tokio::select! {
            line = client.recv() => match line {
                Some(Read::Line(line)) => relay.client_line(line).await?,
                Some(Read::Broken(detail)) => {
                    return Err(format!("the client {detail}; its session ends"));
                }
                Some(Read::End) | None => return relay.drain().await,
            },
            line = relay.engine_read() => match line {
                Read::Line(line) => relay.reply(&line).await?,
                Read::End => relay.lost("closed the connection").await?,
                Read::Broken(detail) => relay.lost(&detail).await?,
            },
        }
    }
}

/// The relay's state.
struct Relay {
    target: Target,
    engine: Option<Link>,
    /// The client's requests the engine hasn't answered, by their ids as JSON, with the
    /// method.
    in_flight: BTreeMap<String, (Value, String)>,
    /// The client's `initialize`: its id and line, replayed to a new engine.
    initialize: Option<(Value, Vec<u8>)>,
    /// The engine was lost while nothing was in flight, and the client hasn't heard.
    unreported: bool,
}

impl Relay {
    async fn engine_read(&mut self) -> Read {
        match &mut self.engine {
            Some(engine) => engine.next().await,
            None => std::future::pending().await,
        }
    }

    async fn client_line(&mut self, line: Vec<u8>) -> Result<(), String> {
        match envelope::read(&line) {
            Message::Request { id, method } => self.request(id, method, line).await,
            // Without an engine, notifications are dropped, and so is a cancellation of a
            // request already answered.
            Message::Notification { cancels } => {
                let answered =
                    cancels.is_some_and(|id| !self.in_flight.contains_key(&id.to_string()));
                if answered {
                    return Ok(());
                }
                self.send(&line).await
            }
            Message::Response { .. } | Message::Other => self.send(&line).await,
        }
    }

    async fn request(&mut self, id: Value, method: String, line: Vec<u8>) -> Result<(), String> {
        let initializing = method == "initialize";
        if initializing {
            self.initialize = Some((id.clone(), line.clone()));
        }
        if self.engine.is_none() {
            if std::mem::take(&mut self.unreported) {
                return self
                    .answer(&id, &method, self.lost_error("since the last call"))
                    .await;
            }
            match self.reconnect(!initializing).await {
                Ok(engine) => self.engine = Some(engine),
                Err(detail) => {
                    let error = ToolError::new(
                        ErrorName::EngineUnavailable,
                        format!("{detail}; the next call tries again"),
                    );
                    return self.answer(&id, &method, error).await;
                }
            }
        }
        self.in_flight.insert(id.to_string(), (id, method));
        self.send(&line).await
    }

    /// Reaches an engine again and, with `replay`, sends it the client's `initialize`. Its
    /// response is the first the new engine sends, matched by the client's own id, and
    /// isn't passed on; `notifications/initialized` follows.
    async fn reconnect(&self, replay: bool) -> Result<Link, String> {
        let mut engine = connect(&self.target).await?;
        let Some((id, line)) = self.initialize.as_ref().filter(|_| replay) else {
            return Ok(engine);
        };
        tokio::time::timeout(hello::DEADLINE, engine.initialize(id, line))
            .await
            .map_err(|_| "the new engine didn't answer initialize in time".to_owned())?
            .map_err(|detail| format!("initialize the new engine: {detail}"))?;
        Ok(engine)
    }

    async fn send(&mut self, line: &[u8]) -> Result<(), String> {
        let Some(engine) = &mut self.engine else {
            return Ok(());
        };
        match tokio::time::timeout(ENGINE_WRITE, engine.output.write_all(line)).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => self.lost(&format!("couldn't be written to: {error}")).await,
            Err(_) => {
                let how = format!("stopped reading for {} s", ENGINE_WRITE.as_secs());
                self.lost(&how).await
            }
        }
    }

    async fn reply(&mut self, line: &[u8]) -> Result<(), String> {
        if let Message::Response { id } = envelope::read(line) {
            self.in_flight.remove(&id.to_string());
        }
        let mut stdout = tokio::io::stdout();
        stdout
            .write_all(line)
            .await
            .map_err(|error| format!("write to the client: {error}"))?;
        if !line.ends_with(b"\n") {
            stdout
                .write_all(b"\n")
                .await
                .map_err(|error| format!("write to the client: {error}"))?;
        }
        stdout
            .flush()
            .await
            .map_err(|error| format!("write to the client: {error}"))
    }

    /// The engine is gone: every request in flight gets `engine_lost`, or the next one
    /// does when none was.
    async fn lost(&mut self, how: &str) -> Result<(), String> {
        let error = self.lost_error(how);
        self.engine = None;
        self.unreported = self.in_flight.is_empty();
        for (id, method) in std::mem::take(&mut self.in_flight).into_values() {
            self.answer(&id, &method, error.clone()).await?;
        }
        Ok(())
    }

    fn lost_error(&self, how: &str) -> ToolError {
        let engine = self.engine.as_ref().map_or_else(
            || "the shared engine".to_owned(),
            |engine| format!("the shared engine (PID {})", engine.pid),
        );
        ToolError::new(
            ErrorName::EngineLost,
            format!(
                "{engine} ended ({how}); what a call in flight did is unknown. The next call reaches a new engine, where this session holds no lease"
            ),
        )
    }

    async fn answer(&mut self, id: &Value, method: &str, error: ToolError) -> Result<(), String> {
        self.reply(&envelope::answer(id, method, error)).await
    }

    /// The client has ended: the engine hears so, and what it still sends, for up to
    /// `DRAIN`, is passed on.
    async fn drain(mut self) -> Result<(), String> {
        let Some(mut engine) = self.engine.take() else {
            return Ok(());
        };
        engine.output.shutdown().await.ok();
        let until = Instant::now() + DRAIN;
        while let Ok(Read::Line(line)) = tokio::time::timeout_at(until, engine.next()).await {
            self.reply(&line).await?;
        }
        Ok(())
    }
}
