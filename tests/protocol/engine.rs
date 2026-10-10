//! The engine on its own, as a bridge sees it: a hello line each way on the instance
//! socket, then MCP over the same connection. The engine is started directly here.

use std::os::unix::fs::MetadataExt as _;
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader, Lines};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::process::Child;

use crate::client::{CLIENT, WAIT, engine};
use crate::fixture::{DISPLAY, Fixture, eventually};

fn socket(fixture: &Fixture) -> PathBuf {
    fixture.runtime_dir().join("engine.sock")
}

/// The hello of a bridge running this build for the fixture's niri and display.
fn hello(fixture: &Fixture) -> Value {
    let exe = std::fs::metadata(env!("CARGO_BIN_EXE_niri-computer-use")).unwrap();
    json!({
        "hello": 1,
        "exe": {
            "dev": exe.dev(), "ino": exe.ino(), "size": exe.size(),
            "mtime": exe.mtime(), "mtime_nsec": exe.mtime_nsec()
        },
        "niri_socket": fixture.niri_socket(),
        "wayland_socket": fixture.path(&format!("run/{DISPLAY}")),
        "unrestricted": null,
        "keyboard": null,
        "home": null,
        "policy": "missing",
    })
}

/// Starts an engine and waits for its socket.
async fn start(fixture: &Fixture) -> Child {
    let child = engine(fixture);
    assert!(
        eventually(WAIT, || socket(fixture).exists()).await,
        "the engine never listened"
    );
    child
}

/// One connection to the engine.
struct Connection {
    lines: Lines<BufReader<OwnedReadHalf>>,
    output: OwnedWriteHalf,
}

impl Connection {
    /// Connects and sends `hello`. Returns the connection and the engine's reply.
    async fn open(fixture: &Fixture, hello: &Value) -> (Self, Value) {
        let (input, output) = UnixStream::connect(socket(fixture))
            .await
            .unwrap()
            .into_split();
        let mut connection = Self {
            lines: BufReader::new(input).lines(),
            output,
        };
        connection.send(hello).await;
        let reply = connection.next().await.expect("no reply to the hello");
        (connection, reply)
    }

    /// Connects as a session that has done MCP's handshake.
    async fn session(fixture: &Fixture) -> Self {
        let (mut connection, reply) = Self::open(fixture, &hello(fixture)).await;
        assert!(reply["engine"]["pid"].is_u64(), "{reply}");
        let params = json!({
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "clientInfo": {"name": CLIENT, "version": "1"}
        });
        let initialized = connection.request(1, "initialize", params).await;
        assert!(initialized.get("result").is_some(), "{initialized}");
        connection
            .send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
            .await;
        connection
    }

    async fn send(&mut self, message: &Value) {
        let mut line = serde_json::to_vec(message).unwrap();
        line.push(b'\n');
        self.output.write_all(&line).await.unwrap();
    }

    /// The next line, or `None` at the end.
    async fn next(&mut self) -> Option<Value> {
        let line = tokio::time::timeout(WAIT, self.lines.next_line())
            .await
            .expect("nothing from the engine")
            .ok()??;
        Some(serde_json::from_str(&line).unwrap())
    }

    async fn request(&mut self, id: u64, method: &str, params: Value) -> Value {
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .await;
        loop {
            let message = self.next().await.expect("the engine closed the connection");
            if message["id"] == id {
                return message;
            }
        }
    }

    async fn status(&mut self, id: u64) -> Value {
        let params = json!({"name": "status", "arguments": {}});
        self.request(id, "tools/call", params).await["result"]["structuredContent"].clone()
    }
}

async fn exits_within(child: &mut Child, limit: Duration) -> Option<std::process::ExitStatus> {
    tokio::time::timeout(limit, child.wait())
        .await
        .ok()
        .map(Result::unwrap)
}

#[tokio::test]
async fn the_engine_takes_a_matching_hello_and_refuses_the_rest() {
    let fixture = Fixture::new("engine-hello");
    let child = start(&fixture).await;
    let mut session = Connection::session(&fixture).await;
    assert_eq!(session.status(2).await["lease"]["held_by_me"], false);
    let refused = |mut hello: Value, change: &dyn Fn(&mut Value)| {
        change(&mut hello);
        hello
    };
    for (sent, error) in [
        (
            refused(hello(&fixture), &|hello| hello["hello"] = 2.into()),
            "engine_version",
        ),
        (
            refused(hello(&fixture), &|hello| {
                hello["exe"]["size"] = (hello["exe"]["size"].as_u64().unwrap() + 1).into();
            }),
            "engine_version",
        ),
        (
            refused(hello(&fixture), &|hello| {
                hello["niri_socket"] = "/run/user/1/niri.other.sock".into();
            }),
            "session_mismatch",
        ),
        (json!({"hello": 1}), "bad_hello"),
    ] {
        let (mut connection, reply) = Connection::open(&fixture, &sent).await;
        assert_eq!(reply["refused"]["error"], error, "{reply}");
        assert_eq!(connection.next().await, None);
    }
    let (_, reply) = Connection::open(&fixture, &hello(&fixture)).await;
    assert_eq!(reply, json!({"engine": {"pid": child.id().unwrap()}}));
}

