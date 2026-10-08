//! M2's nested acceptance: `niri-computer-use` servers against the nested niri, with the
//! nested Noctalia as the lock source. Two servers compete for the lease, `recover` refuses
//! a live owner, a stop sent through niri's `spawn` action (as the stop keybind sends it)
//! takes the lease back, `resume` gives it out again, and `recover` ends a marker's
//! child. Paths are single-quoted in the shell scripts; none of them holds a quote. Every server, flag and marker lives under `TEST_DIR`.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use niri_ipc::{Action, Request};
use serde_json::Value;

use crate::failure::{Context as _, Failure, Result};
use crate::session::Session;

/// Within what is left of the run's deadline.
const NOCTALIA_DEADLINE: Duration = Duration::from_secs(80);
const READY: Duration = Duration::from_secs(20);
const WAIT: Duration = Duration::from_secs(5);
/// How long server A keeps its stdin open, and with it the lease.
const HOLD: &str = "25";
const HOLDER_DEADLINE: Duration = Duration::from_secs(35);
/// How long a one-shot server keeps stdin open for its replies.
const ANSWER: &str = "3";

pub(crate) fn run(session: &mut Session<'_>, server: &str) -> Result<()> {
    let noctalia = session.start(
        "noctalia",
        &[],
        session.artifact("noctalia.log"),
        NOCTALIA_DEADLINE,
    )?;
    ready(session, server)?;
    let control = session.control_dir()?;
    let mut holder = holds(session, server, &control)?;
    competes(session, server)?;
    let recover = shell(
        session,
        &format!("'{server}' recover < /dev/null 2>&1; echo exit=$?"),
    )?;
    expect(
        recover.contains("a server holds the lease") && recover.contains("exit=1"),
        "recover against a live owner",
        &recover,
    )?;
    session.log("M2: recover refused while server A held the lease")?;
    stops(session, server, &control, &mut holder)?;
    // A gave the lease up but runs until its stdin closes.
    holder.stop()?;
    recovers(session, server, &control)?;
    noctalia.stop().map(drop)
}

/// Waits until a server sees niri, the nested Noctalia, and an unlocked screen.
fn ready(session: &mut Session<'_>, server: &str) -> Result<()> {
    let status = session.wait_until(
        "m2-ready",
        "status with Noctalia running and the screen unlocked",
        READY,
        |session| {
            // A `status` that waits on a Noctalia still starting can miss the reply window;
            // that is not ready yet, not a failure.
            let Ok(status) = one_shot(session, server, "harness-ready", "status") else {
                return Ok(None);
            };
            let ready = field(&status, "/noctalia") == "running"
                && field(&status, "/lock/state") == "unlocked";
            Ok(ready.then_some(status))
        },
    )?;
    session.log(&format!(
        "M2 status: niri {}, lock {}, policy {}",
        field(&status, "/niri/compat"),
        field(&status, "/lock"),
        field(&status, "/policy/state")
    ))
}

/// Starts server A, which takes the lease and keeps it while its stdin stays open.
fn holds(
    session: &mut Session<'_>,
    server: &str,
    control: &Path,
) -> Result<crate::runner::Process> {
    let script = format!(
        "({}; sleep {HOLD}) | '{server}' serve",
        printf(&requests("harness-a", "acquire_desktop"))
    );
    let args = ["-c".into(), OsString::from(script)];
    let process = session.start(
        "sh",
        &args,
        session.artifact("server-a.log"),
        HOLDER_DEADLINE,
    )?;
    let record = control.join("lease.json");
    let label = session.wait_until("m2-lease", "server A holding the lease", WAIT, |_| {
        let holder: Option<Value> = fs::read_to_string(&record)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok());
        Ok(holder.and_then(|holder| field(&holder, "/label").as_str().map(str::to_owned)))
    })?;
    expect(label.starts_with("harness-a/"), "server A's lease", &label)?;
    session.log(&format!("M2: server A holds the lease as {label}"))?;
    Ok(process)
}

/// Server B is refused and told who holds the lease.
fn competes(session: &mut Session<'_>, server: &str) -> Result<()> {
    let error = one_shot(session, server, "harness-b", "acquire_desktop")?;
    expect(
        field(&error, "/error") == "lease_held"
            && field(&error, "/detail")
                .as_str()
                .is_some_and(|detail| detail.contains("(harness-a/")),
        "server B refused with lease_held naming server A",
        &error.to_string(),
    )?;
    session.log(&format!("M2: server B refused: {error}"))
}

/// The stop flag, set by a command niri spawns, takes A's lease and refuses B until resume.
fn stops(
    session: &mut Session<'_>,
    server: &str,
    control: &Path,
    holder: &mut crate::runner::Process,
) -> Result<()> {
    session.request(&Request::Action(Action::Spawn {
        command: vec![server.to_owned(), "stop".to_owned()],
    }))?;
    let record = control.join("lease.json");
    let flag = control.join("stop");
    session.wait_until(
        "m2-stop",
        "the stop flag set and the lease given up",
        WAIT,
        |_| {
            // Server A also empties the record when it exits, so it must still be running
            // for the release to be the stop's doing.
            holder.ensure_running()?;
            let released = fs::read_to_string(&record).is_ok_and(|text| text.is_empty());
            Ok((flag.exists() && released).then_some(()))
        },
    )?;
    session.log("M2: niri's spawn of `stop` set the flag and server A gave the lease up")?;
    let reply = one_shot(session, server, "harness-b", "acquire_desktop")?;
    let refused = field(&reply, "/error");
    expect(
        refused == "stopped",
        "server B refused while stopped",
        &refused.to_string(),
    )?;
    shell(session, &format!("'{server}' resume"))?;
    let taken = one_shot(session, server, "harness-b", "acquire_desktop")?;
    expect(
        field(&taken, "/holder/label")
            .as_str()
            .is_some_and(|label| label.starts_with("harness-b/")),
        "server B holding the lease after resume",
        &taken.to_string(),
    )?;
    session.log("M2: after resume, server B took the lease")
}

