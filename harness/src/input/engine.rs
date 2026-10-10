//! The shared engine against the nested niri (plan §6): ten clients on one engine (E1), a
//! client killed mid-drag (E3), a stop during typing while nine clients keep calling (E4),
//! and the engine killed mid-drag (E5). Every server here bridges to the engine.

use std::fs;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::{SERVER_DEADLINE, Shot, WAIT, Wev};
use crate::failure::{Failure, Result};
use crate::keyboard;
use crate::mcp::{Client, field, structured};
use crate::session::Session;
use crate::wev::{self, Pointer};

const CLIENTS: usize = 10;
/// How soon a session's end or a stop must free the lease.
const FREED: Duration = Duration::from_secs(1);
const PRESS: Pointer = Pointer::Button {
    code: 272,
    pressed: true,
};
const RELEASE: Pointer = Pointer::Button {
    code: 272,
    pressed: false,
};

pub(super) fn run(
    session: &mut Session<'_>,
    clients: &mut Vec<Client>,
    wev: &Wev<'_>,
    server: &str,
) -> Result<()> {
    let engine = one_engine(session, clients)?;
    killed_mid_drag(session, clients, wev)?;
    stop_under_load(session, clients, wev, server)?;
    engine_killed_mid_drag(session, clients, wev, server, engine)
}

/// Starts the ten clients, the first one ready.
pub(super) fn start(session: &mut Session<'_>, server: &str) -> Result<Vec<Client>> {
    let mut clients = Vec::new();
    for n in 0..CLIENTS {
        let name = format!("harness-e{n}");
        clients.push(Client::start(session, server, &name, SERVER_DEADLINE)?);
    }
    Ok(clients)
}

/// E1: every client names the same engine, which serves all ten with one guardian and
/// one connected event stream. Returns the engine's PID.
fn one_engine(session: &mut Session<'_>, clients: &mut [Client]) -> Result<u64> {
    let mut engines = Vec::new();
    for client in clients.iter_mut() {
        let status = structured(&client.call(session, "status", json!({}))?)?;
        let engine = field(&status, "/engine");
        let connected = field(&status, "/niri/event_stream") == "connected";
        if field(engine, "/mode") != "shared" || field(engine, "/sessions") != 10 || !connected {
            return Err(Failure::new(format!("E1: a client's status: {status}")));
        }
        engines.push(engine_pid(&status)?);
    }
    engines.dedup();
    let [engine] = engines.as_slice() else {
        return Err(Failure::new(format!(
            "E1: the clients name engines {engines:?}"
        )));
    };
    let guardians = guardians(*engine)?;
    if guardians != 1 {
        return Err(Failure::new(format!(
            "E1: {guardians} guardians watch the engine {engine}"
        )));
    }
    session.log(&format!(
        "E1: {CLIENTS} clients, one engine (PID {engine}) serving 10 sessions, one guardian, event stream connected"
    ))?;
    Ok(*engine)
}

fn engine_pid(status: &Value) -> Result<u64> {
    field(status, "/engine/pid")
        .as_u64()
        .ok_or_else(|| Failure::new(format!("no engine PID in {status}")))
}

/// How many processes run `guard <engine>`.
fn guardians(engine: u64) -> Result<usize> {
    let mut found = 0;
    for entry in
        fs::read_dir("/proc").map_err(|error| Failure::new(format!("list /proc: {error}")))?
    {
        let Ok(entry) = entry else { continue };
        // A process can exit between the listing and this read.
        let Ok(cmdline) = fs::read(entry.path().join("cmdline")) else {
            continue;
        };
        let args: Vec<&[u8]> = cmdline.split(|byte| *byte == 0).collect();
        if args.get(1) == Some(&b"guard".as_slice())
            && args.get(2) == Some(&engine.to_string().as_bytes())
        {
            found += 1;
        }
    }
    Ok(found)
}

