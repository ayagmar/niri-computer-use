//! The engine's host: one engine per niri instance, which serves each client's session
//! over its own connection to `<runtime dir>/engine.sock`. `engine.lock` decides which
//! engine that is. The engine listens before anything slow starts, so connections queue
//! meanwhile, and exits once it has been idle for `IDLE_GRACE`, or at once when the stop
//! watcher ends.

use std::fs::{File, OpenOptions, TryLockError};
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
use std::sync::Arc;
use std::time::Duration;

use rmcp::ServiceExt as _;
use tokio::io::{
    AsyncBufReadExt as _, AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _, BufReader,
};
use tokio::net::{UnixListener, UnixStream};
use tokio::task::JoinSet;
use tokio::time::Instant;

use super::Engine;
use super::hello::{self, Own, Refusal, Reply};
use crate::control::runtime::{RuntimeDir, identity};
use crate::session::{Incoming, Session, Settings};
use crate::status::Mode;
use crate::{Env, cli, control, tools};

const LOCK: &str = "engine.lock";
pub(crate) const SOCKET: &str = "engine.sock";
pub(crate) const LOG: &str = "engine.log";
/// How long a new engine waits for an old one, in its idle exit, to let go of the lock.
const LOCK_WAIT: Duration = Duration::from_secs(1);
const LOCK_RETRY: Duration = Duration::from_millis(20);
const MAX_CONNECTIONS: usize = 64;
/// How long the engine stays with no connection, no lease held and no cleanup pending.
const IDLE_GRACE: Duration = Duration::from_secs(2);
const IDLE_CHECK: Duration = Duration::from_millis(100);
/// How long it waits after a failed accept, such as one out of file descriptors.
const ACCEPT_RETRY: Duration = Duration::from_millis(100);

/// Runs the engine for the niri instance `env` names, unless another one does already.
pub(crate) async fn run(env: Env) -> Result<(), String> {
    let runtime = RuntimeDir::of(&env)?;
    runtime
        .create()
        .map_err(|error| format!("create {}: {error}", runtime.path().display()))?;
    let Some(lock) = lock(&runtime).await? else {
        // Another engine serves this instance.
        return Ok(());
    };
    truncate_log(&runtime)?;
    let (listener, bound) = listen(&runtime)?;
    let own = Own {
        exe: hello::Exe::current()?,
        niri_socket: env
            .niri_socket
            .path()
            .map_err(|error| error.detail.clone())?
            .to_path_buf(),
        wayland_socket: env.display.path().ok().map(std::path::Path::to_path_buf),
    };
    let engine = Arc::new(Engine::start(env, Mode::Shared, None).await?);
    let served = accept(&listener, &engine, &Arc::new(own)).await;
    drop(listener);
    unlink_own_socket(&runtime, bound);
    engine.end_sessions().await;
    control::cleanup::settled().await;
    drop(lock);
    served
}

/// Serves `session` over `input` and `output` until its client goes, which ends the
/// session at once, then stops counting it.
pub(crate) async fn serve_session<R, W>(
    engine: &Arc<Engine>,
    session: Session,
    input: R,
    output: W,
) -> Result<(), String>
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let (input, gone) = Incoming::new(input);
    // Without this, rmcp lets a running call finish, for up to 5 s, after the client's end.
    tokio::spawn({
        let (engine, session) = (Arc::clone(engine), session.clone());
        async move {
            gone.await.ok();
            engine.end_session(&session).await;
        }
    });
    let served = match tools::Server::new(Arc::clone(engine), session.clone())
        .serve((input, output))
        .await
    {
        Ok(service) => service
            .waiting()
            .await
            .map(drop)
            .map_err(|error| format!("MCP session: {error}")),
        Err(error) => Err(format!("start MCP session: {error}")),
    };
    engine.close_session(&session).await;
    served
}

/// Accepts connections until the engine has been idle for `IDLE_GRACE`, or the stop
/// watcher ends.
async fn accept(
    listener: &UnixListener,
    engine: &Arc<Engine>,
    own: &Arc<Own>,
) -> Result<(), String> {
    let mut connections = JoinSet::new();
    let mut idle_since = Some(Instant::now());
    let mut check = tokio::time::interval(IDLE_CHECK);
    let watcher_ended = engine.watcher_ended();
    tokio::pin!(watcher_ended);
    loop {
        tokio::select! {
            () = &mut watcher_ended => return Err(
                "the stop watcher ended, because the runtime directory was removed or replaced; the engine exits".to_owned(),
            ),
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => {
                    let busy = connections.len() >= MAX_CONNECTIONS;
                    connections.spawn(connection(Arc::clone(engine), Arc::clone(own), stream, busy));
                }
                Err(error) => {
                    cli::print_error(&format!("accept a connection: {error}"));
                    tokio::time::sleep(ACCEPT_RETRY).await;
                }
            },
            Some(_) = connections.join_next() => {}
            _ = check.tick() => {
                if !connections.is_empty() || !engine.settled() {
                    idle_since = None;
                } else if idle_since.get_or_insert_with(Instant::now).elapsed() >= IDLE_GRACE {
                    return Ok(());
                }
            }
        }
    }
}

