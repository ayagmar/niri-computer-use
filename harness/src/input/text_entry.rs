//! Native and wtype text in a real GTK 4 entry, read back from the application: ASCII,
//! the C10 corpus and a submitted line, then `paste` (see `paste`). Optional dev
//! fixture, like A05's button.

use std::fs;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::{SERVER_DEADLINE, WAIT, stop};
use crate::failure::{Context as _, Failure, Result};
use crate::keyboard::CORPUS;
use crate::mcp::{Client, field, structured};
use crate::session::Session;

pub(super) const APP_ID: &str = "org.ncu.Entry";
const ASCII: &str = "Hello, world! 0123456789 ~`{}|<>?";
const SUBMITTED: &str = "Sent from the entry: café → 5 €";
const ROUNDS: usize = 3;

/// Tool start to the application's report, per case.
#[derive(Debug, Default)]
struct Times {
    ascii: Vec<Duration>,
    corpus: Vec<Duration>,
    submit: Vec<Duration>,
}

pub(super) fn run(session: &mut Session<'_>, owner: &mut Client, server: &str) -> Result<()> {
    let Some(process) = super::activation::start_fixture(session, "entry", "M7 GTK text entry")?
    else {
        return Ok(());
    };
    structured(&owner.call(session, "acquire_desktop", json!({}))?)?;
    session.wait_until(
        "m7-entry-ready",
        "the focused GTK entry",
        Duration::from_secs(15),
        |session| {
            let desktop = structured(&owner.call(session, "desktop_state", json!({}))?)?;
            Ok(focused_app(&desktop).filter(|app| app == APP_ID).map(drop))
        },
    )?;
    let wtype = rounds(session, owner, "wtype")?;
    super::paste::empty(session, owner, "wtype")?;
    super::paste::run(session, owner, server, "wtype")?;
    super::paste::secret(session, owner)?;
    structured(&owner.call(session, "release_desktop", json!({"restore_focus": false}))?)?;
    let mut native = Client::start_command(
        session,
        "env",
        &[
            "NIRI_COMPUTER_USE_KEYBOARD=native".into(),
            server.into(),
            "serve".into(),
        ],
        "harness-m7-entry",
        SERVER_DEADLINE,
    )?;
    structured(&native.call(session, "acquire_desktop", json!({}))?)?;
    let native_times = rounds(session, &mut native, "native")?;
    super::paste::run(session, &mut native, server, "native")?;
    structured(&native.call(session, "release_desktop", json!({"restore_focus": false}))?)?;
    native.stop()?;
    process.stop()?;
    for (backend, times) in [("wtype", wtype), ("native", native_times)] {
        session.log(&format!(
            "M7 GTK entry {backend} tool-to-application ms (min/median/max of {ROUNDS}): ASCII {}, C10 corpus {}, submit {}",
            summary(times.ascii),
            summary(times.corpus),
            summary(times.submit)
        ))?;
    }
    Ok(())
}

fn focused_app(desktop: &Value) -> Option<String> {
    let focused = field(desktop, "/focused_window").as_u64()?;
    field(desktop, "/windows")
        .as_array()?
        .iter()
        .find(|window| field(window, "/id").as_u64() == Some(focused))
        .and_then(|window| field(window, "/app_id").as_str().map(str::to_owned))
}

fn rounds(session: &mut Session<'_>, client: &mut Client, backend: &str) -> Result<Times> {
    let mut times = Times::default();
    for _ in 0..ROUNDS {
        times.ascii.push(typed(session, client, ASCII)?);
        clear(session, client)?;
        times.corpus.push(typed(session, client, CORPUS)?);
        clear(session, client)?;
        times.submit.push(submitted(session, client)?);
        stop::marker_gone(session)?;
    }
    session.log(&format!("M7 GTK entry {backend}: ASCII, the C10 corpus and a submitted line read back exactly from the entry, {ROUNDS} times"))?;
    Ok(times)
}

fn typed(session: &mut Session<'_>, client: &mut Client, text: &str) -> Result<Duration> {
    let start = Instant::now();
    call(session, client, "type_text", json!({"text": text}))?;
    entry_shows(session, text)?;
    Ok(start.elapsed())
}

/// `ctrl+a`, then `BackSpace`, empties the entry.
pub(super) fn clear(session: &mut Session<'_>, client: &mut Client) -> Result<()> {
    call(
        session,
        client,
        "key",
        json!({"keys": ["ctrl+a", "BackSpace"]}),
    )?;
    entry_shows(session, "")
}

/// The entry's activation must report the whole line once, after all of it went in.
fn submitted(session: &mut Session<'_>, client: &mut Client) -> Result<Duration> {
    let before = submits(session)?.0;
    let start = Instant::now();
    call(
        session,
        client,
        "type_text",
        json!({"text": SUBMITTED, "submit": true}),
    )?;
    let (count, text) = session.wait_until(
        "m7-entry-submit",
        "the entry's activation",
        WAIT,
        |session| {
            let seen = submits(session)?;
            Ok((seen.0 != before).then_some(seen))
        },
    )?;
    let elapsed = start.elapsed();
    if count != before + 1 || text != SUBMITTED {
        return Err(Failure::new(format!(
            "M7 GTK entry submitted {count} after {before}: {text:?}"
        )));
    }
    entry_shows(session, "")?;
    Ok(elapsed)
}

fn call(session: &mut Session<'_>, client: &mut Client, tool: &str, mut args: Value) -> Result<()> {
    if let Some(args) = args.as_object_mut() {
        args.insert("expect".into(), json!({"app_id": APP_ID}));
    }
    let result = structured(&client.call(session, tool, args)?)?;
    if field(&result, "/observed") != "sent" || field(&result, "/focus") != "matched" {
        return Err(Failure::new(format!("M7 GTK entry {tool}: {result}")));
    }
    Ok(())
}

pub(super) fn entry_shows(session: &mut Session<'_>, expected: &str) -> Result<()> {
    session.wait_until("m7-entry-text", "the entry's text", WAIT, |session| {
        let path = session.test_dir().root().join("entry-text");
        let text = fs::read_to_string(&path).context(format!("read {}", path.display()))?;
        Ok((text == expected).then_some(()))
    })
}

fn submits(session: &Session<'_>) -> Result<(u64, String)> {
    let path = session.test_dir().root().join("entry-submits");
    let report: Value = serde_json::from_str(
        &fs::read_to_string(&path).context(format!("read {}", path.display()))?,
    )
    .context("parse the entry's submissions")?;
    let count = field(&report, "/count")
        .as_u64()
        .ok_or_else(|| Failure::new("entry submissions lack a count"))?;
    let text = field(&report, "/text")
        .as_str()
        .ok_or_else(|| Failure::new("entry submissions lack the text"))?;
    Ok((count, text.to_owned()))
}

fn summary(mut times: Vec<Duration>) -> String {
    times.sort_unstable();
    let ms = |time: Option<&Duration>| time.map_or(f64::NAN, |time| time.as_secs_f64() * 1000.0);
    format!(
        "{:.3}/{:.3}/{:.3}",
        ms(times.first()),
        ms(times.get(times.len() / 2)),
        ms(times.last())
    )
}
