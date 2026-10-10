//! The crash guardian after a SIGKILL: without `recover`, the input the killed server's
//! marker names must be released, and the marker must stay, noting when, until `recover`.

use std::fs;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use super::WAIT;
use crate::failure::{Context as _, Failure, Result};
use crate::mcp::{Client, field, structured};
use crate::runner;
use crate::session::Session;

const MS_PER_DAY: i64 = 86_400_000;

/// When a server was killed: monotonic for polling, wall clock to compare with the time
/// the guardian wrote into the marker.
#[derive(Debug, Clone, Copy)]
pub(super) struct Killed {
    at: Instant,
    wall: SystemTime,
}

impl Killed {
    fn now() -> Self {
        Self {
            at: Instant::now(),
            wall: SystemTime::now(),
        }
    }
}

/// SIGKILLs the process the guardian watches: the server, or in shared mode the engine
/// its `status` names. Then the client's server or bridge goes too.
pub(super) fn kill(session: &mut Session<'_>, mut client: Client) -> Result<Killed> {
    let status = structured(&client.call(session, "status", json!({}))?)?;
    let engine = field(&status, "/engine");
    if field(engine, "/mode") != "shared" {
        let killed = Killed::now();
        client.stop()?;
        return Ok(killed);
    }
    let pid = field(engine, "/pid")
        .as_u64()
        .ok_or_else(|| Failure::new(format!("status names no engine: {status}")))?;
    let killed = kill_engine(pid)?;
    client.stop()?;
    Ok(killed)
}

/// SIGKILLs the shared engine `pid`.
pub(super) fn kill_engine(pid: u64) -> Result<Killed> {
    let killed = Killed::now();
    runner::kill_pid(u32::try_from(pid).context("the engine's PID")?)?;
    Ok(killed)
}

/// After the shared engine was killed, `client`'s session went with it: its next call
/// gets `engine_lost`, and the one after reaches a new engine. A standalone server's
/// client has nothing to report.
pub(super) fn reconnect(session: &mut Session<'_>, client: &mut Client) -> Result<()> {
    let first = client.call(session, "status", json!({}))?;
    if field(&first, "/structuredContent/error") == "engine_lost" {
        return structured(&client.call(session, "status", json!({}))?).map(drop);
    }
    let status = structured(&first)?;
    if field(&status, "/engine/mode") == "shared" {
        return Err(Failure::new(format!(
            "a client of the killed engine didn't report it: {status}"
        )));
    }
    Ok(())
}

/// Waits for the guardian's note in the marker and for `seen` to find the releases in the
/// client, then logs both times from the kill. `seen` is polled every 50 ms.
pub(super) fn released(
    session: &mut Session<'_>,
    killed: Killed,
    what: &str,
    mut seen: impl FnMut() -> Result<bool>,
) -> Result<()> {
    let path = session.control_dir()?.join("input-dirty");
    let noted = session.wait_until(
        "m7-guardian",
        "the guardian's releases noted in the marker",
        WAIT,
        |_| {
            let text = fs::read_to_string(&path).context("read the marker")?;
            let marker: Value = serde_json::from_str(&text).context("parse the marker")?;
            Ok(marker
                .get("released")
                .and_then(Value::as_str)
                .map(str::to_owned))
        },
    )?;
    session.wait_until(
        "m7-guardian-seen",
        "the releases in the client",
        WAIT,
        |_| Ok(seen()?.then_some(())),
    )?;
    let observed = killed.at.elapsed();
    let noted_ms = ms_of_day(&noted)?;
    let killed_ms = i64::try_from(
        killed
            .wall
            .duration_since(UNIX_EPOCH)
            .context("the clock")?
            .as_millis(),
    )
    .context("the clock")?
        % MS_PER_DAY;
    session.log(&format!(
        "M7 guardian, {what}: releases acknowledged by niri {} ms after the kill (marker time, ms resolution); seen in the client within {:.0} ms (50 ms polling); marker kept until recover",
        (noted_ms - killed_ms).rem_euclid(MS_PER_DAY),
        observed.as_secs_f64() * 1000.0
    ))
}

/// Milliseconds since midnight UTC of a marker time such as `2026-10-10T01:13:15.123Z`.
fn ms_of_day(time: &str) -> Result<i64> {
    let clock = time
        .split_once('T')
        .and_then(|(_, clock)| clock.strip_suffix('Z'))
        .ok_or_else(|| Failure::new(format!("unexpected marker time {time}")))?;
    let (hms, millis) = clock
        .split_once('.')
        .ok_or_else(|| Failure::new(format!("unexpected marker time {time}")))?;
    let mut total: i64 = 0;
    for part in hms.split(':') {
        total = total * 60 + part.parse::<i64>().context("marker time")?;
    }
    Ok(total * 1000 + millis.parse::<i64>().context("marker time")?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_time_of_day_from_a_marker_time() {
        assert_eq!(
            ms_of_day("2026-10-10T01:13:15.123Z").unwrap(),
            ((60 + 13) * 60 + 15) * 1000 + 123
        );
        assert!(ms_of_day("2026-10-10 01:13:15").is_err());
    }
}