/// A marker naming a running child: `recover` ends it and clears the marker. The child
/// shares the supervisor's process group, so `recover` ends it alone; the unit tests cover
/// the group kill.
fn recovers(session: &mut Session<'_>, server: &str, control: &Path) -> Result<()> {
    let pid_file = control.join("child.pid");
    let script = format!("echo $$ > '{}'; exec sleep 60", pid_file.display());
    let args = ["-c".into(), OsString::from(script)];
    let child = session.start("sh", &args, session.artifact("child.log"), HOLDER_DEADLINE)?;
    let pid = session.wait_until("m2-child", "the child's PID", WAIT, |_| {
        Ok(fs::read_to_string(&pid_file)
            .ok()
            .and_then(|text| text.trim().parse::<u32>().ok()))
    })?;
    let start_time = start_time(pid)?;
    let marker = serde_json::json!({
        "operation": "type_text", "phase": "running", "server_pid": 1,
        "since": "2026-10-08T00:00:00.000Z",
        "child": {"pid": pid, "start_time": start_time}
    });
    let path = control.join("input-dirty");
    fs::write(&path, marker.to_string()).context(format!("write {}", path.display()))?;
    let output = shell(
        session,
        &format!("echo yes | '{server}' recover 2>&1; echo exit=$?"),
    )?;
    expect(output.contains("exit=0"), "recover with a marker", &output)?;
    session.wait_until("m2-recover", "the child ended", WAIT, |_| {
        Ok(start_time_of(pid)
            .is_none_or(|(state, start)| start != start_time || state == 'Z')
            .then_some(()))
    })?;
    expect(!path.exists(), "the marker cleared", &output)?;
    drop(child);
    session.log(&format!(
        "M2: recover ended child {pid} and cleared the marker"
    ))
}

/// One server for one call: initialize, the tool, and a few seconds for the replies.
fn one_shot(session: &Session<'_>, server: &str, client: &str, tool: &str) -> Result<Value> {
    let script = format!(
        "({}; sleep {ANSWER}) | '{server}' serve",
        printf(&requests(client, tool))
    );
    let output = shell(session, &script)?;
    let replies: Vec<Value> = output
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|reply| field(reply, "/id") == 2)
        .collect();
    match replies.as_slice() {
        [reply] => Ok(field(reply, "/result/structuredContent").clone()),
        _ => Err(Failure::new(format!("{tool}: no reply in {output:?}"))),
    }
}

/// `initialize`, `initialized`, and one `tools/call` with id 2.
fn requests(client: &str, tool: &str) -> Vec<String> {
    vec![
        serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-11-25", "capabilities": {},
            "clientInfo": {"name": client, "version": "1"}}})
        .to_string(),
        serde_json::json!({"jsonrpc": "2.0", "method": "notifications/initialized"}).to_string(),
        serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
            "params": {"name": tool, "arguments": {}}})
        .to_string(),
    ]
}

/// A `printf` writing each line, single-quoted for `sh`. The JSON holds no single quote.
fn printf(lines: &[String]) -> String {
    let quoted: Vec<String> = lines.iter().map(|line| format!("'{line}'")).collect();
    format!("printf '%s\\n' {}", quoted.join(" "))
}

/// Runs a shell command in NESTED and returns its stdout.
fn shell(session: &Session<'_>, script: &str) -> Result<String> {
    let output = session.run("sh", &["-c".into(), script.into()])?;
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn start_time(pid: u32) -> Result<u64> {
    start_time_of(pid)
        .map(|(_, start)| start)
        .ok_or_else(|| Failure::new(format!("no /proc entry for child {pid}")))
}

/// The state and start time from `/proc/<pid>/stat`, counted from the last `)`.
fn start_time_of(pid: u32) -> Option<(char, u64)> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let (_, rest) = stat.rsplit_once(") ")?;
    let mut fields = rest.split(' ');
    let state = fields.next()?.chars().next()?;
    Some((state, fields.nth(18)?.parse().ok()?))
}

/// The value at a JSON pointer, or null.
fn field<'a>(value: &'a Value, pointer: &str) -> &'a Value {
    value.pointer(pointer).unwrap_or(&Value::Null)
}

fn expect(holds: bool, what: &str, seen: &str) -> Result<()> {
    if holds {
        Ok(())
    } else {
        Err(Failure::new(format!("M2: {what} failed; saw {seen}")))
    }
}

/// The runtime directory the servers share for the nested niri.
pub(crate) fn runtime_dir(run: &Path, niri_socket: &Path) -> Result<PathBuf> {
    let name = niri_socket
        .file_name()
        .ok_or_else(|| Failure::new("the nested NIRI_SOCKET has no file name"))?
        .to_string_lossy();
    let instance = name.strip_suffix(".sock").unwrap_or(&name);
    Ok(run.join("niri-computer-use").join(instance))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_runtime_dir_is_named_after_the_nested_instance() {
        assert_eq!(
            runtime_dir(
                Path::new("/t/run"),
                Path::new("/t/run/niri.wayland-2.99.sock")
            )
            .unwrap(),
            Path::new("/t/run/niri-computer-use/niri.wayland-2.99")
        );
    }

    #[test]
    fn requests_quote_safely_for_sh() {
        let lines = requests("harness-a", "acquire_desktop");
        assert!(lines.iter().all(|line| !line.contains('\'')));
        assert!(printf(&lines).starts_with("printf '%s\\n' '{\"id\":1"));
    }
}
