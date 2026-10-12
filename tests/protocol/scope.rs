//! The engine a bridge starts through `systemd-run --user --scope`, to give it a scope of
//! its own, against a fake `systemd-run`. The nested engine suite runs the real one.

use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::client::{Server, WAIT, tool_error};
use crate::fixture::{Fixture, eventually, exited, kill, parent, pid_in};
use crate::niri::Niri;
use crate::shared::{engine, hold_engine_lock, shared};

/// A fake `systemd-run` that records its arguments and PID, then runs `then`.
fn systemd_run(fixture: &Fixture, then: &str) {
    fixture.program(
        "systemd-run",
        &format!(
            r#"printf '%s\n' "$@" > "$DIR/systemd-run.args"; echo $$ > "$DIR/systemd-run.pid"
{then}"#
        ),
    );
}

/// What `systemd-run --scope` does once the scope runs: it becomes the command after
/// `--`, with the environment it was given.
fn become_the_command(fixture: &Fixture) -> String {
    format!(
        r#"while [ "$1" != "--" ]; do shift; done; shift; PATH='{}' exec "$@""#,
        fixture.path("bin").display()
    )
}

async fn engine_status(client: &mut Server) -> Value {
    client.structured("status").await["engine"].clone()
}

#[tokio::test]
async fn a_bridge_starts_the_engine_through_systemd_run() {
    let fixture = shared("scope-run");
    let _niri = Niri::start(&fixture);
    systemd_run(&fixture, &become_the_command(&fixture));
    let mut client = Server::start(&fixture).await;
    let engine = engine_status(&mut client).await;
    assert_eq!(engine["mode"], "shared", "{engine}");
    let pid = pid_in(&fixture.path("systemd-run.pid")).await;
    assert_eq!(engine["pid"], pid, "systemd-run didn't become the engine");
    assert!(engine["unscoped"].is_null(), "{engine}");
    assert!(
        engine["cgroup"].as_str().unwrap().starts_with('/'),
        "{engine}"
    );
    let exe = format!("/proc/{}/exe", client.pid);
    assert_eq!(
        fixture.args("systemd-run"),
        [
            "--user",
            "--scope",
            "--collect",
            "--quiet",
            "--description=niri-computer-use shared engine",
            "--",
            &exe,
            "engine"
        ]
    );
}

/// A `systemd-run` that can't reach systemd, or one that hangs: the bridge starts the
/// engine itself, at once or once two seconds have passed, and its client is served shared.
/// A hung `systemd-run` is killed and reaped. The engine's `status` says why it has no
/// scope of its own, though the engine empties the log `systemd-run` wrote to.
#[tokio::test]
async fn a_bridge_whose_systemd_run_fails_or_hangs_starts_the_engine_itself() {
    let refuses = r#"echo "Failed to connect to user scope bus" >&2; exit 1"#;
    let cases = [
        (
            refuses,
            1,
            "systemd-run exited with exit status: 1: Failed to connect to user scope bus",
        ),
        (
            "await_file never",
            4,
            "systemd-run didn't start the engine within 2 s, so it was killed",
        ),
    ];
    for (then, within, why) in cases {
        let fixture = shared("scope-failed");
        let _niri = Niri::start(&fixture);
        systemd_run(&fixture, then);
        let started = Instant::now();
        let mut client = Server::start(&fixture).await;
        let took = started.elapsed();
        assert!(took < Duration::from_secs(within), "{then}: {took:?}");
        let engine = engine_status(&mut client).await;
        assert_eq!(engine["mode"], "shared", "{engine}");
        assert!(engine["fallback"].is_null(), "{engine}");
        assert_eq!(engine["unscoped"], why, "{engine}");
        let pid = u32::try_from(engine["pid"].as_u64().unwrap()).unwrap();
        assert_eq!(parent(pid), client.pid, "{then}");
        let systemd_run = pid_in(&fixture.path("systemd-run.pid")).await;
        assert!(
            !Path::new(&format!("/proc/{systemd_run}")).exists(),
            "{then}: systemd-run is left behind"
        );
    }
}

