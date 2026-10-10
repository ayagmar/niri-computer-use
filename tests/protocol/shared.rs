//! Shared mode over stdio: `serve` as a bridge to the one engine of its niri instance,
//! which the first bridge starts.

use std::fs::File;
use std::os::unix::net::UnixListener;
use std::time::{Duration, Instant};

use serde_json::json;

use crate::client::{CLIENT, Server, WAIT, tool_error};
use crate::fixture::{Fixture, eventually, jpeg, kill};
use crate::niri::{Niri, Stream, window_on};
use crate::noctalia::{self, UNLOCKED};
use crate::session::NiriProcess;

/// A fixture whose servers run in shared mode.
fn shared(name: &str) -> Fixture {
    let mut fixture = Fixture::new(name);
    fixture.set("NIRI_COMPUTER_USE_SHARED", "1");
    fixture
}

/// The arguments and environment of each process of the user's.
fn processes() -> Vec<(i32, Vec<String>, Vec<String>)> {
    let split = |bytes: Vec<u8>| -> Vec<String> {
        bytes
            .split(|byte| *byte == 0)
            .map(|part| String::from_utf8_lossy(part).into_owned())
            .collect()
    };
    std::fs::read_dir("/proc")
        .unwrap()
        .filter_map(|entry| {
            let pid = entry.ok()?.file_name().to_str()?.parse().ok()?;
            let args = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
            let environ = std::fs::read(format!("/proc/{pid}/environ")).ok()?;
            Some((pid, split(args), split(environ)))
        })
        .collect()
}

/// The engines running for the fixture's niri.
fn engines(fixture: &Fixture) -> Vec<i32> {
    let niri = format!("NIRI_SOCKET={}", fixture.niri_socket().display());
    processes()
        .into_iter()
        .filter(|(_, args, environ)| {
            args.get(1).is_some_and(|arg| arg == "engine") && environ.contains(&niri)
        })
        .map(|(pid, _, _)| pid)
        .collect()
}

/// The one engine for the fixture's niri, once there is exactly one.
async fn engine(fixture: &Fixture) -> i32 {
    assert!(
        eventually(WAIT, || engines(fixture).len() == 1).await,
        "engines: {:?}",
        engines(fixture)
    );
    engines(fixture)[0]
}

/// How many crash guardians watch `server`.
fn guardians(server: i32) -> usize {
    processes()
        .into_iter()
        .filter(|(_, args, _)| {
            args.get(1).is_some_and(|arg| arg == "guard")
                && args.get(2).is_some_and(|arg| *arg == server.to_string())
        })
        .count()
}

/// Holds `engine.lock`, so that every engine started meanwhile leaves at once.
fn hold_engine_lock(fixture: &Fixture) -> File {
    std::fs::create_dir_all(fixture.runtime_dir()).unwrap();
    let lock = File::create(fixture.runtime_dir().join("engine.lock")).unwrap();
    lock.lock().unwrap();
    lock
}

/// A niri and an unlocked Noctalia, so that a client can take the lease.
struct Desktop {
    niri: Niri,
    _noctalia: noctalia::Reply,
}

impl Desktop {
    fn new(fixture: &Fixture) -> Self {
        fixture.program("noctalia", "exit 0");
        Self {
            niri: Niri::start(fixture),
            _noctalia: noctalia::start(fixture, UNLOCKED),
        }
    }

    /// The event stream the engine opens, with one window focused on it.
    async fn stream(&mut self) -> Stream {
        let stream = self.niri.stream().await;
        stream.workspaces(1);
        stream.send(&json!({"WindowsChanged": {"windows": [window_on(1, Some("a"), 1, true)]}}));
        stream.send(&json!({"OverviewOpenedOrClosed": {"is_open": false}}));
        stream
    }
}

#[tokio::test]
async fn a_server_without_shared_mode_serves_its_one_client_itself() {
    let mut fixture = Fixture::new("standalone");
    fixture.unset("NIRI_COMPUTER_USE_SHARED");
    let _niri = Niri::start(&fixture);
    let mut server = Server::start(&fixture).await;
    assert_eq!(
        server.structured("status").await["engine"],
        json!({"pid": server.pid, "mode": "standalone", "sessions": 1, "fallback": null})
    );
    assert_eq!(engines(&fixture), Vec::<i32>::new());
}