/// One client's connection: the hello, then its session.
async fn connection(engine: Arc<Engine>, own: Arc<Own>, stream: UnixStream, busy: bool) {
    let Ok(peer) = stream.peer_cred() else {
        return;
    };
    // Same-user processes could already trace the engine; this keeps out other users.
    if peer.uid() != rustix::process::geteuid().as_raw() {
        return;
    }
    let (input, mut output) = stream.into_split();
    let mut input = BufReader::new(input);
    let Ok(Some(line)) = tokio::time::timeout(hello::DEADLINE, read_hello(&mut input)).await else {
        return;
    };
    let taken = if busy {
        Err((
            Refusal::EngineBusy,
            format!("the engine serves {MAX_CONNECTIONS} connections already"),
        ))
    } else {
        own.take(&line)
    };
    let hello = match taken {
        Ok(hello) => hello,
        Err((error, detail)) => {
            reply(&mut output, &Reply::Refused { error, detail })
                .await
                .ok();
            return;
        }
    };
    let pid = peer
        .pid()
        .and_then(|pid| u32::try_from(pid).ok())
        .unwrap_or(0);
    let session = engine.open_session(pid, Settings::new(hello.given()));
    let engine_pid = std::process::id();
    if reply(&mut output, &Reply::Engine { pid: engine_pid })
        .await
        .is_err()
    {
        engine.close_session(&session).await;
        return;
    }
    if let Err(error) = serve_session(&engine, session, input, output).await {
        cli::print_error(&error);
    }
}

/// The hello line, if one ends within `hello::MAX_LINE` bytes.
async fn read_hello(input: &mut BufReader<tokio::net::unix::OwnedReadHalf>) -> Option<Vec<u8>> {
    let mut line = Vec::new();
    let limit = u64::try_from(hello::MAX_LINE).unwrap_or(u64::MAX);
    (&mut *input)
        .take(limit)
        .read_until(b'\n', &mut line)
        .await
        .ok()?;
    line.ends_with(b"\n").then_some(line)
}

async fn reply(output: &mut (impl AsyncWrite + Unpin), reply: &Reply) -> std::io::Result<()> {
    let mut line = serde_json::to_vec(reply).map_err(std::io::Error::other)?;
    line.push(b'\n');
    tokio::time::timeout(hello::DEADLINE, output.write_all(&line))
        .await
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::TimedOut))?
}

/// Takes `engine.lock`, waiting up to `LOCK_WAIT` for an engine in its idle exit. `None`
/// when another engine holds it.
async fn lock(runtime: &RuntimeDir) -> Result<Option<File>, String> {
    let path = runtime.path().join(LOCK);
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(&path)
        .map_err(|error| format!("open {}: {error}", path.display()))?;
    let deadline = Instant::now() + LOCK_WAIT;
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
        tokio::time::sleep(LOCK_RETRY).await;
    }
}

/// Empties the engine's log, which a bridge opened for its stderr, for this engine's run.
fn truncate_log(runtime: &RuntimeDir) -> Result<(), String> {
    let path = runtime.path().join(LOG);
    OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&path)
        .map(drop)
        .map_err(|error| format!("truncate {}: {error}", path.display()))
}

/// Binds `engine.sock` afresh, readable by the user alone. Only the lock holder does, so a
/// socket already there is stale.
fn listen(runtime: &RuntimeDir) -> Result<(UnixListener, (u64, u64)), String> {
    let path = runtime.path().join(SOCKET);
    match std::fs::remove_file(&path) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            return Err(format!("remove {}: {error}", path.display()));
        }
        _ => {}
    }
    let listener =
        UnixListener::bind(&path).map_err(|error| format!("bind {}: {error}", path.display()))?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
        .map_err(|error| format!("chmod {}: {error}", path.display()))?;
    let bound = identity(&path).map_err(|error| format!("stat {}: {error}", path.display()))?;
    Ok((listener, bound))
}

/// Removes `engine.sock` if it is still the socket this engine bound. After the runtime
/// directory was replaced, the path may name a new engine's socket, which must stay.
fn unlink_own_socket(runtime: &RuntimeDir, bound: (u64, u64)) {
    let path = runtime.path().join(SOCKET);
    if identity(&path).ok() == Some(bound) {
        std::fs::remove_file(path).ok();
    }
}
