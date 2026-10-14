//! `niri-computer-use recover` against a fixture's runtime directory: a live lease holder,
//! the input a marker names, and the human's answer.

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};

use crate::client::{Server, WAIT, answer, run, subcommand, tool_error};
use crate::fixture::Fixture;
use crate::session::NiriProcess;

const DIR: &str = "run/niri-computer-use/niri.test";

fn write_marker(fixture: &Fixture, marker: &Value) {
    std::fs::create_dir_all(fixture.path(DIR)).unwrap();
    std::fs::write(
        fixture.path(&format!("{DIR}/input-dirty")),
        marker.to_string(),
    )
    .unwrap();
}

fn marker_exists(fixture: &Fixture) -> bool {
    fixture.path(&format!("{DIR}/input-dirty")).exists()
}

/// A marker 0.1.0 wrote for its input child, which this version can't read.
fn with_child() -> Value {
    json!({
        "operation": "type_text", "phase": "running", "server_pid": 1,
        "since": "2026-10-08T00:00:00.000Z",
        "child": {"pid": 1, "start_time": 1}
    })
}

fn pending() -> Value {
    json!({"operation": "key", "phase": "pending", "server_pid": 1, "since": "t"})
}

#[tokio::test]
async fn recover_refuses_while_a_server_holds_the_lease() {
    let fixture = Fixture::new("recover-live");
    let _niri = NiriProcess::unlocked(&fixture).await;
    let mut server = Server::start(&fixture).await;
    server.structured("acquire_desktop").await;
    write_marker(
        &fixture,
        &json!({"operation": "key", "phase": "pending", "server_pid": 1, "since": "t"}),
    );
    let holder = server.serving_pid().await;
    // More answers than a pipe holds: recover refuses before it reads any of them.
    let out = answer(&fixture, "recover", &"yes\n".repeat(32 * 1024)).await;
    assert!(!out.status.success());
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains(&format!("PID {holder}")), "{stderr}");
    assert!(marker_exists(&fixture));
}

/// An unreadable marker, such as one an older version wrote, blocks the lease like any
/// other until the human confirms the input is released.
#[tokio::test]
async fn recover_clears_an_unreadable_marker_once_confirmed() {
    let fixture = Fixture::new("recover-old");
    let _niri = NiriProcess::unlocked(&fixture).await;
    write_marker(&fixture, &with_child());
    let mut server = Server::start(&fixture).await;
    let (name, detail) = tool_error(&server.call("acquire_desktop", json!({})).await);
    assert_eq!(name, "recovery_required");
    assert!(detail.contains("recover"), "{detail}");

    let out = answer(&fixture, "recover", "yes\n").await;
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(out.status.success(), "{stdout}");
    assert!(!marker_exists(&fixture));
    server.structured("acquire_desktop").await;
}

#[tokio::test]
async fn without_a_yes_the_marker_stays() {
    let fixture = Fixture::new("recover-no");
    write_marker(&fixture, &pending());
    let out = answer(&fixture, "recover", "no\n").await;
    assert!(!out.status.success());
    assert!(
        String::from_utf8(out.stderr)
            .unwrap()
            .contains("not confirmed")
    );
    assert!(marker_exists(&fixture));
    // End of input is not a yes either.
    assert!(!answer(&fixture, "recover", "").await.status.success());
    assert!(marker_exists(&fixture));
}

#[tokio::test]
async fn a_marker_written_while_recover_waits_for_the_answer_stays() {
    let fixture = Fixture::new("recover-replaced");
    write_marker(&fixture, &pending());
    let mut recover = subcommand(&fixture, "recover");
    let mut stdout = BufReader::new(recover.stdout.take().unwrap()).lines();
    while let Some(line) = tokio::time::timeout(WAIT, stdout.next_line())
        .await
        .unwrap()
        .unwrap()
    {
        if line.contains("Is all input released?") {
            break;
        }
    }
    // Another input's marker, written after recover read the first one.
    let theirs = json!({"operation": "click", "phase": "pending", "server_pid": 2, "since": "t", "buttons": [272]});
    write_marker(&fixture, &theirs);
    let mut stdin = recover.stdin.take().unwrap();
    stdin.write_all(b"yes\n").await.unwrap();
    drop(stdin);
    let out = tokio::time::timeout(WAIT, recover.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        stderr.contains("run `niri-computer-use recover` again"),
        "{stderr}"
    );
    let left = std::fs::read_to_string(fixture.path(&format!("{DIR}/input-dirty"))).unwrap();
    assert_eq!(serde_json::from_str::<Value>(&left).unwrap(), theirs);
}

#[tokio::test]
async fn a_pointer_marker_without_a_niri_asks_the_human_to_release_its_buttons() {
    let fixture = Fixture::new("recover-pointer");
    write_marker(
        &fixture,
        &json!({
            "operation": "drag", "phase": "pending", "server_pid": 1, "since": "t",
            "buttons": [272], "output": "DP-1"
        }),
    );
    // Without a niri to send the release to, the human is asked to release the button.
    let out = answer(&fixture, "recover", "yes\n").await;
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(out.status.success(), "{stdout}");
    assert!(
        stdout.contains("pointer buttons [272] may be held, and their release couldn't be sent"),
        "{stdout}"
    );
    assert!(!marker_exists(&fixture));
}

#[tokio::test]
async fn failed_native_release_keeps_the_marker_even_after_a_yes() {
    let fixture = Fixture::new("recover-native");
    write_marker(
        &fixture,
        &json!({
            "operation": "type_text", "phase": "pending", "server_pid": 1, "since": "t",
            "keyboard": {"codes": [30], "group": 0}
        }),
    );
    let out = answer(&fixture, "recover", "yes\n").await;
    assert!(!out.status.success());
    assert!(marker_exists(&fixture));
}

#[tokio::test]
async fn native_recovery_refuses_invalid_protocol_codes() {
    let fixture = Fixture::new("recover-native-code");
    write_marker(
        &fixture,
        &json!({
            "operation": "key", "phase": "pending", "server_pid": 1, "since": "t",
            "keyboard": {"codes": [u32::MAX], "group": 0}
        }),
    );
    let out = answer(&fixture, "recover", "yes\n").await;
    assert!(!out.status.success());
    assert!(
        String::from_utf8(out.stderr)
            .unwrap()
            .contains("invalid native keycodes")
    );
    assert!(marker_exists(&fixture));
}

#[tokio::test]
async fn nothing_to_recover_is_fine() {
    let fixture = Fixture::new("recover-none");
    let out = run(&fixture, "recover").await;
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        "No input-dirty marker: nothing to recover.\n"
    );
}
