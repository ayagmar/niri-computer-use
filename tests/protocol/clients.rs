//! Many clients on one engine: several bridges at once, each with a session of its own.
//! What one client does, or what happens to it or to the engine, must leave the other
//! sessions as the rules say: one lease owner, a stop for everyone, a client's end freeing
//! only what it held, no setting crossing sessions, and an audit line per call naming the
//! session that made it.

use std::ffi::OsString;
use std::time::{Duration, Instant};

use serde_json::json;

use crate::client::{CLIENT, Server, WAIT, run, tool_error};
use crate::fixture::{Fixture, eventually, exited, file_at, jpeg, kill, parent, png};
use crate::shared::{
    Desktop, engine, engines, guardians, hold_engine_lock, holds_alone, sessions, shared,
};

/// The audit log's name for `client`'s session.
fn label(client: &Server) -> String {
    format!("{CLIENT}/{}", client.pid)
}

/// Starts `count` bridges at once, then completes each one's handshake.
async fn bridges(fixture: &Fixture, count: usize) -> Vec<Server> {
    let mut clients: Vec<Server> = (0..count).map(|_| Server::spawn(fixture)).collect();
    for client in &mut clients {
        let response = client.initialize("2025-11-25").await;
        assert!(response.get("result").is_some(), "{response}");
    }
    clients
}

