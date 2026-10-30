//! `--upgrade current`: the build under test installed over itself, as a reinstall of the
//! same release writes a new file. Its engine, idle, hands over to the first client of the
//! new file, in a scope of its own, and its old clients get `engine_lost` once, logged,
//! then the advice to restart them.

use std::time::Duration;

use serde_json::{Value, json};

use super::install::{CURRENT, Place};
use super::{audit, client, ended, expect, first_line};
use crate::failure::{Failure, Result};
use crate::mcp::{Client, field, structured};
use crate::session::Session;

/// How long the old engine may take to exit once the new client is served: it has let go
/// of `engine.lock` by then.
const EXIT: Duration = Duration::from_secs(2);

pub(super) fn run(session: &mut Session<'_>, place: &Place) -> Result<()> {
    let printed = place.install(session, CURRENT)?;
    session.log(&format!(
        "upgrade: installed {CURRENT}: {}",
        first_line(&printed)
    ))?;
    let mut old = [
        client(session, place, "reinstall-old-a")?,
        client(session, place, "reinstall-old-b")?,
    ];
    let engine = shared_engine(session, &mut old[0])?;
    let [a, b] = &mut old;
    let second = shared_engine(session, b)?;
    expect(
        field(&engine, "/pid") == field(&second, "/pid"),
        "both clients on one engine",
        &format!("{engine} {second}"),
    )?;
    let pid = field(&engine, "/pid").as_u64().unwrap_or_default();
    place.install(session, CURRENT)?;
    let replaced = structured(&a.call(session, "status", json!({}))?)?;
    expect(
        field(&replaced, "/engine/replaced") == true,
        "the old engine's file replaced by the reinstall",
        &replaced.to_string(),
    )?;
    let mut new = client(session, place, "reinstall-new")?;
    handed_over(session, &mut new, pid)?;
    for client in &mut old {
        told_to_restart(session, client)?;
    }
    lost_logged(session)?;
    for client in old.into_iter().chain([new]) {
        client.stop()?;
    }
    crate::shared::no_engine_left(session, "reinstall-engines-exit")
}

/// The shared engine `client` is on, as its `status` names it.
fn shared_engine(session: &mut Session<'_>, client: &mut Client) -> Result<Value> {
    let status = structured(&client.call(session, "status", json!({}))?)?;
    let engine = field(&status, "/engine").clone();
    expect(
        field(&engine, "/mode") == "shared" && field(&engine, "/pid").is_u64(),
        "a client on a shared engine",
        &status.to_string(),
    )?;
    Ok(engine)
}

/// The new client is on a new shared engine, in a scope of its own as the first engine
/// was, and the old engine, `old`, has exited.
fn handed_over(session: &mut Session<'_>, new: &mut Client, old: u64) -> Result<()> {
    let engine = shared_engine(session, new)?;
    session.log(&format!(
        "upgrade: after the reinstall, the new client is on {engine}"
    ))?;
    expect(
        field(&engine, "/pid") != old
            && field(&engine, "/fallback").is_null()
            && field(&engine, "/unscoped").is_null()
            && field(&engine, "/replaced") == false,
        "the new client on a new engine, in a scope of its own",
        &engine.to_string(),
    )?;
    let old = u32::try_from(old).map_err(|_| Failure::new("an engine PID over u32"))?;
    session.wait_until(
        "reinstall-old-engine-exit",
        "the old engine's exit",
        EXIT,
        |_| Ok(ended(old).then_some(())),
    )
}

/// An old client's first call gets `engine_lost`, and its next one the advice to restart.
fn told_to_restart(session: &mut Session<'_>, client: &mut Client) -> Result<()> {
    let lost = client.call(session, "status", json!({}))?;
    expect(
        field(&lost, "/structuredContent/error") == "engine_lost",
        "an old client's engine_lost",
        &lost.to_string(),
    )?;
    let met = client.call(session, "status", json!({}))?;
    session.log(&format!(
        "upgrade: an old client after the hand-over: {}",
        field(&met, "/structuredContent")
    ))?;
    expect(
        field(&met, "/structuredContent/error") == "engine_unavailable"
            && field(&met, "/structuredContent/detail")
                .as_str()
                .is_some_and(|detail| detail.contains("restart this MCP client")),
        "the old client told to restart",
        &met.to_string(),
    )
}

/// Both old clients' `engine_lost`, answered by their bridges, are in the audit log.
fn lost_logged(session: &mut Session<'_>) -> Result<()> {
    let lines = audit(session)?;
    let lost = lines
        .iter()
        .filter(|line| field(line, "/error") == "engine_lost" && field(line, "/tool") == "status")
        .count();
    expect(
        lost == 2,
        "two engine_lost lines in the audit log",
        &lost.to_string(),
    )?;
    session.log("upgrade: the audit log has both old clients' engine_lost")
}
