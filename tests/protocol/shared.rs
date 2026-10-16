//! Shared mode over stdio: `serve` as a bridge to the one engine of its niri instance,
//! which the first bridge starts.

use std::fs::File;
use std::os::unix::net::UnixListener;
use std::time::{Duration, Instant};

use niri_ipc::Action;
use serde_json::json;
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};

use crate::client::{CLIENT, Server, WAIT, tool_error};
use crate::fixture::{Fixture, eventually, exited, kill, signal};
use crate::framing;
use crate::niri::{Niri, Stream, window_on};
use crate::noctalia::{self, UNLOCKED};
use crate::session::NiriProcess;

/// A fixture whose servers run in shared mode, the default.
pub(crate) fn shared(name: &str) -> Fixture {
    let mut fixture = Fixture::new(name);
    fixture.unset("NIRI_COMPUTER_USE_SHARED");
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
pub(crate) fn engines(fixture: &Fixture) -> Vec<i32> {
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
pub(crate) async fn engine(fixture: &Fixture) -> i32 {
    assert!(
        eventually(WAIT, || engines(fixture).len() == 1).await,
        "engines: {:?}",
        engines(fixture)
    );
    engines(fixture)[0]
}

/// How many crash guardians watch `server`.
pub(crate) fn guardians(server: i32) -> usize {
    processes()
        .into_iter()
        .filter(|(_, args, _)| {
            args.get(1).is_some_and(|arg| arg == "guard")
                && args.get(2).is_some_and(|arg| *arg == server.to_string())
        })
        .count()
}

/// Holds `engine.lock`, so that every engine started meanwhile leaves at once.
pub(crate) fn hold_engine_lock(fixture: &Fixture) -> File {
    std::fs::create_dir_all(fixture.runtime_dir()).unwrap();
    let lock = File::create(fixture.runtime_dir().join("engine.lock")).unwrap();
    lock.lock().unwrap();
    lock
}

/// A niri and an unlocked Noctalia, so that a client can take the lease.
pub(crate) struct Desktop {
    niri: Niri,
    _noctalia: noctalia::Reply,
}

impl Desktop {
    pub(crate) fn new(fixture: &Fixture) -> Self {
        fixture.program("noctalia", "exit 0");
        Self {
            niri: Niri::start(fixture),
            _noctalia: noctalia::start(fixture, UNLOCKED),
        }
    }

    /// Holds every action niri is sent, as a niri that never answers.
    pub(crate) fn hold_actions(&self) {
        self.niri.hold_actions(true);
    }

    /// The next action niri was sent.
    pub(crate) async fn action(&mut self) -> Action {
        self.niri.action().await
    }

    /// The event stream the engine opens, with one window focused on it.
    pub(crate) async fn stream(&mut self) -> Stream {
        let stream = self.niri.stream().await;
        stream.workspaces(1);
        stream.send(&json!({"WindowsChanged": {"windows": [window_on(1, Some("a"), 1, true)]}}));
        stream.send(&json!({"OverviewOpenedOrClosed": {"is_open": false}}));
        stream
    }
}

/// `shared = false` in the policy file, with the variable unset, serves the client
/// standalone; so does an unknown value of the variable, which says why.
#[tokio::test]
async fn a_server_with_shared_mode_off_serves_its_one_client_itself() {
    let mut fixture = Fixture::new("standalone");
    fixture.set("NIRI_COMPUTER_USE_SHARED", "yes");
    let _niri = Niri::start(&fixture);
    let mut typo = Server::start(&fixture).await;
    let status = typo.structured("status").await;
    assert_eq!(status["shared"]["setting"], "off", "{status}");
    let error = status["shared"]["error"].as_str().unwrap();
    assert!(error.contains("\"yes\""), "{error}");
    let refused = status["engine"].clone();
    assert_eq!(refused["mode"], "standalone", "{refused}");
    let fallback = refused["fallback"].as_str().unwrap();
    assert!(
        fallback.contains("NIRI_COMPUTER_USE_SHARED is \"yes\""),
        "{fallback}"
    );
    fixture.unset("NIRI_COMPUTER_USE_SHARED");
    std::fs::create_dir_all(fixture.path("config/niri-computer-use")).unwrap();
    std::fs::write(
        fixture.path("config/niri-computer-use/policy.toml"),
        "shared = false\n",
    )
    .unwrap();
    let mut server = Server::start(&fixture).await;
    let engine = server.structured("status").await["engine"].clone();
    assert_eq!(engine["pid"], server.pid, "{engine}");
    assert_eq!(engine["mode"], "standalone", "{engine}");
    assert_eq!(engine["sessions"], 1, "{engine}");
    assert!(engine["fallback"].is_null(), "{engine}");
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
        let status = server.structured("status").await["engine"].clone();
        assert_eq!(status["pid"], engine, "{status}");
        assert_eq!(status["mode"], "shared", "{status}");
        assert_eq!(status["sessions"], 10, "{status}");
        assert!(status["fallback"].is_null(), "{status}");
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
    // The engine takes the environment of the client that starts it, so each kind of
    // client starts one.
    unrestricted_per_client(true).await;
    unrestricted_per_client(false).await;
}

/// A client with `NIRI_COMPUTER_USE_UNRESTRICTED` as `first` starts the engine, then one
/// with the opposite joins; each must see its own setting in `status` and the tool list.
async fn unrestricted_per_client(first: bool) {
    let mut fixture = shared("shared-unrestricted");
    let _niri = Niri::start(&fixture);
    fixture.program("noctalia", "exit 0");
    let mut servers = Vec::new();
    for unrestricted in [first, !first] {
        fixture.unset("NIRI_COMPUTER_USE_UNRESTRICTED");
        if unrestricted {
            fixture.set("NIRI_COMPUTER_USE_UNRESTRICTED", "1");
        }
        servers.push((Server::start(&fixture).await, unrestricted));
    }
    engine(&fixture).await;
    for (server, unrestricted) in &mut servers {
        let status = server.structured("status").await;
        assert_eq!(status["unrestricted"]["enabled"], *unrestricted);
        let tools = server.tools().await;
        let noctalia = tools.iter().any(|tool| tool["name"] == "noctalia");
        assert_eq!(noctalia, *unrestricted, "first unrestricted: {first}");
    }
}

/// The engine's `loginctl` and `noctalia` come from the desktop's directories, not from
/// the `PATH` of the client that starts it. A client that names the same directories
/// spelled another way joins it, and one that names other directories is served standalone with what it found, leaving the engine's clients theirs.
#[tokio::test]
async fn the_first_clients_path_decides_nothing_for_the_desktop() {
    let mut fixture = shared("shared-desktop-path");
    fixture.program("noctalia", "exit 0");
    fixture.program("loginctl", "echo no");
    noctalia::start(&fixture, UNLOCKED);
    let _niri = NiriProcess::start(&fixture, Some("c4")).await;
    std::fs::create_dir(fixture.path("empty")).unwrap();
    fixture.set("PATH", fixture.path("empty"));
    let mut narrow = Server::start(&fixture).await;
    let engine = engine(&fixture).await;
    fixture.set("PATH", fixture.path("bin"));
    let mut joined = Server::start(&fixture).await;
    let mut respelled = format!("{}/:relative:", fixture.path("bin").display());
    respelled.insert(0, '/');
    fixture.set("NIRI_COMPUTER_USE_DESKTOP_PATH", respelled);
    let mut same_dirs = Server::start(&fixture).await;
    fixture.set("NIRI_COMPUTER_USE_DESKTOP_PATH", fixture.path("empty"));
    let mut elsewhere = Server::start(&fixture).await;
    assert_eq!(engines(&fixture), [engine]);
    for server in [&mut narrow, &mut joined, &mut same_dirs] {
        assert_eq!(i32::try_from(server.serving_pid().await).unwrap(), engine);
        let status = server.structured("status").await;
        assert_eq!(status["noctalia"], "running", "{status}");
        assert_eq!(status["lock"]["source"], "logind", "{status}");
    }
    let status = elsewhere.structured("status").await;
    assert_eq!(status["noctalia"], "not_installed", "{status}");
    let logind = status["lock"]["logind_error"].as_str().unwrap();
    assert!(logind.starts_with("no loginctl in "), "{status}");
    let (_, _, stderr) = elsewhere.stop().await;
    assert!(
        stderr.contains(
            "refused this client (session_mismatch): the bridge looks for loginctl and noctalia in"
        ),
        "{stderr}"
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

/// More requests in flight than the bridge's queue of 32 holds, which only calls are
/// limited by: when the engine is lost, a client that reads hears `engine_lost` for each,
/// keeps its bridge, and its next call reaches a new engine.
#[tokio::test]
async fn a_lost_engine_answers_every_request_in_flight_to_a_client_that_reads() {
    let fixture = shared("shared-lost-many");
    let _niri = Niri::start(&fixture);
    let mut server = Server::start(&fixture).await;
    let first = server.serving_pid().await;
    signal(first, rustix::process::Signal::STOP);
    let mut ids = Vec::new();
    for _ in 0..40 {
        ids.push(server.request("tools/list", json!({})).await);
    }
    // The bridge relays them to the stopped engine's socket meanwhile.
    tokio::time::sleep(Duration::from_millis(500)).await;
    kill(first);
    for id in ids {
        let response = server.response(id).await;
        let message = response["error"]["message"].as_str().unwrap_or_default();
        assert!(message.starts_with("engine_lost: "), "{response}");
    }
    assert_ne!(server.serving_pid().await, first);
}

#[tokio::test]
async fn after_the_runtime_directory_goes_the_next_call_reaches_a_new_engine() {
    let fixture = shared("shared-gone");
    let _niri = Niri::start(&fixture);
    let mut server = Server::start(&fixture).await;
    let first = server.serving_pid().await;
    std::fs::remove_dir_all(fixture.runtime_dir()).unwrap();
    assert!(eventually(WAIT, || exited(i32::try_from(first).unwrap())).await);
    let (lost, _) = tool_error(&server.call("status", json!({})).await);
    assert_eq!(lost, "engine_lost");
    assert_ne!(server.serving_pid().await, first);
}

#[tokio::test]
async fn an_engine_ended_by_a_replaced_directory_leaves_the_new_engine_reachable() {
    let fixture = shared("shared-replaced");
    let _niri = Niri::start(&fixture);
    let mut first = Server::start(&fixture).await;
    let old = first.serving_pid().await;
    // Stopped, the old engine can only notice the new directory once the new engine
    // listens in it.
    signal(old, rustix::process::Signal::STOP);
    std::fs::remove_dir_all(fixture.runtime_dir()).unwrap();
    let mut second = Server::start(&fixture).await;
    let new = second.serving_pid().await;
    assert_ne!(new, old);
    signal(old, rustix::process::Signal::CONT);
    assert!(eventually(WAIT, || exited(i32::try_from(old).unwrap())).await);
    let mut third = Server::start(&fixture).await;
    let engine = third.structured("status").await["engine"].clone();
    assert_eq!(engine["mode"], "shared", "{engine}");
    assert_eq!(engine["pid"], new, "{engine}");
}

#[tokio::test]
async fn an_engine_that_stops_reading_counts_as_lost() {
    let fixture = shared("shared-wedged");
    let _niri = Niri::start(&fixture);
    let mut server = Server::start(&fixture).await;
    let engine = server.serving_pid().await;
    signal(engine, rustix::process::Signal::STOP);
    // More than the socket's buffers hold, so the bridge's write waits for a read.
    let call = server
        .start_call("screenshot", json!({"target": "x".repeat(4 << 20)}))
        .await;
    let (lost, detail) = tool_error(&server.response(call).await["result"]);
    assert_eq!(lost, "engine_lost");
    assert!(detail.contains("stopped reading"), "{detail}");
}

#[tokio::test]
async fn an_engine_that_exits_is_reaped_by_the_bridge_that_started_it() {
    let fixture = shared("shared-reaped");
    let _niri = Niri::start(&fixture);
    let mut server = Server::start(&fixture).await;
    let started = server.serving_pid().await;
    kill(started);
    let gone = std::path::PathBuf::from(format!("/proc/{started}"));
    assert!(
        eventually(WAIT, || !gone.exists()).await,
        "the exited engine is left a zombie"
    );
    assert!(!exited(i32::try_from(server.pid).unwrap()));
}

#[tokio::test]
async fn an_engine_refusing_the_hello_after_a_loss_leaves_calls_unavailable() {
    let fixture = shared("shared-refused");
    let _niri = Niri::start(&fixture);
    let mut server = Server::start(&fixture).await;
    let first = server.serving_pid().await;
    kill(first);
    assert!(eventually(WAIT, || exited(i32::try_from(first).unwrap())).await);
    let (lost, _) = tool_error(&server.call("status", json!({})).await);
    assert_eq!(lost, "engine_lost");
    let _refusing = refusing_engine(&fixture);
    for _ in 0..2 {
        let (error, detail) = tool_error(&server.call("status", json!({})).await);
        assert_eq!(error, "engine_unavailable");
        assert!(detail.contains("another build"), "{detail}");
    }
}

/// An engine of 0.1.1 refuses a client of this build without naming its release or what
/// to do. The client is served standalone, and its fallback names the engine's PID and
/// says what to do from the engine's file: here, the test's binary, still installed
/// beside the client's, so restarting either side's clients runs the same builds again.
/// The nested upgrade check covers an engine whose file an upgrade replaced.
#[tokio::test]
async fn a_client_beside_an_installed_engine_of_0_1_1_is_told_to_run_one_build() {
    let fixture = shared("shared-older");
    let _niri = Niri::start(&fixture);
    std::fs::create_dir_all(fixture.runtime_dir()).unwrap();
    let _older = refusing_engine(&fixture);
    let mut server = Server::start(&fixture).await;
    let engine = server.structured("status").await["engine"].clone();
    assert_eq!(engine["mode"], "standalone", "{engine}");
    let fallback = engine["fallback"].as_str().unwrap();
    assert!(
        fallback.contains("(engine_version): another build")
            && fallback.contains(&format!(
                "that engine, PID {}, runs niri-computer-use 0.1.1 or older from {}",
                std::process::id(),
                std::env::current_exe().unwrap().display()
            ))
            && fallback.contains("both builds are still installed: point this MCP client at")
            && !fallback.contains("restart"),
        "{fallback}"
    );
}

/// Answers every hello on the engine's socket with an `engine_version` refusal, as an
/// engine of 0.1.1 sends it: with no release. A socket a killed engine left is replaced.
fn refusing_engine(fixture: &Fixture) -> tokio::task::JoinHandle<()> {
    let path = fixture.runtime_dir().join("engine.sock");
    std::fs::create_dir_all(fixture.runtime_dir()).unwrap();
    std::fs::remove_file(&path).ok();
    let listener = tokio::net::UnixListener::bind(path).unwrap();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let (input, mut output) = stream.into_split();
            BufReader::new(input).lines().next_line().await.ok();
            let refusal = br#"{"refused":{"error":"engine_version","detail":"another build"}}"#;
            output
                .write_all(&[refusal.as_slice(), b"\n"].concat())
                .await
                .ok();
        }
    })
}

/// Writes the policy file of every client of `fixture`.
fn policy(fixture: &Fixture, text: &str) {
    std::fs::create_dir_all(fixture.path("config/niri-computer-use")).unwrap();
    std::fs::write(fixture.path("config/niri-computer-use/policy.toml"), text).unwrap();
}

/// Calls each tool and requires `engine_unavailable`, with `reason` in the detail.
async fn every_call_unavailable(server: &mut Server, reason: &str) {
    let tools = server.tools().await;
    let names: Vec<&str> = tools
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect();
    for tool in ["status", "desktop_state", "acquire_desktop", "screenshot"] {
        assert!(names.contains(&tool), "{names:?}");
        let (error, detail) = tool_error(&server.call(tool, json!({})).await);
        assert_eq!(error, "engine_unavailable", "{tool}");
        assert!(detail.contains(reason), "{tool}: {detail}");
    }
    assert_eq!(guardians(i32::try_from(server.pid).unwrap()), 0);
}

/// A client that requires the shared engine, with none answering in time, is never served
/// standalone: its tools are listed, and every call fails with `engine_unavailable` and the
/// bridge's reason.
#[tokio::test]
async fn without_an_engine_in_time_a_client_that_requires_one_gets_no_standalone_server() {
    let mut fixture = shared("required-cold");
    fixture.set("NIRI_COMPUTER_USE_SHARED", "required");
    let _niri = Niri::start(&fixture);
    let _lock = hold_engine_lock(&fixture);
    let mut server = Server::start(&fixture).await;
    let socket = fixture.runtime_dir().join("engine.sock");
    let reason = format!(
        "no shared engine answered within 5 s: connect to {}",
        socket.display()
    );
    every_call_unavailable(&mut server, &reason).await;
    let (_, _, stderr) = server.stop().await;
    assert!(
        stderr.contains("the shared engine is required, so every tool fails with engine_unavailable: no shared engine answered"),
        "{stderr}"
    );
}

/// An engine that refuses a client that requires it, as the policy file says, counts as
/// unreachable.
#[tokio::test]
async fn an_engine_refusing_a_client_that_requires_it_leaves_every_call_unavailable() {
    let fixture = shared("required-refused");
    policy(&fixture, "shared = \"required\"\n");
    let _niri = Niri::start(&fixture);
    let _refusing = refusing_engine(&fixture);
    let mut server = Server::start(&fixture).await;
    every_call_unavailable(
        &mut server,
        "the shared engine refused this client (engine_version): another build",
    )
    .await;
}

/// A mistake elsewhere in the policy file doesn't drop its requirement: an engine that
/// refuses the client still leaves every call unavailable.
#[tokio::test]
async fn an_invalid_policy_file_still_requires_the_engine() {
    let fixture = shared("required-invalid");
    policy(&fixture, "shared = \"required\"\nnot_a_key = 1\n");
    let _niri = Niri::start(&fixture);
    let _refusing = refusing_engine(&fixture);
    let mut server = Server::start(&fixture).await;
    every_call_unavailable(&mut server, "the shared engine refused this client").await;
}

/// The engine reports the setting the bridge decided from the whole policy file, for a
/// file over the 64 KiB the hello carries and for an invalid one.
#[tokio::test]
async fn the_engine_reports_the_requirement_of_a_file_it_cant_read() {
    let fixture = shared("required-reported");
    let _niri = Niri::start(&fixture);
    let comment = format!("# {}\n", "x".repeat(70 * 1024));
    for text in [
        format!("shared = \"required\"\n{comment}"),
        "shared = \"required\"\nnot_a_key = 1\n".to_owned(),
    ] {
        policy(&fixture, &text);
        let mut server = Server::start(&fixture).await;
        let status = server.structured("status").await;
        assert_eq!(status["engine"]["mode"], "shared", "{status}");
        assert_eq!(status["policy"]["state"], "invalid", "{status}");
        assert_eq!(status["shared"]["setting"], "required", "{status}");
    }
}

/// A line over the limit ends a session that requires the engine and has none, as it ends
/// every other session, rather than being read whole.
#[tokio::test]
async fn a_line_over_the_limit_ends_a_session_without_the_engine() {
    let mut fixture = shared("required-line");
    fixture.set("NIRI_COMPUTER_USE_SHARED", "required");
    let _niri = Niri::start(&fixture);
    let _refusing = refusing_engine(&fixture);
    let mut server = Server::start(&fixture).await;
    server.send_raw(&framing::oversized_between(100)).await;
    assert!(server.exit_within(WAIT).await.is_some());
    let (_, unread, _) = server.stop().await;
    assert!(framing::answers(&unread, &[100]), "{unread:?}");
    assert!(!framing::answers(&unread, &[101, 102]), "{unread:?}");
}

/// A wrong variable doesn't drop the file's requirement, and a client that requires the
/// engine is served by it when it answers.
#[tokio::test]
async fn a_wrong_variable_keeps_the_files_requirement() {
    let mut fixture = shared("required-typo");
    policy(&fixture, "shared = \"required\"\n");
    let _niri = Niri::start(&fixture);
    fixture.set("NIRI_COMPUTER_USE_SHARED", "yes");
    let mut typo = Server::start(&fixture).await;
    every_call_unavailable(&mut typo, "NIRI_COMPUTER_USE_SHARED is \"yes\"").await;
    assert_eq!(engines(&fixture), Vec::<i32>::new());
    fixture.unset("NIRI_COMPUTER_USE_SHARED");
    let mut server = Server::start(&fixture).await;
    let status = server.structured("status").await;
    assert_eq!(
        status["shared"],
        json!({"setting": "required", "error": null})
    );
    assert_eq!(status["engine"]["mode"], "shared", "{status}");
    assert_eq!(status["engine"]["pid"], engine(&fixture).await, "{status}");
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

/// The engine's open file descriptors once the count has held for half a second: a client
/// that just left can keep a few open a moment longer, as its clipboard watcher does until
/// its next 250 ms tick, and counting those would make every later count look short.
async fn settled_fds(pid: i32) -> usize {
    let start = Instant::now();
    let mut last = fds(pid);
    let mut since = Instant::now();
    while since.elapsed() < Duration::from_millis(500) && start.elapsed() < WAIT {
        tokio::time::sleep(Duration::from_millis(20)).await;
        let now = fds(pid);
        if now != last {
            last = now;
            since = Instant::now();
        }
    }
    last
}

/// How many sessions the engine serves, once its count has stopped changing.
pub(crate) async fn sessions(server: &mut Server) -> u64 {
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
    let before = settled_fds(engine).await;
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
    fixture.set("NIRI_COMPUTER_USE_SHARED", "0");
    let mut standalone = Server::start(&fixture).await;
    let _own_stream = desktop.stream().await;
    holds_alone(&mut bridged, &mut standalone).await;
    holds_alone(&mut standalone, &mut bridged).await;
}

/// A client whose runtime directory and niri socket are other spellings of the same ones,
/// through a link to the directory and a link to the socket whose name says nothing,
/// reaches the same lease and stop flag, whether it is served standalone or not.
#[tokio::test]
async fn servers_reaching_one_niri_through_other_paths_share_its_lease_and_stop_flag() {
    let mut fixture = shared("shared-alias");
    let mut desktop = Desktop::new(&fixture);
    let mut bridged = Server::start(&fixture).await;
    let _engine_stream = desktop.stream().await;
    let alias = fixture.path("alias");
    std::os::unix::fs::symlink(fixture.path("run"), &alias).unwrap();
    std::os::unix::fs::symlink(alias.join("niri.test.sock"), fixture.path("run/niri")).unwrap();
    let runtime = fixture.env()["XDG_RUNTIME_DIR"].clone();
    fixture.set("NIRI_COMPUTER_USE_SHARED", "0");
    fixture.set("XDG_RUNTIME_DIR", &alias);
    fixture.set("NIRI_SOCKET", alias.join("niri"));
    let mut standalone = Server::start(&fixture).await;
    let _own_stream = desktop.stream().await;
    let status = standalone.structured("status").await;
    assert_eq!(status["engine"]["mode"], "standalone", "{status}");
    holds_alone(&mut bridged, &mut standalone).await;
    holds_alone(&mut standalone, &mut bridged).await;
    assert!(crate::client::run(&fixture, "stop").await.status.success());
    assert_eq!(bridged.structured("status").await["stop"], true);
    // The engine, started with the fixture's own variables, is found by them at the end.
    fixture.set("XDG_RUNTIME_DIR", runtime);
}

/// Sol's case: a hard link to niri's socket is another name for the same niri, which would
/// have a runtime directory, and so a lease and stop flag, of its own. While the link
/// exists, a server started by either name takes no lease, and `stop` has no flag to set;
/// once it is gone, the socket's own name works again.
#[tokio::test]
async fn a_niri_socket_with_a_hard_link_takes_no_lease_by_either_name() {
    let mut fixture = shared("shared-hard-link");
    let mut desktop = Desktop::new(&fixture);
    let alias = fixture.path("run/alias.sock");
    std::fs::hard_link(fixture.niri_socket(), &alias).unwrap();
    let mut original = Server::start(&fixture).await;
    let _original_stream = desktop.stream().await;
    fixture.set("NIRI_COMPUTER_USE_SHARED", "0");
    fixture.set("NIRI_SOCKET", &alias);
    let mut linked = Server::start(&fixture).await;
    let _linked_stream = desktop.stream().await;
    for server in [&mut original, &mut linked] {
        let (name, detail) = tool_error(&server.call("acquire_desktop", json!({})).await);
        assert_eq!(name, "niri_unavailable");
        assert!(detail.contains("which has 2 hard links"), "{detail}");
        assert_eq!(server.structured("status").await["lease"]["error"], detail);
    }
    assert!(!crate::client::run(&fixture, "stop").await.status.success());
    assert!(!fixture.runtime_dir().exists());

    std::fs::remove_file(&alias).unwrap();
    fixture.set("NIRI_SOCKET", fixture.niri_socket());
    let mut again = Server::start(&fixture).await;
    let _again_stream = desktop.stream().await;
    again.structured("acquire_desktop").await;
}

/// `holder` takes the lease, `other` is refused it, and `holder` gives it back.
pub(crate) async fn holds_alone(holder: &mut Server, other: &mut Server) {
    holder.structured("acquire_desktop").await;
    let (name, _) = tool_error(&other.call("acquire_desktop", json!({})).await);
    assert_eq!(name, "lease_held");
    let release = json!({"restore_focus": false});
    let released = holder.structured_with("release_desktop", release).await;
    assert_eq!(released["released"], true);
}

/// A marker a killed server left, whose crash guardian sent the releases: the engine
/// refuses every client the lease until `recover`.
#[tokio::test]
async fn a_marker_a_killed_server_left_sends_every_client_to_recover() {
    let fixture = shared("shared-killed-marker");
    let mut desktop = Desktop::new(&fixture);
    let mut first = Server::start(&fixture).await;
    let mut other = Server::start(&fixture).await;
    let _stream = desktop.stream().await;
    let marker = json!({
        "operation": "key", "phase": "pending", "server_pid": 1, "since": "t",
        "keyboard": {"codes": [108], "group": 0}, "released": "t"
    });
    std::fs::write(
        fixture.runtime_dir().join("input-dirty"),
        marker.to_string(),
    )
    .unwrap();
    for client in [&mut first, &mut other] {
        let (name, detail) = tool_error(&client.call("acquire_desktop", json!({})).await);
        assert_eq!(name, "recovery_required");
        assert!(
            detail.contains("call release_desktop, then the user runs `niri-computer-use recover`")
                && !detail.contains("try again"),
            "{detail}"
        );
    }
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

/// A client that has stopped reading: its pipe is full with `tools/list` answers, and more
/// wait in the bridge for it, fewer than its queue holds.
async fn stop_reading(client: &mut Server) {
    client.pause_reading();
    for _ in 0..10 {
        client.request("tools/list", json!({})).await;
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
}

/// Whether `other` takes the lease within `limit`.
async fn takes_the_lease(other: &mut Server, limit: Duration) -> bool {
    let until = Instant::now() + limit;
    while Instant::now() < until {
        if other.call("acquire_desktop", json!({})).await["isError"] == false {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    false
}

/// A client that stopped reading and then closed stdin gives the lease up at once, however
/// much of what it was sent is still unwritten.
#[tokio::test]
async fn a_client_that_stops_reading_and_leaves_frees_the_lease_within_a_second() {
    let fixture = shared("shared-deaf-end");
    let mut desktop = Desktop::new(&fixture);
    let mut deaf = Server::start(&fixture).await;
    let _stream = desktop.stream().await;
    deaf.structured("acquire_desktop").await;
    let mut other = Server::start(&fixture).await;
    stop_reading(&mut deaf).await;
    let (held, _) = tool_error(&other.call("acquire_desktop", json!({})).await);
    assert_eq!(held, "lease_held");
    deaf.close_stdin();
    assert!(takes_the_lease(&mut other, Duration::from_secs(1)).await);
}

/// A client that sends without reading fills the bridge's queue, which ends the bridge at
/// once rather than when a line's deadline passes.
#[tokio::test]
async fn a_client_that_never_reads_ends_its_bridge_once_the_queue_is_full() {
    let fixture = shared("shared-deaf-full");
    let mut desktop = Desktop::new(&fixture);
    let mut deaf = Server::start(&fixture).await;
    let _stream = desktop.stream().await;
    deaf.structured("acquire_desktop").await;
    let mut other = Server::start(&fixture).await;
    deaf.pause_reading();
    for _ in 0..100 {
        deaf.request("tools/list", json!({})).await;
    }
    let (status, stderr) = deaf.exit_within(Duration::from_secs(5)).await.unwrap();
    assert!(!status.success(), "{status}");
    assert_eq!(
        stderr,
        "niri-computer-use: the client left 32 lines unread; its session ends\n"
    );
    assert!(takes_the_lease(&mut other, Duration::from_secs(1)).await);
}

/// A cancellation still reaches the engine while the client's stdout is full.
#[tokio::test]
async fn a_cancellation_goes_through_while_the_client_isnt_reading() {
    let fixture = shared("shared-deaf-cancel");
    fixture.program("grim", crate::cancellation::STUCK);
    let mut desktop = Desktop::new(&fixture);
    let mut deaf = Server::start(&fixture).await;
    let _stream = desktop.stream().await;
    stop_reading(&mut deaf).await;
    let id = deaf
        .start_call("screenshot", json!({"target": "focused_output"}))
        .await;
    crate::fixture::pid_in(&fixture.path("child.pid")).await;
    deaf.cancel(id).await;
    assert!(crate::cancellation::stuck_program_exited(&fixture).await);
}

/// The engine lost while the client isn't reading: its call in flight is answered with
/// `engine_lost` after what was queued before, once the client reads again.
#[tokio::test]
async fn an_engine_lost_while_the_client_isnt_reading_is_reported_in_order() {
    let fixture = shared("shared-deaf-lost");
    fixture.program("grim", crate::cancellation::STUCK);
    let mut desktop = Desktop::new(&fixture);
    let mut deaf = Server::start(&fixture).await;
    let _stream = desktop.stream().await;
    let engine = engine(&fixture).await;
    stop_reading(&mut deaf).await;
    let id = deaf
        .start_call("screenshot", json!({"target": "focused_output"}))
        .await;
    crate::fixture::pid_in(&fixture.path("child.pid")).await;
    kill(u32::try_from(engine).unwrap());
    deaf.read_again();
    let (name, _) = tool_error(&deaf.response(id).await["result"]);
    assert_eq!(name, "engine_lost");
    // `initialize`, the ten `tools/list`, then the screenshot.
    assert_eq!(deaf.answered(), (1..=id).collect::<Vec<_>>());
}

/// A client that keeps stdin open but reads nothing more ends its bridge 30 s into the
/// line it doesn't take, though the write to it can't be cut short: the process exits.
#[tokio::test]
async fn a_bridge_whose_client_reads_nothing_exits_at_the_write_deadline() {
    let fixture = shared("shared-deaf-deadline");
    let mut desktop = Desktop::new(&fixture);
    let mut deaf = Server::start(&fixture).await;
    let _stream = desktop.stream().await;
    deaf.structured("acquire_desktop").await;
    let mut other = Server::start(&fixture).await;
    let stopped = Instant::now();
    stop_reading(&mut deaf).await;
    let (status, stderr) = deaf.exit_within(Duration::from_secs(40)).await.unwrap();
    assert!(
        stopped.elapsed() >= Duration::from_secs(30),
        "{:?}",
        stopped.elapsed()
    );
    assert!(!status.success(), "{status}");
    assert_eq!(
        stderr,
        "niri-computer-use: the client didn't read a line within 30 s; its session ends\n"
    );
    assert!(takes_the_lease(&mut other, Duration::from_secs(1)).await);
}

/// A `tools/call` line of about 15 MiB with id `id`.
fn padded_call(id: u64) -> Vec<u8> {
    let call = json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "tools/call",
        "params": {"name": "screenshot", "arguments": {"target": "x".repeat(15 << 20)}},
    });
    let mut line = serde_json::to_vec(&call).unwrap();
    line.push(b'\n');
    line
}

/// A client whose lines the engine doesn't take, more than the bridge's backlog holds with
/// more input behind them, ends its bridge at once, closed stdin or not, and the engine
/// ends its session once it reads again.
#[tokio::test]
async fn a_client_whose_lines_overflow_the_backlog_ends_its_bridge_at_once() {
    let fixture = shared("shared-backlog");
    let mut desktop = Desktop::new(&fixture);
    // The engine's parent is the bridge that started it. Were it the one that exits, the
    // stopped engine's process group would be orphaned, and the kernel would hang it up.
    let mut other = Server::start(&fixture).await;
    let _stream = desktop.stream().await;
    let mut flood = Server::start(&fixture).await;
    flood.structured("acquire_desktop").await;
    let engine = flood.serving_pid().await;
    signal(engine, rustix::process::Signal::STOP);
    for id in 100..103 {
        flood.send_raw(&padded_call(id)).await;
    }
    flood
        .send_raw(b"{\"jsonrpc\":\"2.0\",\"id\":103,\"method\":\"tools/list\"}\n")
        .await;
    flood.close_stdin();
    let exited = flood.exit_within(Duration::from_secs(2)).await;
    signal(engine, rustix::process::Signal::CONT);
    let (status, stderr) = exited.expect("the bridge waited for the engine");
    assert!(!status.success(), "{status}");
    assert_eq!(
        stderr,
        "niri-computer-use: the client sent more than 33554432 bytes the shared engine hadn't taken yet; its session ends\n"
    );
    assert!(takes_the_lease(&mut other, Duration::from_secs(2)).await);
}