#[tokio::test]
async fn a_line_over_the_limit_ends_only_that_session() {
    let fixture = Fixture::new("engine-line");
    let _child = start(&fixture).await;
    let mut long = Connection::session(&fixture).await;
    let mut other = Connection::session(&fixture).await;
    let over = vec![b'x'; 16 * 1024 * 1024 + 1];
    // The engine may stop reading, and close, before it has all of it.
    long.output.write_all(&over).await.ok();
    assert_eq!(long.next().await, None);
    assert_eq!(other.status(2).await["lease"]["held_by_me"], false);
}

#[tokio::test]
async fn a_second_engine_for_the_instance_leaves_at_once() {
    let fixture = Fixture::new("engine-second");
    let _first = start(&fixture).await;
    let mut second = engine(&fixture);
    let status = exits_within(&mut second, Duration::from_secs(3))
        .await
        .expect("the second engine stayed");
    assert!(status.success(), "{status}");
    Connection::session(&fixture).await;
}

#[tokio::test]
async fn an_idle_engine_exits_and_removes_its_socket() {
    let fixture = Fixture::new("engine-idle");
    let mut child = start(&fixture).await;
    let session = Connection::session(&fixture).await;
    // While a client is connected, the engine stays.
    assert!(
        exits_within(&mut child, Duration::from_secs(3))
            .await
            .is_none()
    );
    drop(session);
    let status = exits_within(&mut child, Duration::from_secs(4))
        .await
        .expect("the engine stayed idle");
    assert!(status.success(), "{status}");
    assert!(!socket(&fixture).exists());
}

#[tokio::test]
async fn removing_the_runtime_directory_ends_the_engine_and_its_sessions() {
    let fixture = Fixture::new("engine-gone");
    let mut child = start(&fixture).await;
    let mut session = Connection::session(&fixture).await;
    std::fs::remove_dir_all(fixture.runtime_dir()).unwrap();
    assert_eq!(session.next().await, None);
    assert!(
        exits_within(&mut child, Duration::from_secs(5))
            .await
            .is_some()
    );
}

#[tokio::test]
async fn the_engine_listens_before_its_slow_start() {
    let mut fixture = Fixture::new("engine-early");
    // A session bus that accepts and never answers holds the accessibility lookup up to
    // its deadline.
    let bus = UnixListener::bind(fixture.path("bus")).unwrap();
    let mut address = std::ffi::OsString::from("unix:path=");
    address.push(fixture.path("bus"));
    fixture.set("DBUS_SESSION_BUS_ADDRESS", address);
    let started = Instant::now();
    let _child = start(&fixture).await;
    let listening = started.elapsed();
    let (_, reply) = Connection::open(&fixture, &hello(&fixture)).await;
    assert!(reply["engine"]["pid"].is_u64(), "{reply}");
    let answered = started.elapsed();
    assert!(
        listening + Duration::from_millis(500) < answered,
        "listening at {listening:?}, answered at {answered:?}"
    );
    drop(bus);
}

#[tokio::test]
async fn past_sixty_four_connections_the_engine_is_busy() {
    let fixture = Fixture::new("engine-busy");
    let _engine = start(&fixture).await;
    // A connection counts from its accept, before its hello.
    let mut waiting = Vec::new();
    for _ in 0..63 {
        waiting.push(UnixStream::connect(socket(&fixture)).await.unwrap());
    }
    let (_last, served) = Connection::open(&fixture, &hello(&fixture)).await;
    assert!(served["engine"]["pid"].is_u64(), "{served}");
    let (mut over, refused) = Connection::open(&fixture, &hello(&fixture)).await;
    assert_eq!(refused["refused"]["error"], "engine_busy", "{refused}");
    assert_eq!(over.next().await, None);
    drop(waiting);
    // A connection that closes stops counting.
    let since = Instant::now();
    loop {
        let (_again, reply) = Connection::open(&fixture, &hello(&fixture)).await;
        if reply["engine"]["pid"].is_u64() {
            break;
        }
        assert!(since.elapsed() < WAIT, "{reply}");
    }
}