#[tokio::test]
async fn ten_clients_share_one_engine_and_its_lease() {
    let fixture = shared("shared-ten");
    let _niri = NiriProcess::unlocked(&fixture).await;
    let mut servers = Vec::new();
    for _ in 0..10 {
        servers.push(Server::spawn(&fixture));
    }
    for server in &mut servers {
        let response = server.initialize("2025-11-25").await;
        assert!(response.get("result").is_some(), "{response}");
    }
    let engine = engine(&fixture).await;
    assert_eq!(guardians(engine), 1);
    for server in &mut servers {
        assert_eq!(
            server.structured("status").await["engine"],
            json!({"pid": engine, "mode": "shared", "sessions": 10, "fallback": null})
        );
    }
    let (first, rest) = servers.split_first_mut().unwrap();
    let second = &mut rest[0];
    let label = format!("{CLIENT}/{}", first.pid);
    assert_eq!(
        first.structured("acquire_desktop").await["holder"]["label"],
        label.as_str()
    );
    let (name, detail) = tool_error(&second.call("acquire_desktop", json!({})).await);
    assert_eq!(name, "lease_held");
    assert!(detail.contains(&format!("({label})")), "{detail}");
    let sessions: Vec<_> = fixture
        .audit_lines()
        .into_iter()
        .filter(|line| line["tool"] == "acquire_desktop")
        .map(|line| line["session"].clone())
        .collect();
    assert_eq!(
        sessions,
        [json!(label), json!(format!("{CLIENT}/{}", second.pid))]
    );
}

#[tokio::test]
async fn unrestricted_is_the_clients_own_in_shared_mode() {
    let mut fixture = shared("shared-unrestricted");
    let _niri = Niri::start(&fixture);
    fixture.set("NIRI_COMPUTER_USE_UNRESTRICTED", "1");
    let mut on = Server::start(&fixture).await;
    fixture.unset("NIRI_COMPUTER_USE_UNRESTRICTED");
    let mut off = Server::start(&fixture).await;
    engine(&fixture).await;
    assert_eq!(
        off.structured("status").await["unrestricted"]["enabled"],
        false
    );
    assert_eq!(
        on.structured("status").await["unrestricted"]["enabled"],
        true
    );
    assert_eq!(
        off.structured("status").await["unrestricted"]["enabled"],
        false
    );
}

#[tokio::test]
async fn a_client_on_another_display_is_served_standalone() {
    let mut fixture = shared("shared-mismatch");
    let _niri = Niri::start(&fixture);
    let mut bridged = Server::start(&fixture).await;
    let engine = engine(&fixture).await;
    let _other = UnixListener::bind(fixture.path("run/wayland-other")).unwrap();
    fixture.set("WAYLAND_DISPLAY", "wayland-other");
    let mut standalone = Server::start(&fixture).await;
    assert_eq!(standalone.call("status", json!({})).await["isError"], false);
    assert_eq!(engines(&fixture), [engine]);
    assert_eq!(guardians(i32::try_from(standalone.pid).unwrap()), 1);
    let (_, _, stderr) = standalone.stop().await;
    assert!(
        stderr.contains("serving this client standalone: the shared engine refused this client (session_mismatch)"),
        "{stderr}"
    );
    assert_eq!(bridged.call("status", json!({})).await["isError"], false);
}

