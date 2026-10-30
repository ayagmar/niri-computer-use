//! `--compat`: what one server does on the nested niri, whatever niri's version, for
//! `harness matrix`. With the GTK 4 fixture's window open, drawn by Cairo, it reads
//! `status`, calls the five observation tools, calls every other tool the server lists
//! without holding the lease, and then takes the lease. The contract: the observation tools answer,
//! `release_desktop` answers, every other tool is refused with `lease_required`, and
//! `acquire_desktop` succeeds when `status` says `ok` or `patch_warning` and is refused
//! with `read_only` when it says `read_only`. Each call's outcome goes to `compat.json` in
//! the artifacts, also when a call goes otherwise.

use std::fs;
use std::time::Duration;

use niri_ipc::Window;
use serde_json::{Value, json};

use crate::a11y::{Bus, Fixture};
use crate::failure::{Context as _, Failure, Result};
use crate::mcp::{self, Client, field};
use crate::session::Session;

/// The probe's outcome in the artifacts directory.
pub(crate) const REPORT: &str = "compat.json";
/// The tools that only look, which every niri version must answer.
pub(crate) const OBSERVING: [&str; 5] = [
    "status",
    "desktop_state",
    "screenshot",
    "elements",
    "wait_for",
];
/// Within what is left of the run's deadline.
const NOCTALIA_DEADLINE: Duration = Duration::from_secs(110);
const SERVER_DEADLINE: Duration = Duration::from_secs(100);
/// How long `status` may take to see niri's version, Noctalia and an unlocked screen.
const READY: Duration = Duration::from_secs(20);
const WAIT_FOR_MS: u64 = 2000;

pub(crate) fn run(session: &mut Session<'_>, server: &str) -> Result<()> {
    let bus = Bus::start(session)?;
    let noctalia = session.start(
        "noctalia",
        &[],
        session.artifact("noctalia.log"),
        NOCTALIA_DEADLINE,
    )?;
    let fixture = Fixture::start_cairo(session)?;
    let window = fixture.window(session, &bus)?;
    let mut client = Client::start(session, server, "harness-compat", SERVER_DEADLINE)?;
    let report = probe(session, &mut client, &window);
    let written = report.as_ref().map_or(Ok(()), |report| {
        let path = session.artifact(REPORT);
        fs::write(&path, report.to_string()).context(format!("write {}", path.display()))
    });
    client.stop()?;
    fixture.stop()?;
    noctalia.stop()?;
    bus.stop()?;
    let report = report?;
    written?;
    session.log(&format!("compat: {report}"))?;
    held(&report)
}

/// The calls in order, each with what it gave and what the contract expects.
fn probe(session: &mut Session<'_>, client: &mut Client, window: &Window) -> Result<Value> {
    let status = settled(session, client)?;
    let mut calls = observe(session, client, window)?;
    let shot = calls
        .iter()
        .find_map(|call| field(call, "/screenshot_ref").as_str().map(str::to_owned))
        .unwrap_or_default();
    for tool in client.tools(session)? {
        if OBSERVING.contains(&tool.as_str()) || tool == "acquire_desktop" {
            continue;
        }
        let expected = if tool == "release_desktop" {
            "ok"
        } else {
            "lease_required"
        };
        let Some(arguments) = arguments(&tool, window, &shot) else {
            calls.push(json!({"tool": tool, "expected": expected, "got": "untried",
                "detail": "the probe has no arguments for this tool"}));
            continue;
        };
        calls.push(call(session, client, &tool, arguments, expected)?);
    }
    // After the screenshot, whose capture niri may report as an event.
    let after = mcp::report(&client.call(session, "status", json!({}))?);
    let expected = match field(&after, "/niri/compat").as_str() {
        Some("ok" | "patch_warning") => "ok",
        Some("read_only") => "read_only",
        _ => "a known niri version",
    };
    calls.push(call(
        session,
        client,
        "acquire_desktop",
        json!({}),
        expected,
    )?);
    if calls.last().is_some_and(|call| field(call, "/got") == "ok") {
        client.call(session, "release_desktop", json!({"restore_focus": false}))?;
    }
    Ok(json!({
        "version": field(&status, "/niri/version"),
        "compat": field(&after, "/niri/compat"),
        "event_stream": field(&after, "/niri/event_stream"),
        "niri_error": field(&after, "/niri/error"),
        "lock": field(&status, "/lock/state"),
        "calls": calls,
    }))
}

/// `status` once it sees niri's version, Noctalia running and the screen unlocked, or as it
/// is when that doesn't come: on a niri the server doesn't support, the lock state is not
/// part of the contract.
fn settled(session: &mut Session<'_>, client: &mut Client) -> Result<Value> {
    let ready = session.wait_until(
        "compat-status",
        "status with niri's version, Noctalia running and the screen unlocked",
        READY,
        |session| {
            let status = mcp::report(&client.call(session, "status", json!({}))?);
            let ready = field(&status, "/niri/version").is_string()
                && field(&status, "/noctalia") == "running"
                && field(&status, "/lock/state") == "unlocked";
            Ok(ready.then_some(status))
        },
    );
    match ready {
        Ok(status) => Ok(status),
        Err(failure) => {
            session.log(&format!("compat: {failure}; going on with status as it is"))?;
            Ok(mcp::report(&client.call(session, "status", json!({}))?))
        }
    }
}