/// `client` takes the lease and starts a drag across `wev`. Returns the
/// call's id once `wev` has seen the press, and the log offset before it.
fn drag(session: &mut Session<'_>, client: &mut Client, wev: &Wev<'_>) -> Result<(u64, usize)> {
    structured(&client.call(session, "acquire_desktop", json!({}))?)?;
    let shot = Shot::take(session, client, 4000)?;
    let from = shot.pixel(wev.in_layout((50.0, 50.0)));
    let to = shot.pixel(wev.in_layout((350.0, 250.0)));
    let offset = wev.offset()?;
    let id = client.start_call(
        "drag",
        json!({
            "screenshot_ref": shot.id,
            "from": {"x": from.0, "y": from.1},
            "to": {"x": to.0, "y": to.1}
        }),
    )?;
    session.wait_until("e-drag-press", "the drag's press in wev", WAIT, |_| {
        Ok(wev.since(offset)?.contains(&PRESS).then_some(()))
    })?;
    Ok((id, offset))
}

/// Waits until `client` can take the lease, and gives it back. Returns how long it took.
fn lease_free(session: &mut Session<'_>, client: &mut Client, since: Instant) -> Result<Duration> {
    session.wait_until(
        "e-lease",
        "the lease free for another client",
        WAIT,
        |session| {
            let result = client.call(session, "acquire_desktop", json!({}))?;
            Ok(structured(&result).ok().map(drop))
        },
    )?;
    let took = since.elapsed();
    structured(&client.call(session, "release_desktop", json!({"restore_focus": false}))?)?;
    if took > FREED {
        return Err(Failure::new(format!(
            "the lease was free only after {took:?}"
        )));
    }
    Ok(took)
}

/// E3: A's bridge is killed mid-drag. The engine ends A's session, releases the button
/// and frees the lease, while B's calls keep working.
fn killed_mid_drag(
    session: &mut Session<'_>,
    clients: &mut Vec<Client>,
    wev: &Wev<'_>,
) -> Result<()> {
    let mut a = clients
        .pop()
        .ok_or_else(|| Failure::new("E3: no client left"))?;
    let (_, offset) = drag(session, &mut a, wev)?;
    let killed = Instant::now();
    a.stop()?;
    let b = clients
        .first_mut()
        .ok_or_else(|| Failure::new("E3: no client left"))?;
    structured(&b.call(session, "status", json!({}))?)?;
    structured(&b.call(session, "desktop_state", json!({}))?)?;
    session.wait_until("e3-release", "the drag's release in wev", WAIT, |_| {
        Ok(wev.since(offset)?.contains(&RELEASE).then_some(()))
    })?;
    let released = killed.elapsed();
    let took = lease_free(session, b, killed)?;
    super::stop::marker_gone(session)?;
    session.log(&format!(
        "E3: A's bridge killed mid-drag; button released after {released:?}, lease free for B after {took:?}, B's status and desktop_state answered meanwhile, marker gone"
    ))
}

/// E4: a stop while A types and the other clients call `status`, `desktop_state` and
/// `screenshot`: A's call says `stopped`, every other call is answered, and the lease is
/// free within a second. wtype still types the whole text.
fn stop_under_load(
    session: &mut Session<'_>,
    clients: &mut [Client],
    wev: &Wev<'_>,
    server: &str,
) -> Result<()> {
    let text = "abcdefghij".repeat(10);
    let (a, load) = clients
        .split_first_mut()
        .ok_or_else(|| Failure::new("E4: no client"))?;
    structured(&a.call(session, "acquire_desktop", json!({}))?)?;
    let offset = wev.offset()?;
    let id = a.start_call(
        "type_text",
        json!({"text": text, "expect": {"app_id": "wev"}}),
    )?;
    session.wait_until("e4-typing", "the first key in wev", WAIT, |_| {
        let seen = keyboard::since(wev.log, offset)?;
        Ok((!wev::keyboard::trace(&seen)?.keys.is_empty()).then_some(()))
    })?;
    let mut calls = Vec::new();
    for client in load.iter_mut() {
        for (tool, arguments) in [
            ("status", json!({})),
            ("desktop_state", json!({})),
            (
                "screenshot",
                json!({"target": "focused_output", "max_width": 640}),
            ),
        ] {
            calls.push(client.start_call(tool, arguments)?);
        }
    }
    let stopped = Instant::now();
    session.run(server, &["stop".into()])?;
    let result = a.result(session, id)?;
    let took = stopped.elapsed();
    if field(&result, "/structuredContent/error") != "stopped" {
        return Err(Failure::new(format!(
            "E4: the stop landed after the typing; A's call: {result}"
        )));
    }
    let watcher = load
        .first_mut()
        .ok_or_else(|| Failure::new("E4: no other client"))?;
    let status = structured(&watcher.call(session, "status", json!({}))?)?;
    let free = field(&status, "/lease/holder").is_null();
    let checked = stopped.elapsed();
    if !free || checked > FREED {
        return Err(Failure::new(format!(
            "E4: lease free {free} {checked:?} after the stop"
        )));
    }
    answered(session, load, &calls)?;
    let seen = keyboard::observed(session, wev.log, offset, text.chars().count(), false)?;
    keyboard::text(&wev::keyboard::trace(&seen)?, &text)?;
    super::stop::marker_gone(session)?;
    super::stop::resume(session, a, server)?;
    structured(&a.call(session, "release_desktop", json!({"restore_focus": false}))?)?;
    session.log(&format!(
        "E4: stop during typing with {} calls from {} other clients in flight: A stopped after {took:?}, lease free at {checked:?}, every other call answered, wtype finished the text, resume took the lease again",
        calls.len(),
        load.len()
    ))
}