#[tokio::test]
async fn a_lost_engine_fails_what_was_in_flight_and_the_next_call_reaches_a_new_one() {
    let fixture = shared("shared-lost");
    let _niri = Niri::start(&fixture);
    std::fs::write(fixture.path("grim.out"), jpeg(1280, 720, b"pixels")).unwrap();
    fixture.program(
        "grim",
        r#"echo >> "$DIR/grim.started"; await_file go; cat "$DIR/grim.out""#,
    );
    let mut busy = Server::start(&fixture).await;
    let mut idle = Server::start(&fixture).await;
    let first = engine(&fixture).await;
    let shot = busy
        .start_call("screenshot", json!({"target": "focused_output"}))
        .await;
    assert!(eventually(WAIT, || fixture.path("grim.started").exists()).await);
    kill(u32::try_from(first).unwrap());
    let lost = Instant::now();
    let (in_flight, detail) = tool_error(&busy.response(shot).await["result"]);
    assert_eq!(in_flight, "engine_lost");
    assert!(detail.contains(&format!("(PID {first})")), "{detail}");
    assert!(
        lost.elapsed() < Duration::from_secs(1),
        "{:?}",
        lost.elapsed()
    );
    // A client with nothing in flight hears of the loss at its next call.
    let lock = hold_engine_lock(&fixture);
    let (next, _) = tool_error(&idle.call("status", json!({})).await);
    assert_eq!(next, "engine_lost");
    let (unavailable, why) = tool_error(&idle.call("status", json!({})).await);
    assert_eq!(unavailable, "engine_unavailable");
    assert!(why.contains("the next call tries again"), "{why}");
    drop(lock);
    // Each reaches a new engine, which needs `initialize` replayed to take a call.
    assert_eq!(idle.call("status", json!({})).await["isError"], false);
    assert_eq!(busy.call("status", json!({})).await["isError"], false);
    assert_ne!(engine(&fixture).await, first);
}

#[tokio::test]
async fn a_line_over_the_limit_ends_the_bridge() {
    let fixture = shared("shared-line");
    let _niri = Niri::start(&fixture);
    let mut long = Server::start(&fixture).await;
    let mut other = Server::start(&fixture).await;
    engine(&fixture).await;
    long.send_raw(&vec![b'x'; 16 * 1024 * 1024 + 1]).await;
    let (status, unread, stderr) = long.stop().await;
    assert!(!status.success(), "{status}");
    assert_eq!(unread, Vec::<serde_json::Value>::new());
    assert_eq!(stderr.lines().count(), 1, "{stderr}");
    assert!(stderr.contains("over 16777216 bytes"), "{stderr}");
    assert_eq!(other.call("status", json!({})).await["isError"], false);
}

#[tokio::test]
async fn without_an_engine_in_time_the_client_is_served_standalone() {
    let fixture = shared("shared-cold");
    let _niri = Niri::start(&fixture);
    let _lock = hold_engine_lock(&fixture);
    let started = Instant::now();
    let mut server = Server::start(&fixture).await;
    assert!(
        started.elapsed() >= Duration::from_secs(4),
        "{:?}",
        started.elapsed()
    );
    let reported = server.structured("status").await["engine"].clone();
    assert_eq!(reported["pid"], server.pid);
    assert_eq!(reported["mode"], "standalone");
    assert!(
        reported["fallback"]
            .as_str()
            .unwrap()
            .starts_with("no shared engine answered within 5 s"),
        "{reported}"
    );
    // Engines started meanwhile all leave.
    assert!(eventually(WAIT, || engines(&fixture).is_empty()).await);
    let (_, _, stderr) = server.stop().await;
    assert!(
        stderr.contains("serving this client standalone: no shared engine answered within 5 s"),
        "{stderr}"
    );
}

/// The engine's open file descriptors.
fn fds(pid: i32) -> usize {
    std::fs::read_dir(format!("/proc/{pid}/fd"))
        .unwrap()
        .count()
}

