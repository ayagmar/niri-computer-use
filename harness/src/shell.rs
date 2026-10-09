//! M5's nested acceptance: a `niri-computer-use` server against the nested niri and
//! Noctalia. The nested niri runs without a logind session, so the lock state comes from
//! Noctalia. Each allowlisted panel opens and closes as observed, and the panels outside
//! the allowlist return `panel_not_allowed` without opening anything. With Noctalia
//! stopped, `shell_status` is `noctalia_unavailable` and, with no lock source left, the
//! shell tools are refused as `screen_locked`. A server whose `PATH` has no `noctalia`
//! lists no shell tools and reports Noctalia as not installed.

use std::ffi::OsString;
use std::fs;
use std::time::Duration;

use serde_json::{Value, json};

use crate::failure::{Context as _, Failure, Result};
use crate::mcp::{self, Client, field, structured};
use crate::noctalia;
use crate::session::Session;

/// Within what is left of the run's deadline.
const NOCTALIA_DEADLINE: Duration = Duration::from_secs(110);
const SERVER_DEADLINE: Duration = Duration::from_secs(100);
const READY: Duration = Duration::from_secs(20);
const WAIT: Duration = Duration::from_secs(5);
/// The allowlisted panels that cover enough of the output for the pixel check. The tray
/// drawer is one icon wide while the nested session has no tray items, under the 1% a drawn
/// panel must change, so only its `activePanelId` is checked.
const DRAWN: [&str; 2] = ["control-center", "wallpaper"];
/// The panels plan §6.1 never allows, and one Noctalia doesn't have.
const REFUSED: [&str; 7] = [
    "session",
    "launcher",
    "polkit",
    "clipboard",
    "setup-wizard",
    "test",
    "no-such-panel",
];

pub(crate) fn run(session: &mut Session<'_>, server: &str) -> Result<()> {
    let noctalia = noctalia::start(session, NOCTALIA_DEADLINE)?;
    let mut client = Client::start(session, server, "harness-m5", SERVER_DEADLINE)?;
    lock_from_noctalia(session, &mut client)?;
    noctalia::no_panel_open(session, &mut client)?;
    structured(&client.call(session, "acquire_desktop", json!({}))?)?;
    let closed = noctalia::settled(session)?;
    for panel in DRAWN {
        noctalia::cycle(session, &mut client, panel, Some(&closed))?;
    }
    noctalia::cycle(session, &mut client, "tray-drawer", None)?;
    refused(session, &mut client)?;
    noctalia.stop()?;
    stopped(session, &mut client)?;
    client.stop()?;
    not_installed(session, server)
}

/// The nested niri has no logind session, so Noctalia's `locked` decides.
fn lock_from_noctalia(session: &mut Session<'_>, client: &mut Client) -> Result<()> {
    let status = mcp::ready(session, client, "m5-ready", READY)?;
    let lock = field(&status, "/lock");
    expect(
        field(lock, "/source") == "noctalia" && field(lock, "/logind_error").is_string(),
        "the lock state from Noctalia, with logind's reason for not answering",
        &lock.to_string(),
    )?;
    session.log(&format!("M5: lock from Noctalia: {lock}"))
}

/// Every panel outside the allowlist is refused, and none opened.
fn refused(session: &mut Session<'_>, client: &mut Client) -> Result<()> {
    for panel in REFUSED {
        for tool in ["shell_open", "shell_close"] {
            let result = client.call(session, tool, json!({"panel": panel}))?;
            expect(
                error(&result) == "panel_not_allowed",
                &format!("{tool} {panel} refused"),
                &result.to_string(),
            )?;
        }
    }
    noctalia::no_panel_open(session, client)?;
    session.log(&format!("M5: panel_not_allowed for {}", REFUSED.join(", ")))
}

/// With Noctalia stopped there is neither `shell_status` nor a lock state.
fn stopped(session: &mut Session<'_>, client: &mut Client) -> Result<()> {
    let status = session.wait_until(
        "m5-stopped",
        "status with Noctalia not running",
        WAIT,
        |session| {
            let status = structured(&client.call(session, "status", json!({}))?)?;
            Ok((field(&status, "/noctalia") == "not_running").then_some(status))
        },
    )?;
    let lock = field(&status, "/lock");
    expect(
        field(lock, "/state") == "unknown" && field(lock, "/source") == "none",
        "an unknown lock state without Noctalia",
        &lock.to_string(),
    )?;
    let shell_status = client.call(session, "shell_status", json!({}))?;
    expect(
        error(&shell_status) == "noctalia_unavailable",
        "shell_status without Noctalia",
        &shell_status.to_string(),
    )?;
    let open = client.call(session, "shell_open", json!({"panel": "control-center"}))?;
    expect(
        error(&open) == "screen_locked",
        "shell_open with the lock state unknown",
        &open.to_string(),
    )?;
    session.log(&format!(
        "M5: Noctalia stopped: lock {lock}, shell_status noctalia_unavailable, shell_open screen_locked"
    ))
}

/// A server started with an empty `PATH` directory doesn't find `noctalia`.
fn not_installed(session: &mut Session<'_>, server: &str) -> Result<()> {
    let empty = session.test_dir().root().join("path-without-noctalia");
    fs::create_dir_all(&empty).context(format!("create {}", empty.display()))?;
    let mut path = OsString::from("PATH=");
    path.push(&empty);
    let args = [path, server.into(), "serve".into()];
    let mut client = Client::start_command(session, "env", &args, "harness-m5-no-noctalia", READY)?;
    let tools = client.tools(session)?;
    let status = structured(&client.call(session, "status", json!({}))?)?;
    client.stop()?;
    expect(
        !tools.iter().any(|tool| tool.starts_with("shell_")) && tools.contains(&"status".into()),
        "no shell tools without noctalia on PATH",
        &tools.join(", "),
    )?;
    expect(
        field(&status, "/noctalia") == "not_installed",
        "Noctalia not installed",
        &status.to_string(),
    )?;
    session.log(&format!(
        "M5: without noctalia on PATH: noctalia not_installed, tools {}",
        tools.join(", ")
    ))
}

/// The error name of a failed call, or an empty string.
fn error(result: &Value) -> &str {
    if field(result, "/isError") != true {
        return "";
    }
    field(result, "/structuredContent/error")
        .as_str()
        .unwrap_or_default()
}

fn expect(holds: bool, what: &str, seen: &str) -> Result<()> {
    if holds {
        Ok(())
    } else {
        Err(Failure::new(format!("M5: {what} failed; saw {seen}")))
    }
}