/// Whether `observer` sees the lease free within `limit`.
async fn lease_free_within(observer: &mut Server, limit: Duration) -> bool {
    let until = Instant::now() + limit;
    loop {
        if observer.structured("status").await["lease"]["holder"].is_null() {
            return true;
        }
        if Instant::now() >= until {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Every client asks for the lease at once. Exactly one gets it, and every other is
/// refused with `lease_held` naming it. Returns the owner's index.
async fn race_for_the_lease(clients: &mut [Server]) -> usize {
    let mut asked = Vec::new();
    for client in clients.iter_mut() {
        asked.push(client.start_call("acquire_desktop", json!({})).await);
    }
    let mut results = Vec::new();
    for (client, id) in clients.iter_mut().zip(asked) {
        results.push(client.response(id).await["result"].clone());
    }
    let owners: Vec<usize> = (0..results.len())
        .filter(|index| results[*index]["isError"] == false)
        .collect();
    assert_eq!(owners.len(), 1, "{results:#?}");
    let owner = owners[0];
    let held_by = format!("({})", label(&clients[owner]));
    for (index, result) in results.iter().enumerate() {
        if index != owner {
            let (name, detail) = tool_error(result);
            assert_eq!(name, "lease_held", "{detail}");
            assert!(detail.contains(&held_by), "{detail}");
        }
    }
    owner
}

/// Six clients ask for the lease at once, three times. Each time exactly one gets it, and
/// once that owner is killed between calls, the lease is free for the next race within a
/// second, while the engine keeps serving the rest.
#[tokio::test]
async fn clients_racing_for_the_lease_leave_exactly_one_owner() {
    let fixture = shared("many-race");
    let mut desktop = Desktop::new(&fixture);
    let mut clients = bridges(&fixture, 6).await;
    let _stream = desktop.stream().await;
    let engine = engine(&fixture).await;
    for _ in 0..3 {
        let owner = race_for_the_lease(&mut clients).await;
        let mut killed = clients.remove(owner);
        killed.kill().await;
        assert!(
            lease_free_within(&mut clients[0], Duration::from_secs(1)).await,
            "a killed owner's lease was still held a second later"
        );
    }
    assert_eq!(sessions(&mut clients[0]).await, 3);
    assert_eq!(engines(&fixture), [engine]);
}

/// A stop while one client's key is running: that call is stopped within a second, the
/// other clients' calls in flight are answered, and every client is refused the lease
/// until `resume`, after which another client takes it.
#[tokio::test]
async fn a_stop_during_one_clients_action_holds_every_client_back_until_resume() {
    let fixture = shared("many-stop");
    let mut desktop = Desktop::new(&fixture);
    fixture.program("wtype", r#"echo >> "$DIR/wtype.calls"; await_file go"#);
    let mut clients = bridges(&fixture, 5).await;
    let _stream = desktop.stream().await;
    let (owner, others) = clients.split_first_mut().unwrap();
    owner.structured("acquire_desktop").await;
    let key = owner
        .start_call("key", json!({"keys": ["Down"], "expect": "none"}))
        .await;
    assert!(eventually(WAIT, || fixture.path("wtype.calls").exists()).await);
    let mut calls = Vec::new();
    for other in others.iter_mut() {
        calls.push(other.start_call("desktop_state", json!({})).await);
    }
    let stopped = Instant::now();
    assert!(run(&fixture, "stop").await.status.success());
    // wtype had started on the key, so it may have gone out: the outcome says so.
    let cut = owner.response(key).await["result"]["structuredContent"].clone();
    assert_eq!(cut["observed"], "uncertain", "{cut}");
    assert!(
        cut["detail"].as_str().unwrap().contains("stop flag"),
        "{cut}"
    );
    assert!(
        stopped.elapsed() < Duration::from_secs(1),
        "{:?}",
        stopped.elapsed()
    );
    for (other, id) in others.iter_mut().zip(calls) {
        assert_eq!(other.response(id).await["result"]["isError"], false);
    }
    for client in std::iter::once(&mut *owner).chain(others.iter_mut()) {
        let status = client.structured("status").await;
        assert_eq!(status["stop"], true, "{status}");
        assert!(status["lease"]["holder"].is_null(), "{status}");
        let (refused, _) = tool_error(&client.call("acquire_desktop", json!({})).await);
        assert_eq!(refused, "stopped");
    }
    // The released wtype finishes the stopped key, which takes the marker off.
    std::fs::write(fixture.path("go"), "").unwrap();
    let dirty = fixture.runtime_dir().join("input-dirty");
    assert!(eventually(WAIT, || !dirty.exists()).await);
    assert!(run(&fixture, "resume").await.status.success());
    others[0].structured("acquire_desktop").await;
}

/// A client killed mid-key, the one whose bridge started the engine: the lease is free
/// within a second while three other clients keep calling, its key's input cleanup holds
/// every client back with `recovery_required` until the key's wtype ends, and then another
/// client takes the lease.
#[tokio::test]
async fn a_client_killed_mid_key_frees_the_lease_while_others_keep_working() {
    let fixture = shared("many-killed");
    let mut desktop = Desktop::new(&fixture);
    fixture.program("wtype", r#"echo >> "$DIR/wtype.calls"; await_file go"#);
    let mut clients = bridges(&fixture, 4).await;
    let _stream = desktop.stream().await;
    let started_by = parent(u32::try_from(engine(&fixture).await).unwrap());
    let starter = clients.iter().position(|client| client.pid == started_by);
    let mut typing = clients.remove(starter.unwrap());
    typing.structured("acquire_desktop").await;
    typing
        .start_call("key", json!({"keys": ["Down"], "expect": "none"}))
        .await;
    assert!(eventually(WAIT, || fixture.path("wtype.calls").exists()).await);
    typing.kill().await;
    let killed = Instant::now();
    let mut free = false;
    while !free && killed.elapsed() < Duration::from_secs(1) {
        for other in &mut clients {
            let state = other.call("desktop_state", json!({})).await;
            assert_eq!(state["isError"], false, "{state}");
        }
        free = clients[0].structured("status").await["lease"]["holder"].is_null();
    }
    assert!(
        free,
        "the lease was still held {:?} after",
        killed.elapsed()
    );
    let marker = fixture.runtime_dir().join("input-dirty");
    assert!(marker.exists());
    // The engine still runs that key's wtype, which takes the marker off once it ends.
    for other in &mut clients {
        let (name, detail) = tool_error(&other.call("acquire_desktop", json!({})).await);
        assert_eq!(name, "recovery_required");
        assert!(detail.contains("still finishing"), "{detail}");
    }
    std::fs::write(fixture.path("go"), "").unwrap();
    assert!(eventually(WAIT, || !marker.exists()).await);
    let owner = race_for_the_lease(&mut clients).await;
    assert_eq!(
        clients[owner].structured("status").await["lease"]["held_by_me"],
        true
    );
}

/// The engine killed with five clients connected, one of them mid-screenshot: that call
/// gets `engine_lost` at once, each idle client gets `engine_lost` once at its next call,
/// `engine_unavailable` while no engine can start, and then all five reach one new engine,
/// which serves each of them a session of its own.
#[tokio::test]
async fn a_lost_engine_fails_what_was_in_flight_and_every_client_reaches_one_new_engine() {
    let fixture = shared("many-lost");
    let _niri = crate::niri::Niri::start(&fixture);
    std::fs::write(fixture.path("grim.out"), jpeg(1280, 720, b"pixels")).unwrap();
    fixture.program(
        "grim",
        r#"echo >> "$DIR/grim.started"; await_file go; cat "$DIR/grim.out""#,
    );
    let mut clients = bridges(&fixture, 5).await;
    let first = engine(&fixture).await;
    let (busy, idle) = clients.split_first_mut().unwrap();
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
    let lock = hold_engine_lock(&fixture);
    let (next, _) = tool_error(&idle[0].call("status", json!({})).await);
    assert_eq!(next, "engine_lost");
    let (unavailable, why) = tool_error(&idle[0].call("status", json!({})).await);
    assert_eq!(unavailable, "engine_unavailable");
    assert!(why.contains("the next call tries again"), "{why}");
    drop(lock);
    for client in &mut idle[1..] {
        let (lost_too, _) = tool_error(&client.call("status", json!({})).await);
        assert_eq!(lost_too, "engine_lost");
    }
    let mut reached = Vec::new();
    for client in &mut clients {
        // Each bridge replays its client's `initialize` to the new engine first.
        let engine = client.structured("status").await["engine"].clone();
        reached.push(engine["pid"].as_i64().unwrap());
    }
    let second = engine(&fixture).await;
    assert_ne!(second, first);
    assert_eq!(reached, vec![i64::from(second); 5]);
    assert_eq!(sessions(&mut clients[0]).await, 5);
    assert_eq!(guardians(second), 1);
}

/// Four clients at a time arrive around the engine's two-second idle exit: before it, in
/// its last checks, while the engine is going and after it has gone. Every client is
/// served by one shared engine, none standalone, and once the last has left the engine
/// exits and removes its socket.
#[tokio::test]
async fn clients_arriving_around_the_idle_exit_are_served_by_one_shared_engine() {
    let fixture = shared("many-churn");
    let _niri = crate::niri::Niri::start(&fixture);
    let mut served_by = Vec::new();
    for pause in (1800..=2400).step_by(75).map(Duration::from_millis) {
        let pid = served_once(&fixture).await;
        served_by.push(pid);
        tokio::time::sleep(pause).await;
    }
    // Once the engine is surely gone, the next clients start another.
    let last = *served_by.last().unwrap();
    assert!(eventually(WAIT, || exited(last)).await);
    let after = served_once(&fixture).await;
    assert_ne!(after, last);
    assert!(
        served_by.windows(2).any(|pair| pair[0] == pair[1]),
        "clients 1.8 s after the last left reached another engine: {served_by:?}"
    );
    assert!(eventually(WAIT, || exited(after)).await);
    assert!(!fixture.runtime_dir().join("engine.sock").exists());
}

/// Starts four bridges at once, checks that one shared engine serves all four, stops them,
/// and returns the engine's PID.
async fn served_once(fixture: &Fixture) -> i32 {
    let mut clients = bridges(fixture, 4).await;
    let mut pids = Vec::new();
    for client in &mut clients {
        let engine = client.structured("status").await["engine"].clone();
        assert_eq!(engine["mode"], "shared", "{engine}");
        assert!(engine["fallback"].is_null(), "{engine}");
        pids.push(i32::try_from(engine["pid"].as_i64().unwrap()).unwrap());
    }
    pids.dedup();
    assert_eq!(pids.len(), 1, "{pids:?}");
    for client in clients {
        let (status, _, stderr) = client.stop().await;
        assert!(status.success(), "{stderr}");
    }
    pids[0]
}

/// A copy of the binary Cargo built: the same build as far as its code goes, but another
/// file, so another build to the hello.
fn copy_of_the_binary(fixture: &Fixture, name: &str) -> std::path::PathBuf {
    let copy = fixture.path(name);
    std::fs::copy(env!("CARGO_BIN_EXE_niri-computer-use"), &copy).unwrap();
    copy
}

/// `cargo install` replaces the binary's file while an engine runs from it. A client
/// started from the new file is served standalone, sharing the lease, and says why; the
/// old engine's clients keep working and their `status` says the file was replaced; once
/// they leave, the old engine exits, and the next client starts an engine of the new build.
#[tokio::test]
async fn after_the_binary_is_replaced_new_clients_wait_out_the_old_engine_standalone() {
    let fixture = shared("many-upgrade");
    let mut desktop = Desktop::new(&fixture);
    let installed = copy_of_the_binary(&fixture, "installed");
    let mut old = Vec::new();
    for _ in 0..3 {
        old.push(Server::start_from(&fixture, &installed).await);
    }
    let _engine_stream = desktop.stream().await;
    let old_engine = engine(&fixture).await;
    let before = old[0].structured("status").await;
    assert_eq!(before["engine"]["replaced"], false, "{before}");
    let update = copy_of_the_binary(&fixture, "update");
    std::fs::rename(&update, &installed).unwrap();
    let after = old[0].structured("status").await;
    assert_eq!(after["engine"]["replaced"], true, "{after}");
    let mut upgraded = Server::start_from(&fixture, &installed).await;
    let _own_stream = desktop.stream().await;
    let served = upgraded.structured("status").await["engine"].clone();
    assert_eq!(served["mode"], "standalone", "{served}");
    let fallback = served["fallback"].as_str().unwrap();
    assert!(fallback.contains("(engine_version)"), "{fallback}");
    assert!(
        fallback.contains(&format!("PID {old_engine}")),
        "{fallback}"
    );
    holds_alone(&mut upgraded, &mut old[0]).await;
    holds_alone(&mut old[1], &mut upgraded).await;
    for client in old {
        client.stop().await;
    }
    assert!(
        eventually(Duration::from_secs(4), || exited(old_engine)).await,
        "the old engine stayed after its last client left"
    );
    let mut next = Server::start_from(&fixture, &installed).await;
    let engine_status = next.structured("status").await["engine"].clone();
    assert_eq!(engine_status["mode"], "shared", "{engine_status}");
    assert_eq!(engine_status["replaced"], false, "{engine_status}");
    assert_ne!(engine_status["pid"], old_engine);
    assert_eq!(upgraded.call("status", json!({})).await["isError"], false);
}

/// What one client's environment gives its session, for `settings_stay_with_their_session`.
#[derive(Debug, Clone, Copy)]
struct Given {
    unrestricted: bool,
    keyboard: &'static str,
    tools: Option<&'static str>,
    policy: Option<&'static str>,
}

const PLAIN: Given = Given {
    unrestricted: false,
    keyboard: "wtype",
    tools: None,
    policy: None,
};
const SHOTS: Option<&str> = Some("capture_dir = \"~/shots\"\n");

/// Starts a bridge for a client whose environment gives `given`, with a home and a config
/// directory of its own named `name`.
async fn client_with(fixture: &mut Fixture, name: &str, given: Given) -> Server {
    let home = fixture.path(&format!("home-{name}"));
    let config = home.join(".config");
    std::fs::create_dir_all(config.join("niri-computer-use")).unwrap();
    if let Some(policy) = given.policy {
        std::fs::write(config.join("niri-computer-use/policy.toml"), policy).unwrap();
    }
    fixture.set("HOME", &home);
    fixture.set("XDG_CONFIG_HOME", &config);
    fixture.set("NIRI_COMPUTER_USE_KEYBOARD", given.keyboard);
    fixture.unset("NIRI_COMPUTER_USE_UNRESTRICTED");
    if given.unrestricted {
        fixture.set("NIRI_COMPUTER_USE_UNRESTRICTED", "1");
    }
    fixture.unset("NIRI_COMPUTER_USE_TOOLS");
    if let Some(tools) = given.tools {
        fixture.set("NIRI_COMPUTER_USE_TOOLS", tools);
    }
    Server::start(fixture).await
}

/// A grim that writes a full-size PNG for a save and a small JPEG to send.
fn two_sizes(fixture: &Fixture) {
    std::fs::write(fixture.path("full.png"), png(2560, 1440)).unwrap();
    std::fs::write(fixture.path("small.jpg"), jpeg(1280, 720, b"small")).unwrap();
    fixture.program(
        "grim",
        r#"case "$*" in
  "-t png -s 1 "*) cat "$DIR/full.png" ;;
  *) cat "$DIR/small.jpg" ;;
esac"#,
    );
}

/// Four clients, by name, the first of which starts the engine: unrestricted, with a
/// `capture_dir` and a keyboard backend that doesn't exist; a plain one; an `observe`
/// one with the same `capture_dir`; and one whose policy file is invalid.
async fn four_clients(fixture: &mut Fixture) -> Vec<(Server, &'static str)> {
    let clients = [
        (
            "first",
            Given {
                unrestricted: true,
                keyboard: "typo",
                policy: SHOTS,
                ..PLAIN
            },
        ),
        ("plain", PLAIN),
        (
            "observer",
            Given {
                tools: Some("observe"),
                policy: SHOTS,
                ..PLAIN
            },
        ),
        (
            "invalid",
            Given {
                policy: Some("bogus = 1\n"),
                ..PLAIN
            },
        ),
    ];
    let mut started = Vec::new();
    for (name, given) in clients {
        started.push((client_with(fixture, name, given).await, name));
    }
    started
}

/// Each client's `status` shows its own `unrestricted`, policy and tool profile, and its
/// tool list follows them.
async fn each_status_is_its_own(clients: &mut [(Server, &'static str)]) {
    let mut seen = Vec::new();
    for (client, _) in clients.iter_mut() {
        let status = client.structured("status").await;
        assert_eq!(status["engine"]["sessions"], 4, "{status}");
        seen.push([
            status["unrestricted"]["enabled"].clone(),
            status["policy"]["state"].clone(),
            status["tools"]["profile"].clone(),
        ]);
    }
    assert_eq!(
        seen,
        [
            [json!(true), json!("loaded"), json!("full")],
            [json!(false), json!("missing"), json!("full")],
            [json!(false), json!("loaded"), json!("observe")],
            [json!(false), json!("invalid"), json!("full")],
        ]
    );
    let mut listed = Vec::new();
    for (client, _) in clients.iter_mut() {
        let tools = client.tools().await;
        let named = |name: &str| tools.iter().any(|tool| tool["name"] == name);
        listed.push((named("noctalia"), named("acquire_desktop")));
    }
    assert_eq!(
        listed,
        [(true, true), (false, true), (false, false), (false, true)]
    );
}

/// The engine takes the environment of the client that starts it, so the most permissive
/// client starts it here. None of its settings reach the other sessions: each keeps its
/// own policy, home, keyboard backend and tool profile, and every call is audited under
/// the session that made it.
#[tokio::test]
async fn settings_stay_with_their_session() {
    let mut fixture = shared("many-settings");
    let mut desktop = Desktop::new(&fixture);
    two_sizes(&fixture);
    fixture.program("wtype", r#"cat > /dev/null; echo >> "$DIR/wtype.calls""#);
    let mut clients = four_clients(&mut fixture).await;
    let _stream = desktop.stream().await;
    each_status_is_its_own(&mut clients).await;
    // One `status` each so far. `~/shots` is under each session's own home.
    let mut calls = [1, 1, 1, 1];
    let save = json!({"target": "focused_output", "save_path": "one.png"});
    for (index, (client, name)) in clients.iter_mut().enumerate() {
        let result = client.call("screenshot", save.clone()).await;
        calls[index] += 1;
        if index % 2 == 0 {
            let path = fixture.path(&format!("home-{name}/shots/one.png"));
            assert_eq!(result["structuredContent"]["saved"]["path"], json!(path));
            assert!(path.exists());
        } else {
            assert_eq!(result["structuredContent"]["error"], "save_not_enabled");
        }
    }
    // The first session's keyboard backend doesn't exist; the plain one's is wtype.
    let key = json!({"keys": ["Down"], "expect": "none"});
    let first = &mut clients[0].0;
    first.structured("acquire_desktop").await;
    let (name, detail) = tool_error(&first.call("key", key.clone()).await);
    assert_eq!(name, "refused");
    assert!(detail.contains("NIRI_COMPUTER_USE_KEYBOARD"), "{detail}");
    first
        .structured_with("release_desktop", json!({"restore_focus": false}))
        .await;
    let plain = &mut clients[1].0;
    plain.structured("acquire_desktop").await;
    assert_eq!(plain.call("key", key).await["isError"], false);
    assert!(fixture.path("wtype.calls").exists());
    calls[0] += 3;
    calls[1] += 2;
    let audited = fixture.audit_lines();
    for ((client, _), count) in clients.iter().zip(calls) {
        let label = label(client);
        let lines = audited
            .iter()
            .filter(|line| line["session"] == label.as_str());
        assert_eq!(lines.count(), count, "{label}: {audited:#?}");
    }
    assert_eq!(audited.len(), calls.iter().sum::<usize>(), "{audited:#?}");
}

/// Sixteen calls running is each session's own limit: while one client's sixteen
/// screenshots wait for grim, another client still starts sixteen, and only each one's
/// seventeenth is refused.
#[tokio::test]
async fn each_session_runs_sixteen_calls_whatever_the_others_run() {
    let fixture = shared("many-cap");
    let _niri = crate::niri::Niri::start(&fixture);
    std::fs::write(fixture.path("grim.out"), jpeg(1280, 720, b"pixels")).unwrap();
    fixture.program(
        "grim",
        r#"echo >> "$DIR/grim.started"; await_file go; cat "$DIR/grim.out""#,
    );
    let mut clients = bridges(&fixture, 4).await;
    let target = json!({"target": "focused_output"});
    let mut running = Vec::new();
    for client in &mut clients[..2] {
        let mut ids = Vec::new();
        for _ in 0..16 {
            ids.push(client.start_call("screenshot", target.clone()).await);
        }
        running.push(ids);
    }
    let started = || {
        std::fs::read_to_string(fixture.path("grim.started"))
            .map_or(0, |started| started.lines().count())
    };
    assert!(eventually(WAIT, || started() == 32).await, "{}", started());
    for client in &mut clients[..2] {
        let refused = client.start_call("screenshot", target.clone()).await;
        let response = client.response(refused).await;
        let message = response["error"]["message"].as_str().unwrap();
        assert!(message.contains("16 tool calls running"), "{response}");
    }
    for client in &mut clients[2..] {
        assert_eq!(client.call("status", json!({})).await["isError"], false);
    }
    std::fs::write(fixture.path("go"), "").unwrap();
    for (client, ids) in clients.iter_mut().zip(running) {
        for id in ids {
            assert_eq!(client.response(id).await["result"]["isError"], false);
        }
    }
}

/// A session's `grim` and `wtype` come from its own `PATH`, as with a standalone server,
/// and relative entries are under the client's own working directory, not the engine's.
/// A starts the engine, which works in A's directory, where `bin` has a `grim` that fails;
/// B's `PATH` is `bin` too.
#[tokio::test]
async fn each_client_runs_grim_and_wtype_from_its_own_path_and_directory() {
    let mut fixture = shared("many-path");
    let mut desktop = Desktop::new(&fixture);
    std::fs::write(fixture.path("small.jpg"), jpeg(1280, 720, b"small")).unwrap();
    fixture.program_at(&fixture.path("a/bin/grim"), "echo A-grim >&2; exit 1");
    fixture.program_at(&fixture.path("b/bin/grim"), r#"cat "$DIR/small.jpg""#);
    for name in ["a", "b"] {
        let wtype = format!(r#"cat > /dev/null; echo >> "$DIR/wtype-{name}""#);
        fixture.program_at(&fixture.path(&format!("{name}/bin/wtype")), &wtype);
    }
    // Noctalia, on the fixture's `PATH`, reports the session unlocked, for the lease.
    let mut path = OsString::from("bin:");
    path.push(fixture.path("bin"));
    fixture.set("PATH", path);
    let mut clients = Vec::new();
    for name in ["a", "b"] {
        fixture.work_in(fixture.path(name));
        clients.push(Server::start(&fixture).await);
    }
    let _stream = desktop.stream().await;
    let shot = json!({"target": "focused_output"});
    let (_, detail) = tool_error(&clients[0].call("screenshot", shot.clone()).await);
    assert!(detail.contains("A-grim"), "{detail}");
    let taken = clients[1].call("screenshot", shot).await;
    assert_eq!(taken["structuredContent"]["width"], 1280, "{taken}");
    let key = json!({"keys": ["Down"], "expect": "none"});
    for (client, name) in clients.iter_mut().zip(["a", "b"]) {
        client.structured("acquire_desktop").await;
        let pressed = client.call("key", key.clone()).await;
        assert_eq!(
            pressed["structuredContent"]["observed"], "sent",
            "{pressed}"
        );
        client
            .structured_with("release_desktop", json!({"restore_focus": false}))
            .await;
        let calls = std::fs::read_to_string(fixture.path(&format!("wtype-{name}"))).unwrap();
        assert_eq!(calls.lines().count(), 1, "{name}");
    }
}

/// An empty `PATH` is the working directory, as for `execvp`, not the default search path
/// an unset one gets, standalone or shared.
#[tokio::test]
async fn an_empty_path_finds_grim_in_the_working_directory() {
    let mut fixture = Fixture::new("empty-path");
    let mut desktop = Desktop::new(&fixture);
    std::fs::write(fixture.path("small.jpg"), jpeg(1280, 720, b"small")).unwrap();
    fixture.program_at(&fixture.path("work/grim"), r#"cat "$DIR/small.jpg""#);
    fixture.set("PATH", "");
    fixture.work_in(fixture.path("work"));
    let mut client = Server::start(&fixture).await;
    let _stream = desktop.stream().await;
    let binaries = client.structured("status").await["binaries"].clone();
    assert_eq!(binaries["grim"], true, "{binaries}");
    let taken = client
        .call("screenshot", json!({"target": "focused_output"}))
        .await;
    assert_eq!(taken["structuredContent"]["width"], 1280, "{taken}");
}

/// A working directory whose name has a `:` can't be written into a `PATH`, so it must
/// not be. A `grim` found through an empty or a relative entry still runs with the
/// client's `PATH`, and finds its helper there, standalone or shared.
#[tokio::test]
async fn a_working_directory_with_a_colon_keeps_the_clients_path() {
    let mut fixture = Fixture::new("colon-cwd");
    let mut desktop = Desktop::new(&fixture);
    std::fs::write(fixture.path("small.jpg"), jpeg(1280, 720, b"small")).unwrap();
    let mut clients = Vec::new();
    for (path, cwd, bin) in [
        ("", "empty:cwd", "empty:cwd"),
        ("bin", "relative:cwd", "relative:cwd/bin"),
    ] {
        let bin = fixture.path(bin);
        file_at(
            &bin.join("grim"),
            "#!/bin/sh\nexec ncu-local-helper\n",
            0o755,
        );
        fixture.program_at(&bin.join("ncu-local-helper"), r#"cat "$DIR/small.jpg""#);
        fixture.set("PATH", path);
        fixture.work_in(fixture.path(cwd));
        clients.push(Server::start(&fixture).await);
    }
    let _stream = desktop.stream().await;
    for client in &mut clients {
        let taken = client
            .call("screenshot", json!({"target": "focused_output"}))
            .await;
        assert_eq!(taken["structuredContent"]["width"], 1280, "{taken}");
    }
}

/// As with `execvp`, a `grim` on `PATH` that can't be started, because only others may
/// execute it or its interpreter is missing, is passed over for the next one.
#[tokio::test]
async fn a_grim_that_cant_start_is_passed_over_for_the_next_on_path() {
    let mut fixture = Fixture::new("unstartable");
    let mut desktop = Desktop::new(&fixture);
    std::fs::write(fixture.path("small.jpg"), jpeg(1280, 720, b"small")).unwrap();
    fixture.program_at(&fixture.path("good/grim"), r#"cat "$DIR/small.jpg""#);
    let others = "#!/bin/sh\necho FIRST-grim >&2; exit 1\n";
    let mut clients = Vec::new();
    for (first, text, mode) in [
        ("others", others, 0o645),
        ("no-interpreter", "#!/dev/null/no-interpreter\n", 0o755),
    ] {
        file_at(&fixture.path(first).join("grim"), text, mode);
        let mut path = fixture.path(first).into_os_string();
        path.push(":");
        path.push(fixture.path("good"));
        fixture.set("PATH", path);
        clients.push(Server::start(&fixture).await);
    }
    let _stream = desktop.stream().await;
    for client in &mut clients {
        let taken = client
            .call("screenshot", json!({"target": "focused_output"}))
            .await;
        assert_eq!(taken["structuredContent"]["width"], 1280, "{taken}");
    }
}

/// Starts a client in `work` with `bin` and then `extra` on its `PATH`. The fixture's own
/// `bin`, where Noctalia reports the session unlocked for the lease, comes last.
async fn client_in(fixture: &mut Fixture, work: &str, extra: &str) -> Server {
    let mut path = OsString::from("bin:");
    path.push(fixture.path(extra));
    path.push(":");
    path.push(fixture.path("bin"));
    fixture.set("PATH", path);
    fixture.work_in(fixture.path(work));
    Server::start(fixture).await
}

/// Takes a screenshot and sends one key with `client`, and returns `status.binaries`.
async fn shoot_and_type(client: &mut Server) -> serde_json::Value {
    let taken = client
        .call("screenshot", json!({"target": "focused_output"}))
        .await;
    assert_eq!(taken["structuredContent"]["width"], 1280, "{taken}");
    client.structured("acquire_desktop").await;
    let key = json!({"keys": ["Down"], "expect": "none"});
    let pressed = client.call("key", key).await;
    assert_eq!(
        pressed["structuredContent"]["observed"], "sent",
        "{pressed}"
    );
    client
        .structured_with("release_desktop", json!({"restore_focus": false}))
        .await;
    client.structured("status").await["binaries"].clone()
}

/// A client's calls stay in the directory it works in after that directory is renamed and
/// another is made under its old name, as a standalone server's do, standalone or shared.
#[tokio::test]
async fn a_renamed_working_directory_keeps_its_clients_programs() {
    let mut fixture = Fixture::new("cwd-renamed");
    let mut desktop = Desktop::new(&fixture);
    std::fs::write(fixture.path("small.jpg"), jpeg(1280, 720, b"small")).unwrap();
    fixture.program_at(&fixture.path("work/bin/grim"), r#"cat "$DIR/small.jpg""#);
    let wtype = r#"cat > /dev/null; echo >> "$DIR/wtype-original""#;
    fixture.program_at(&fixture.path("work/bin/wtype"), wtype);
    let mut client = client_in(&mut fixture, "work", "none").await;
    std::fs::rename(fixture.path("work"), fixture.path("moved")).unwrap();
    let replacement = "echo REPLACEMENT >&2; exit 1";
    fixture.program_at(&fixture.path("work/bin/grim"), replacement);
    let _stream = desktop.stream().await;
    let binaries = shoot_and_type(&mut client).await;
    assert_eq!(binaries["wtype"], true, "{binaries}");
    let calls = std::fs::read_to_string(fixture.path("wtype-original")).unwrap();
    assert_eq!(calls.lines().count(), 1);
}

/// A client whose working directory is removed still runs its programs from the rest of
/// its `PATH`, standalone or shared.
#[tokio::test]
async fn a_removed_working_directory_leaves_the_rest_of_the_path() {
    let mut fixture = Fixture::new("cwd-removed");
    let mut desktop = Desktop::new(&fixture);
    std::fs::write(fixture.path("small.jpg"), jpeg(1280, 720, b"small")).unwrap();
    fixture.program_at(&fixture.path("tools/grim"), r#"cat "$DIR/small.jpg""#);
    let wtype = r#"cat > /dev/null; echo >> "$DIR/wtype-tools""#;
    fixture.program_at(&fixture.path("tools/wtype"), wtype);
    std::fs::create_dir(fixture.path("gone")).unwrap();
    let mut client = client_in(&mut fixture, "gone", "tools").await;
    std::fs::remove_dir(fixture.path("gone")).unwrap();
    let _stream = desktop.stream().await;
    let binaries = shoot_and_type(&mut client).await;
    assert_eq!(binaries["grim"], true, "{binaries}");
    assert_eq!(binaries["wtype"], true, "{binaries}");
    let calls = std::fs::read_to_string(fixture.path("wtype-tools")).unwrap();
    assert_eq!(calls.lines().count(), 1);
}