/// How many sessions the engine serves, once its count has stopped changing.
async fn sessions(server: &mut Server) -> u64 {
    let mut last = 0;
    for _ in 0..50 {
        let now = server.structured("status").await["engine"]["sessions"]
            .as_u64()
            .unwrap();
        if now == last {
            return now;
        }
        last = now;
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    last
}

/// One client that takes the lease and leaves without giving it back.
async fn visit(fixture: &Fixture) {
    let mut server = Server::start(fixture).await;
    let taken = server.structured("acquire_desktop").await;
    assert!(taken["holder"].is_object(), "{taken}");
    let (status, _, stderr) = server.stop().await;
    assert!(status.success(), "{stderr}");
}

#[tokio::test]
async fn fifty_clients_coming_and_going_leave_the_engine_as_it_was() {
    let fixture = shared("shared-churn");
    let mut desktop = Desktop::new(&fixture);
    let mut anchor = Server::start(&fixture).await;
    let engine = engine(&fixture).await;
    let _stream = desktop.stream().await;
    // The first visit opens what the engine keeps for good, such as niri's event stream.
    visit(&fixture).await;
    assert_eq!(sessions(&mut anchor).await, 1);
    let before = fds(engine);
    for _ in 0..50 {
        visit(&fixture).await;
    }
    assert!(
        eventually(WAIT, || fds(engine) == before).await,
        "{before} file descriptors before, {} after",
        fds(engine)
    );
    assert_eq!(sessions(&mut anchor).await, 1);
    assert_eq!(engines(&fixture), [engine]);
}

#[tokio::test]
async fn a_bridge_and_a_standalone_server_share_one_lease() {
    let mut fixture = shared("shared-mixed");
    let mut desktop = Desktop::new(&fixture);
    let mut bridged = Server::start(&fixture).await;
    let _engine_stream = desktop.stream().await;
    fixture.unset("NIRI_COMPUTER_USE_SHARED");
    let mut standalone = Server::start(&fixture).await;
    let _own_stream = desktop.stream().await;
    holds_alone(&mut bridged, &mut standalone).await;
    holds_alone(&mut standalone, &mut bridged).await;
}

/// `holder` takes the lease, `other` is refused it, and `holder` gives it back.
async fn holds_alone(holder: &mut Server, other: &mut Server) {
    holder.structured("acquire_desktop").await;
    let (name, _) = tool_error(&other.call("acquire_desktop", json!({})).await);
    assert_eq!(name, "lease_held");
    let release = json!({"restore_focus": false});
    let released = holder.structured_with("release_desktop", release).await;
    assert_eq!(released["released"], true);
}

#[tokio::test]
async fn a_client_killed_mid_key_frees_the_lease_while_others_keep_working() {
    let fixture = shared("shared-killed");
    let mut desktop = Desktop::new(&fixture);
    fixture.program("wtype", r#"echo >> "$DIR/wtype.calls"; await_file go"#);
    let mut typing = Server::start(&fixture).await;
    let mut other = Server::start(&fixture).await;
    let _stream = desktop.stream().await;
    typing.structured("acquire_desktop").await;
    typing
        .start_call("key", json!({"keys": ["Down"], "expect": "none"}))
        .await;
    assert!(eventually(WAIT, || fixture.path("wtype.calls").exists()).await);
    typing.kill().await;
    let killed = Instant::now();
    let mut free = false;
    while !free && killed.elapsed() < Duration::from_secs(1) {
        free = other.structured("status").await["lease"]["holder"].is_null();
        assert_eq!(
            other.call("desktop_state", json!({})).await["isError"],
            false
        );
    }
    assert!(
        free,
        "the lease was still held {:?} after",
        killed.elapsed()
    );
    let marker = fixture.runtime_dir().join("input-dirty");
    assert!(marker.exists());
    std::fs::write(fixture.path("go"), "").unwrap();
    assert!(eventually(WAIT, || !marker.exists()).await);
    other.structured("acquire_desktop").await;
}

#[tokio::test]
async fn an_engine_socket_path_too_long_falls_back_at_once() {
    let mut fixture = shared("shared-long");
    // The engine's socket would be `<run>/niri-computer-use/<instance>/engine.sock`.
    let niri_socket = fixture.path(&format!("run/niri.{}.sock", "x".repeat(40)));
    let _niri = Niri::listen(&niri_socket);
    fixture.set("NIRI_SOCKET", &niri_socket);
    let started = Instant::now();
    let server = Server::start(&fixture).await;
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
    let (_, _, stderr) = server.stop().await;
    assert!(
        stderr.contains("serving this client standalone: connect to"),
        "{stderr}"
    );
}
