//! `niri-computer-use recover` against a fixture's runtime directory: a live lease holder,
//! a marker naming a running child, and the human's answer.

use std::process::Stdio;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};

use crate::client::{Server, WAIT, answer, run, subcommand, tool_error};
use crate::fixture::{Fixture, eventually, exited};
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

/// A child in a process group of its own that ignores SIGTERM, as a stuck input child
/// might, and its start time from `/proc`.
fn stubborn_child() -> (tokio::process::Child, u32, u64) {
    let child = command()
        .args(["-c", "trap '' TERM; exec sleep 60"])
        .stdin(Stdio::null())
        .process_group(0)
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let pid = child.id().unwrap();
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
    let (_, rest) = stat.rsplit_once(") ").unwrap();
    let start_time = rest.split(' ').nth(19).unwrap().parse().unwrap();
    (child, pid, start_time)
}

#[expect(
    clippy::disallowed_methods,
    reason = "the test starts the child a marker names, outside the server's runner"
)]
fn command() -> tokio::process::Command {
    tokio::process::Command::new("sh")
}

fn running(pid: u32, start_time: u64) -> Value {
    json!({
        "operation": "type_text", "phase": "running", "server_pid": 1,
        "since": "2026-10-08T00:00:00.000Z",
        "child": {"pid": pid, "start_time": start_time}
    })
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
    let out = answer(&fixture, "recover", "yes\n").await;
    assert!(!out.status.success());
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains(&format!("PID {holder}")), "{stderr}");
    assert!(marker_exists(&fixture));
}

#[tokio::test]
async fn recover_ends_the_child_and_clears_the_marker_once_confirmed() {
    let fixture = Fixture::new("recover-child");
    let _niri = NiriProcess::unlocked(&fixture).await;
    let (_child, pid, start_time) = stubborn_child();
    write_marker(&fixture, &running(pid, start_time));
    let mut server = Server::start(&fixture).await;
    let status = server.structured("status").await;
    assert_eq!(status["input_dirty"]["phase"], "running");
    let (name, detail) = tool_error(&server.call("acquire_desktop", json!({})).await);
    assert_eq!(name, "recovery_required");
    assert!(detail.contains("type_text running"), "{detail}");

    let out = answer(&fixture, "recover", "yes\n").await;
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(out.status.success(), "{stdout}");
    assert!(
        stdout.contains(&format!("Ending the input child, PID {pid}")),
        "{stdout}"
    );
    assert!(
        eventually(std::time::Duration::from_secs(2), || exited(
            i32::try_from(pid).unwrap()
        ))
        .await
    );
    assert!(!marker_exists(&fixture));
    server.structured("acquire_desktop").await;
}

#[tokio::test]
async fn without_a_yes_the_marker_stays() {
    let fixture = Fixture::new("recover-no");
    let (_child, pid, start_time) = stubborn_child();
    write_marker(&fixture, &running(pid, start_time));
    let out = answer(&fixture, "recover", "no\n").await;
    assert!(!out.status.success());
    assert!(
        String::from_utf8(out.stderr)
            .unwrap()
            .contains("not confirmed")
    );
    // The child is ended before the question; only the marker waits for the human.
    assert!(
        eventually(std::time::Duration::from_secs(2), || exited(
            i32::try_from(pid).unwrap()
        ))
        .await
    );
    assert!(marker_exists(&fixture));
    // End of input is not a yes either.
    assert!(!answer(&fixture, "recover", "").await.status.success());
    assert!(marker_exists(&fixture));
}

#[tokio::test]
async fn a_marker_written_while_recover_waits_for_the_answer_stays() {
    let fixture = Fixture::new("recover-replaced");
    let (_child, pid, start_time) = stubborn_child();
    write_marker(&fixture, &running(pid, start_time));
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
async fn a_reused_pid_is_never_killed() {
    let fixture = Fixture::new("recover-reused");
    let (_child, pid, start_time) = stubborn_child();
    // The marker's start time doesn't match, so the PID now belongs to someone else.
    write_marker(&fixture, &running(pid, start_time + 1));
    let out = answer(&fixture, "recover", "yes\n").await;
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(out.status.success(), "{stdout}");
    assert!(stdout.contains("has already exited"), "{stdout}");
    assert!(!exited(i32::try_from(pid).unwrap()));
    assert!(!marker_exists(&fixture));
}

#[tokio::test]
async fn a_pointer_marker_releases_its_buttons_without_asking_about_wtype() {
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
    assert!(!stdout.contains("wtype process"), "{stdout}");
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
    assert!(
        !String::from_utf8(out.stdout)
            .unwrap()
            .contains("wtype process")
    );
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