/// An engine started for a bridge that discovered `XDG_RUNTIME_DIR` inherits the directory
/// from `systemd-run`, and reports it as discovered, as the bridge found it.
#[tokio::test]
async fn an_engine_told_its_runtime_dir_was_discovered_reports_it_so() {
    let fixture = shared("scope-discovered");
    let _niri = Niri::start(&fixture);
    let discovered = format!(
        r#"while [ "$1" != "--" ]; do shift; done; PATH='{}' exec "$2" engine --runtime-dir-discovered"#,
        fixture.path("bin").display()
    );
    systemd_run(&fixture, &discovered);
    let mut client = Server::start(&fixture).await;
    let status = client.structured("status").await;
    assert_eq!(
        status["engine"]["pid"],
        pid_in(&fixture.path("systemd-run.pid")).await
    );
    assert_eq!(
        status["discovery"]["runtime_dir"]["source"], "discovered",
        "{status}"
    );
    assert!(status["engine"]["unscoped"].is_null(), "{status}");
}

/// Kills the engine `client` is served by, and has the client hear of it.
async fn lose_engine(fixture: &Fixture, client: &mut Server) {
    kill(u32::try_from(engine(fixture).await).unwrap());
    let (lost, _) = tool_error(&client.call("status", json!({})).await);
    assert_eq!(lost, "engine_lost");
}

/// Has `client`'s next call start a reconnect through a `systemd-run` that runs `then`,
/// and leaves that call in flight. Returns the `systemd-run`, once it has written its PID.
async fn reconnecting(fixture: &Fixture, client: &mut Server, then: &str) -> i32 {
    systemd_run(fixture, then);
    client.start_call("status", json!({})).await;
    pid_in(&fixture.path("systemd-run.pid")).await
}

/// A client that goes while its bridge waits for a hung `systemd-run` to start a new
/// engine: the bridge exits, and the `systemd-run` goes with it, without its two seconds.
#[tokio::test]
async fn a_client_leaving_during_a_reconnect_leaves_no_systemd_run() {
    let fixture = shared("scope-cancel");
    let _niri = Niri::start(&fixture);
    let mut client = Server::start(&fixture).await;
    lose_engine(&fixture, &mut client).await;
    let launcher = reconnecting(&fixture, &mut client, "await_file never").await;
    let left = Instant::now();
    client.close_stdin();
    assert!(client.exit_within(WAIT).await.is_some());
    assert!(
        eventually(Duration::from_secs(1), || exited(launcher)).await,
        "systemd-run outlived the bridge"
    );
    assert!(
        left.elapsed() < Duration::from_secs(2),
        "{:?}",
        left.elapsed()
    );
}

/// The same, once `systemd-run` has become the engine, which still waits for the lock:
/// the engine is the instance's, not the bridge's, and stays.
#[tokio::test]
async fn a_client_leaving_during_a_reconnect_keeps_an_engine_systemd_run_became() {
    let fixture = shared("scope-became");
    let _niri = Niri::start(&fixture);
    let mut client = Server::start(&fixture).await;
    lose_engine(&fixture, &mut client).await;
    let lock = hold_engine_lock(&fixture);
    let becomes = r#"while [ "$1" != "--" ]; do shift; done; shift; exec "$@""#;
    let launcher = reconnecting(&fixture, &mut client, becomes).await;
    let exe = std::fs::canonicalize(env!("CARGO_BIN_EXE_niri-computer-use")).unwrap();
    let became = || std::fs::read_link(format!("/proc/{launcher}/exe")).is_ok_and(|at| at == exe);
    assert!(
        eventually(WAIT, became).await,
        "systemd-run didn't become the engine"
    );
    client.close_stdin();
    assert!(client.exit_within(WAIT).await.is_some());
    assert!(
        !exited(launcher),
        "the bridge killed the engine systemd-run became"
    );
    drop(lock);
}
