//! The lease over stdio: two servers for one niri instance, the stop flag, and what
//! `status` and the audit log say.

use std::time::Duration;

use serde_json::{Value, json};

use crate::client::{CLIENT, Server, WAIT, run, tool_error};
use crate::fixture::{Fixture, eventually, exited, kill, shared_mode};
use crate::guard::guardian;
use crate::niri::{Niri, window_on};
use crate::noctalia::{self, UNLOCKED};
use crate::session::NiriProcess;

#[tokio::test]
async fn one_server_holds_the_lease_and_the_other_is_told_who() {
    let fixture = Fixture::new("lease");
    let _niri = NiriProcess::unlocked(&fixture).await;
    let mut first = Server::start(&fixture).await;
    let mut second = Server::start(&fixture).await;
    let label = format!("{CLIENT}/{}", first.pid);

    let held = first.structured("acquire_desktop").await;
    assert_eq!(held["holder"]["label"], label.as_str());
    assert_eq!(held["holder"]["pid"], first.serving_pid().await);
    // Asking again while holding it is fine.
    assert_eq!(first.structured("acquire_desktop").await, held);

    let (name, detail) = tool_error(&second.call("acquire_desktop", json!({})).await);
    assert_eq!(name, "lease_held");
    assert!(detail.contains(&format!("({label})")), "{detail}");
    let seen = second.structured("status").await;
    assert_eq!(
        seen["lease"],
        json!({"held_by_me": false, "holder": held["holder"]})
    );
    assert_eq!(
        first.structured("status").await["lease"]["held_by_me"],
        true
    );

    assert_eq!(
        first
            .structured_with("release_desktop", json!({"restore_focus": false}))
            .await,
        json!({"released": true, "users_window": null})
    );
    assert_eq!(
        first
            .structured_with("release_desktop", json!({"restore_focus": false}))
            .await,
        json!({"released": false, "users_window": null})
    );
    let taken = second.structured("acquire_desktop").await;
    assert_eq!(
        taken["holder"]["label"],
        format!("{CLIENT}/{}", second.pid).as_str()
    );
    let tools: Vec<Value> = fixture
        .audit_lines()
        .into_iter()
        .filter(|line| line["tool"] != "status")
        .map(|line| json!([line["tool"], line["error"]]))
        .collect();
    assert_eq!(
        tools,
        [
            json!(["acquire_desktop", null]),
            json!(["acquire_desktop", null]),
            json!(["acquire_desktop", "lease_held"]),
            json!(["release_desktop", null]),
            json!(["release_desktop", null]),
            json!(["acquire_desktop", null]),
        ]
    );
}

#[tokio::test]
async fn a_server_that_exits_gives_the_lease_up() {
    let fixture = Fixture::new("lease-exit");
    let _niri = NiriProcess::unlocked(&fixture).await;
    let mut first = Server::start(&fixture).await;
    first.structured("acquire_desktop").await;
    let (status, _, _) = first.stop().await;
    assert!(status.success());
    let mut second = Server::start(&fixture).await;
    assert_eq!(
        second.structured("status").await["lease"]["holder"],
        Value::Null
    );
    second.structured("acquire_desktop").await;
}