/// Each of `load`'s clients answered its three calls, in `calls`' order, without error.
fn answered(session: &mut Session<'_>, load: &mut [Client], calls: &[u64]) -> Result<()> {
    for (client, ids) in load.iter_mut().zip(calls.chunks(3)) {
        for id in ids {
            structured(&client.result(session, *id)?)?;
        }
    }
    Ok(())
}

/// E5: the engine is killed with `SIGKILL` mid-drag. A's call gets `engine_lost`, the guardian
/// releases the button, every other client gets `engine_lost` once and then reaches one
/// new engine, and B is refused with `recovery_required` until `recover`.
fn engine_killed_mid_drag(
    session: &mut Session<'_>,
    clients: &mut [Client],
    wev: &Wev<'_>,
    server: &str,
    engine: u64,
) -> Result<()> {
    let (a, rest) = clients
        .split_first_mut()
        .ok_or_else(|| Failure::new("E5: no client"))?;
    let (id, offset) = drag(session, a, wev)?;
    let killed = super::guardian::kill_engine(engine)?;
    let lost = a.result(session, id)?;
    if field(&lost, "/structuredContent/error") != "engine_lost" {
        return Err(Failure::new(format!("E5: A's drag: {lost}")));
    }
    super::guardian::released(session, killed, "E5 drag button", || {
        Ok(wev.since(offset)?.contains(&RELEASE))
    })?;
    let mut engines = Vec::new();
    for client in rest.iter_mut() {
        let first = client.call(session, "status", json!({}))?;
        if field(&first, "/structuredContent/error") != "engine_lost" {
            return Err(Failure::new(format!(
                "E5: an idle client's next call: {first}"
            )));
        }
        engines.push(engine_pid(&structured(&client.call(
            session,
            "status",
            json!({}),
        )?)?)?);
    }
    engines.dedup();
    let [new] = engines.as_slice() else {
        return Err(Failure::new(format!(
            "E5: the clients reached engines {engines:?}"
        )));
    };
    if *new == engine {
        return Err(Failure::new("E5: the killed engine still answers"));
    }
    let b = rest
        .first_mut()
        .ok_or_else(|| Failure::new("E5: no second client"))?;
    let refused = b.call(session, "acquire_desktop", json!({}))?;
    if field(&refused, "/structuredContent/error") != "recovery_required" {
        return Err(Failure::new(format!("E5: B's acquire: {refused}")));
    }
    super::crash::recover(
        session,
        b,
        server,
        "Sent the release of pointer buttons [272]",
    )?;
    structured(&a.call(session, "status", json!({}))?)?;
    session.log(&format!(
        "E5: engine {engine} killed mid-drag: A's drag got engine_lost, the guardian released the button, {} idle clients got engine_lost once and reached engine {new}, B was refused with recovery_required until recover, then took the lease",
        rest.len()
    ))
}
