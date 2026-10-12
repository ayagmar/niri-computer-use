//! `--latency`: each action tool's round trip against the nested niri, for
//! docs/results/action-latency.md. One server holds the lease over the GTK 4 accessibility
//! fixture, with the nested Noctalia as the lock source. Each round times `focus` to the
//! empty workspace and back to the fixture, `click` on the fixture's entry, `type_text` and
//! `key` into it, `activate_element` on its `Primary` button, `niri_action` resizing the
//! fixture, and `shell_panel` opening and closing the control center, none of them with a
//! screenshot. It writes `latency.json` to the artifacts and a summary to the log.

use std::collections::BTreeMap;
use std::fs;
use std::time::{Duration, Instant};

use niri_ipc::{Request, Response};
use serde_json::{Value, json};

use super::{as_millis, median_millis, proc};
use crate::a11y::{Bus, Fixture, Toolkit};
use crate::failure::{Context as _, Failure, Result};
use crate::mcp::{self, Client, field, structured};
use crate::session::Session;

/// Within what is left of the run's deadline, and the fixture's own four minutes.
const NOCTALIA_DEADLINE: Duration = Duration::from_secs(230);
const SERVER_DEADLINE: Duration = Duration::from_secs(225);
const READY: Duration = Duration::from_secs(20);
const ROUNDS: usize = 30;
/// Screenshot refs last 60 s; a new one is taken well before.
const SHOT_AGE: Duration = Duration::from_secs(30);

pub(crate) fn run(session: &mut Session<'_>, server: &str) -> Result<()> {
    let bus = Bus::start(session)?;
    let noctalia = session.start(
        "noctalia",
        &[],
        session.artifact("noctalia.log"),
        NOCTALIA_DEADLINE,
    )?;
    let fixture = Fixture::start(session, Toolkit::Gtk4)?;
    let window = fixture.window(session, &bus)?.id;
    let empty = empty_workspace(session)?;
    let mut client = Client::start(session, server, "harness-latency", SERVER_DEADLINE)?;
    mcp::ready(session, &mut client, "latency-ready", READY)?;
    structured(&client.call(session, "acquire_desktop", json!({}))?)?;
    let entry = named(session, &mut client, window, None, "Plain entry")?;
    let mut timings = Timings::default();
    let mut shot = None;
    for round in 0..ROUNDS {
        let shot = screenshot_ref(session, &mut client, &mut shot)?;
        // The button's name counts its activations, so its ref lasts one.
        let targets = Targets {
            window,
            empty,
            entry: entry.clone(),
            primary: named(session, &mut client, window, Some("button"), "Primary")?,
        };
        for (label, tool, arguments) in targets.round(round, &shot) {
            let (result, took) = client.timed_call(tool, arguments)?;
            let outcome = structured(&result).context(format!("latency: {label}"))?;
            timings.add(label, took, &outcome);
        }
    }
    structured(&client.call(session, "release_desktop", json!({"restore_focus": false}))?)?;
    client.stop()?;
    fixture.stop()?;
    let report = json!({"rounds": ROUNDS, "calls": timings.summary()?});
    let path = session.artifact("latency.json");
    fs::write(&path, format!("{report:#}\n")).context(format!("write {}", path.display()))?;
    session.log(&format!("latency: {report}"))?;
    noctalia.stop()?;
    bus.stop()
}

/// What the rounds act on.
struct Targets {
    window: u64,
    /// The fixture's output's empty workspace, below its own.
    empty: u64,
    entry: String,
    primary: String,
}