/// The observation tools on the fixture's window, each expected to answer.
fn observe(session: &mut Session<'_>, client: &mut Client, window: &Window) -> Result<Vec<Value>> {
    let app_id = window.app_id.clone().unwrap_or_default();
    let calls = [
        ("status", json!({})),
        ("desktop_state", json!({})),
        ("screenshot", json!({"target": "focused_output"})),
        ("elements", json!({"window_id": window.id})),
        (
            "wait_for",
            json!({"until": {"window": {"app_id": app_id}}, "timeout_ms": WAIT_FOR_MS}),
        ),
    ];
    let mut done = Vec::new();
    for (tool, arguments) in calls {
        done.push(call(session, client, tool, arguments, "ok")?);
    }
    Ok(done)
}

/// Arguments the tool accepts, so that only the lease can refuse it.
fn arguments(tool: &str, window: &Window, shot: &str) -> Option<Value> {
    let expect = json!({"window_id": window.id});
    let at = |x: u32| json!({"x": x, "y": x});
    Some(match tool {
        "release_desktop" => json!({"restore_focus": false}),
        "focus" | "close_window" => json!({"window_id": window.id}),
        "launch" => json!({"preset": "compat"}),
        "niri_action" => json!({"action": {"FocusWorkspaceDown": {}}}),
        "pointer_move" | "click" => json!({"screenshot_ref": shot, "x": 10, "y": 10}),
        "drag" => json!({"screenshot_ref": shot, "from": at(10), "to": at(20)}),
        "scroll" => json!({"screenshot_ref": shot, "x": 10, "y": 10, "notches_y": 1}),
        "key" => json!({"keys": ["Escape"], "expect": expect}),
        "type_text" => json!({"text": "x", "expect": expect}),
        "activate_element" => json!({"element": "e1", "expect": expect}),
        "set_element_text" => json!({"element": "e1", "text": "x", "expect": expect}),
        "shell_panel" => json!({"panel": "launcher", "open": true}),
        "noctalia" => json!({"args": ["status"]}),
        "clipboard_read" => json!({}),
        _ => return None,
    })
}

/// One call's outcome: `ok`, or the error name it was refused with.
fn call(
    session: &mut Session<'_>,
    client: &mut Client,
    tool: &str,
    arguments: Value,
    expected: &str,
) -> Result<Value> {
    let result = client.call(session, tool, arguments)?;
    let report = mcp::report(&result);
    let (got, detail) = if field(&result, "/isError") == true {
        field(&report, "/error").as_str().map_or_else(
            || ("plain_error".to_owned(), field(&result, "/content").clone()),
            |name| (name.to_owned(), field(&report, "/detail").clone()),
        )
    } else {
        ("ok".to_owned(), Value::Null)
    };
    Ok(json!({
        "tool": tool, "expected": expected, "got": got, "detail": detail,
        "screenshot_ref": field(&report, "/screenshot_ref"),
    }))
}

/// Whether every call went as the contract says.
pub(crate) fn held(report: &Value) -> Result<()> {
    let missed: Vec<String> = misses(report);
    if missed.is_empty() {
        Ok(())
    } else {
        Err(Failure::new(format!("compat: {}", missed.join("; "))))
    }
}

/// `tool: expected …, got …` for each call that went otherwise.
pub(crate) fn misses(report: &Value) -> Vec<String> {
    field(report, "/calls")
        .as_array()
        .into_iter()
        .flatten()
        .filter(|call| field(call, "/expected") != field(call, "/got"))
        .map(|call| {
            let detail = field(call, "/detail");
            let detail = match detail.as_str() {
                Some(text) => format!(" ({text})"),
                None if detail.is_null() => String::new(),
                None => format!(" ({detail})"),
            };
            format!(
                "{}: expected {}, got {}{detail}",
                field(call, "/tool").as_str().unwrap_or_default(),
                field(call, "/expected").as_str().unwrap_or_default(),
                field(call, "/got").as_str().unwrap_or_default(),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_call_that_went_otherwise_is_a_miss_with_its_detail() {
        let report = json!({"calls": [
            {"tool": "status", "expected": "ok", "got": "ok", "detail": null},
            {"tool": "acquire_desktop", "expected": "read_only", "got": "ok", "detail": null},
            {"tool": "click", "expected": "lease_required", "got": "untried",
                "detail": "the probe has no arguments for this tool"},
        ]});
        assert_eq!(
            misses(&report),
            [
                "acquire_desktop: expected read_only, got ok",
                "click: expected lease_required, got untried (the probe has no arguments for this tool)",
            ]
        );
        assert!(held(&report).is_err());
        assert!(held(&json!({"calls": []})).is_ok());
    }
}
