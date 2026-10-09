//! Actual GTK button activation, not just receipt of pointer events. Optional dev fixture.

use std::ffi::OsString;
use std::fs;
use std::time::{Duration, Instant};

use serde_json::json;

use super::{Shot, send};
use crate::failure::{Context as _, Failure, Result};
use crate::mcp::{Client, field, structured};
use crate::session::Session;

const TRIALS: u32 = 100;

pub(super) fn run(session: &mut Session<'_>, client: &mut Client) -> Result<()> {
    let probe = [
        "-I",
        "-c",
        "import gi; gi.require_version('Gtk', '4.0'); from gi.repository import Gtk",
    ]
    .map(OsString::from);
    if let Err(error) = session.run("python3", &probe) {
        return session.log(&format!(
            "A05 GTK activation: skipped (Python GTK 4 unavailable: {error})"
        ));
    }
    let fixture = session.test_dir().root().join("button.py");
    fs::write(&fixture, include_str!("../../fixtures/button.py")).context("write GTK fixture")?;
    let args = vec![
        "-I".into(),
        fixture.into(),
        session.test_dir().root().into(),
    ];
    let process = session.start(
        "python3",
        &args,
        session.artifact("button.log"),
        Duration::from_secs(45),
    )?;
    structured(&client.call(session, "acquire_desktop", json!({}))?)?;
    let centre = session.wait_until(
        "gtk-ready",
        "GTK button window",
        Duration::from_secs(15),
        |session| {
            let desktop = structured(&client.call(session, "desktop_state", json!({}))?)?;
            Ok(field(&desktop, "/windows")
                .as_array()
                .into_iter()
                .flatten()
                .find(|window| field(window, "/app_id") == "org.ncu.Activation")
                .and_then(super::scrolling::centre))
        },
    )?;
    let shot = Shot::take(session, client, 4000)?;
    let pixel = shot.pixel(centre);
    let mut times = Vec::new();
    for expected in 1..=TRIALS {
        let start = Instant::now();
        send(
            session,
            client,
            "click",
            json!({"screenshot_ref": shot.id, "x": pixel.0, "y": pixel.1}),
        )?;
        session.wait_until(
            "gtk-click",
            "GTK activation",
            Duration::from_secs(2),
            |session| {
                let count = fs::read_to_string(session.test_dir().root().join("activations"))
                    .context("read GTK activation count")?;
                let actual = count.parse::<u32>().ok();
                if actual.is_some_and(|actual| actual > expected) {
                    return Err(Failure::new(
                        "GTK button activated more than once per click",
                    ));
                }
                Ok((actual == Some(expected)).then_some(()))
            },
        )?;
        times.push(start.elapsed().as_micros());
    }
    times.sort_unstable();
    let sample = |index| {
        times
            .get(index)
            .ok_or_else(|| Failure::new("missing GTK timing sample"))
    };
    session.log(&format!("A05 GTK button: {TRIALS}/{TRIALS} actual activations; tool-to-observed-counter us p50={} p95={} p99={}", sample(49)?, sample(94)?, sample(98)?))?;
    structured(&client.call(session, "release_desktop", json!({"restore_focus": false}))?)?;
    process.stop().map(drop)
}