impl Targets {
    /// One round's calls: a label, the tool and its arguments.
    fn round(&self, round: usize, shot: &str) -> [(&'static str, &'static str, Value); 9] {
        let expect = json!({"window_id": self.window});
        // Alternating, so that every resize changes the window.
        let height = if round.is_multiple_of(2) { 320 } else { 300 };
        let resize =
            json!({"SetWindowHeight": {"id": self.window, "change": {"SetFixed": height}}});
        let panel = |open| json!({"panel": "control-center", "open": open});
        [
            ("focus_away", "focus", json!({"workspace_id": self.empty})),
            ("focus_back", "focus", json!({"window_id": self.window})),
            (
                "click",
                "click",
                json!({"screenshot_ref": shot, "element": self.entry}),
            ),
            (
                "type_text",
                "type_text",
                json!({"text": "ab", "expect": expect}),
            ),
            (
                "key",
                "key",
                json!({"keys": ["BackSpace", "BackSpace"], "expect": expect}),
            ),
            (
                "activate_element",
                "activate_element",
                json!({"element": self.primary, "expect": expect}),
            ),
            ("niri_action", "niri_action", json!({"action": resize})),
            ("shell_panel_open", "shell_panel", panel(true)),
            ("shell_panel_close", "shell_panel", panel(false)),
        ]
    }
}

/// Each label's round trips, and how often each `observed` came back.
#[derive(Default)]
struct Timings(BTreeMap<&'static str, (Vec<Duration>, BTreeMap<String, usize>)>);

impl Timings {
    fn add(&mut self, label: &'static str, took: Duration, outcome: &Value) {
        let (times, observed) = self.0.entry(label).or_default();
        times.push(took);
        let seen = field(outcome, "/observed").as_str().unwrap_or("none");
        *observed.entry(seen.to_owned()).or_default() += 1;
    }

    /// Each label's median and 95th percentile in milliseconds, and its `observed` counts.
    fn summary(&self) -> Result<Value> {
        let mut summary = serde_json::Map::new();
        for (label, (times, observed)) in &self.0 {
            let p95 = proc::p95(times).ok_or_else(|| Failure::new("no round trips"))?;
            summary.insert(
                (*label).to_owned(),
                json!({"median": median_millis(times)?, "p95": as_millis(p95), "observed": observed}),
            );
        }
        Ok(Value::Object(summary))
    }
}

/// The ref of the one element of `window`, of `role` if given, whose name contains `name`.
fn named(
    session: &mut Session<'_>,
    client: &mut Client,
    window: u64,
    role: Option<&str>,
    name: &str,
) -> Result<String> {
    let mut arguments = serde_json::Map::new();
    arguments.insert("window_id".to_owned(), json!(window));
    arguments.insert("name_contains".to_owned(), json!(name));
    if let Some(role) = role {
        arguments.insert("role".to_owned(), json!(role));
    }
    let arguments = Value::Object(arguments);
    let listing = structured(&client.call(session, "elements", arguments)?)?;
    let found = field(&listing, "/elements").as_array().cloned();
    let [element] = found
        .unwrap_or_default()
        .try_into()
        .map_err(|found: Vec<Value>| {
            Failure::new(format!(
                "latency: expected one element named {name:?}; saw {}",
                found.len()
            ))
        })?;
    field(&element, "/element_ref")
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| Failure::new(format!("latency: no element_ref in {element}")))
}

/// A screenshot ref for the clicks, taken again once it is `SHOT_AGE` old.
fn screenshot_ref(
    session: &mut Session<'_>,
    client: &mut Client,
    shot: &mut Option<(String, Instant)>,
) -> Result<String> {
    if let Some((id, taken)) = shot
        && taken.elapsed() < SHOT_AGE
    {
        return Ok(id.clone());
    }
    let metadata =
        structured(&client.call(session, "screenshot", json!({"target": "focused_output"}))?)?;
    let id = field(&metadata, "/screenshot_ref")
        .as_str()
        .ok_or_else(|| Failure::new(format!("latency: no screenshot_ref in {metadata}")))?
        .to_owned();
    *shot = Some((id.clone(), Instant::now()));
    Ok(id)
}

/// The workspace without windows that niri keeps below the focused one.
fn empty_workspace(session: &mut Session<'_>) -> Result<u64> {
    let Response::Workspaces(workspaces) = session.request(&Request::Workspaces)? else {
        return Err(Failure::new(
            "niri answered Workspaces with another response",
        ));
    };
    workspaces
        .iter()
        .find(|workspace| workspace.active_window_id.is_none())
        .map(|workspace| workspace.id)
        .ok_or_else(|| Failure::new(format!("latency: no empty workspace in {workspaces:?}")))
}