#[tokio::test]
async fn the_stop_flag_takes_the_lease_back_and_refuses_it_until_resume() {
    let fixture = Fixture::new("lease-stop");
    let _niri = NiriProcess::unlocked(&fixture).await;
    let mut server = Server::start(&fixture).await;
    server.structured("acquire_desktop").await;
    assert!(run(&fixture, "stop").await.status.success());
    let mut released = false;
    for _ in 0..250 {
        if server.structured("status").await["lease"]["held_by_me"] == false {
            released = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(released, "the stop flag didn't take the lease back");
    let (name, _) = tool_error(&server.call("acquire_desktop", json!({})).await);
    assert_eq!(name, "stopped");
    assert!(run(&fixture, "resume").await.status.success());
    server.structured("acquire_desktop").await;
}

#[tokio::test]
async fn the_lease_is_refused_while_input_may_be_stuck() {
    let fixture = Fixture::new("lease-dirty");
    let mut server = Server::start(&fixture).await;
    std::fs::create_dir_all(fixture.path("run/niri-computer-use/niri.test")).unwrap();
    std::fs::write(
        fixture.path("run/niri-computer-use/niri.test/input-dirty"),
        "",
    )
    .unwrap();
    let (name, _) = tool_error(&server.call("acquire_desktop", json!({})).await);
    assert_eq!(name, "recovery_required");
}

#[tokio::test]
async fn an_invalid_policy_file_makes_the_server_read_only() {
    let fixture = Fixture::new("lease-policy");
    let _niri = Niri::start(&fixture);
    std::fs::create_dir_all(fixture.path("config/niri-computer-use")).unwrap();
    std::fs::write(
        fixture.path("config/niri-computer-use/policy.toml"),
        "[[preset]]\nname = \"shell\"\nargv = [\"bash\"]\napp_id = \"x\"\n",
    )
    .unwrap();
    let mut server = Server::start(&fixture).await;
    let policy = &server.structured("status").await["policy"];
    assert_eq!(policy["state"], "invalid");
    assert!(policy["error"].as_str().unwrap().contains("starts bash"));
    let (name, detail) = tool_error(&server.call("acquire_desktop", json!({})).await);
    assert_eq!(name, "read_only");
    assert!(detail.contains("policy file is invalid"), "{detail}");
}

#[tokio::test]
async fn a_locked_screen_refuses_the_lease() {
    let fixture = Fixture::new("lease-locked");
    fixture.program("loginctl", "echo yes");
    let _niri = NiriProcess::start(&fixture, Some("c4")).await;
    let mut server = Server::start(&fixture).await;
    let (name, _) = tool_error(&server.call("acquire_desktop", json!({})).await);
    assert_eq!(name, "screen_locked");
    fixture.program("loginctl", "echo no");
    server.structured("acquire_desktop").await;
}

#[tokio::test]
async fn an_unknown_lock_state_refuses_the_lease() {
    let fixture = Fixture::new("lease-unknown");
    fixture.program("loginctl", "echo 'Failed to get session' >&2; exit 1");
    let _niri = NiriProcess::start(&fixture, Some("c4")).await;
    let mut server = Server::start(&fixture).await;
    assert_eq!(
        server.structured("status").await["lock"]["state"],
        "unknown"
    );
    let (name, detail) = tool_error(&server.call("acquire_desktop", json!({})).await);
    assert_eq!(name, "screen_locked");
    assert!(detail.contains("unknown"), "{detail}");
}

/// niri restarted under a new socket: a server for the new instance has a lease and flags
/// of its own, while what the old one left stays where it was, for a human to look at. The
/// old server's guardian, ending with it, sends nothing to the new niri for its marker.
#[tokio::test]
async fn a_restarted_niri_shares_nothing_with_the_old_instance() {
    let mut fixture = Fixture::new("lease-restart");
    fixture.program("noctalia", "exit 0");
    let _noctalia = noctalia::start(&fixture, UNLOCKED);
    let mut old_niri = Niri::start(&fixture);
    let mut old = Server::start(&fixture).await;
    let old_stream = old_niri.stream().await;
    old_stream.initial(&[window_on(1, Some("a"), 1, true)]);
    old_stream.workspaces(1);
    old.structured("acquire_desktop").await;
    let serving = old.serving_pid().await;
    let guardian = guardian(serving);
    let marker = fixture.path("run/niri-computer-use/niri.test/input-dirty");
    let releasable = json!({
        "operation": "drag", "phase": "pending", "server_pid": serving,
        "since": "2026-10-10T00:00:00.000Z", "buttons": [272]
    })
    .to_string();
    std::fs::write(&marker, &releasable).unwrap();

    drop((old_stream, old_niri));
    std::fs::remove_file(fixture.niri_socket()).unwrap();
    let restarted = fixture.path("run/niri.restarted.sock");
    let mut new_niri = Niri::listen(&restarted);
    let old_socket = fixture.niri_socket();
    fixture.set("NIRI_SOCKET", &restarted);
    let mut new = Server::start(&fixture).await;
    let new_stream = new_niri.stream().await;
    new_stream.initial(&[window_on(1, Some("a"), 1, true)]);
    new_stream.workspaces(1);
    assert!(run(&fixture, "stop").await.status.success());
    assert_eq!(new.structured("status").await["stop"], true);
    assert_eq!(old.structured("status").await["stop"], false);
    assert!(run(&fixture, "resume").await.status.success());
    new.structured("acquire_desktop").await;

    // The old instance's socket is gone, so nothing resolves to it any more.
    fixture.set("NIRI_SOCKET", old_socket);
    let recover = run(&fixture, "recover").await;
    assert!(!recover.status.success(), "{recover:?}");
    let stderr = String::from_utf8_lossy(&recover.stderr);
    assert!(stderr.contains("can't be resolved"), "{stderr}");
    assert!(marker.exists());
    let release = json!({"restore_focus": false});
    assert_eq!(
        old.structured_with("release_desktop", release).await["released"],
        true
    );
    assert_eq!(new.structured("status").await["lease"]["held_by_me"], true);

    if shared_mode() {
        kill(serving);
    } else {
        old.kill().await;
    }
    assert!(eventually(WAIT, || exited(guardian)).await);
    assert!(!new_niri.connected_from(guardian));
    assert_eq!(std::fs::read_to_string(&marker).unwrap(), releasable);
}
