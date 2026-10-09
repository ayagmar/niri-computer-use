//! The lease over stdio: two servers for one niri instance, the stop flag, and what
//! `status` and the audit log say.

use std::time::Duration;

use serde_json::{Value, json};

use crate::client::{CLIENT, Server, run, tool_error};
use crate::fixture::Fixture;
use crate::niri::Niri;
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
    assert_eq!(held["holder"]["pid"], first.pid);
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
