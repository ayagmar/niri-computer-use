//! The engine a bridge starts through `systemd-run --user --scope`, to give it a scope of
//! its own, against a fake `systemd-run`. The nested engine suite runs the real one.

use std::time::{Duration, Instant};

use serde_json::Value;

use crate::client::Server;
use crate::fixture::{Fixture, parent, pid_in};
use crate::niri::Niri;
use crate::shared::shared;

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
#[tokio::test]
async fn a_bridge_whose_systemd_run_fails_or_hangs_starts_the_engine_itself() {
    let refuses = r#"echo "Failed to connect to user scope bus" >&2; exit 1"#;
    for (then, within) in [(refuses, 1), ("await_file never", 4)] {
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
        let pid = u32::try_from(engine["pid"].as_u64().unwrap()).unwrap();
        assert_eq!(parent(pid), client.pid, "{then}");
        assert!(fixture.path("systemd-run.args").exists());
    }
}
