//! `--latency`: each action tool's round trip against the nested niri, for
//! docs/results/action-latency.md. One server holds the lease over the GTK 4 accessibility
//! fixture, with the nested Noctalia as the lock source. Each round times `focus` to the
//! empty workspace and back to the fixture, `click` on the fixture's entry, `type_text` and
//! `key` into it, `activate_element` on its `Primary` button, `niri_action` resizing the
//! fixture, and `shell_panel` opening and closing the control center, none of them with a
//! screenshot. The click waits for the view to settle first, untimed. It writes each call's round trips and which `observed` and `changes` keys
//! came back to `latency.json` in the artifacts, and a summary to the log.

use std::collections::{BTreeMap, BTreeSet};
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
/// The server refuses an element aim for a second after niri activated a workspace on its
/// output or moved or resized its window, as the focus calls and the resize do. The click
/// waits this long first, outside its round trip, rather than being refused or retried.
const SETTLE: Duration = Duration::from_millis(1200);

pub(crate) fn run(session: &mut Session<'_>, server: &str) -> Result<()> {
    let bus = Bus::start(session)?;
    let noctalia = session.start(
        "noctalia",
        &[],
        session.artifact("noctalia.log"),
        NOCTALIA_DEADLINE,
    )?;
    let fixture = Fixture::start(session, Toolkit::Gtk4)?;
    let window = fixture.window(session, &bus)?;
    let empty = empty_workspace(session, window.workspace_id)?;
    let window = window.id;
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
            if tool == "click" {
                crate::session::pause(SETTLE);
            }
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

/// Each label's calls.
#[derive(Default)]
struct Timings(BTreeMap<&'static str, Calls>);

/// One label's round trips, and how often each `observed` and each set of `changes` keys
/// came back.
#[derive(Default)]
struct Calls {
    times: Vec<Duration>,
    observed: BTreeMap<String, usize>,
    changes: BTreeMap<String, usize>,
}

impl Timings {
    fn add(&mut self, label: &'static str, took: Duration, outcome: &Value) {
        let calls = self.0.entry(label).or_default();
        calls.times.push(took);
        let seen = field(outcome, "/observed").as_str().unwrap_or("none");
        *calls.observed.entry(seen.to_owned()).or_default() += 1;
        *calls.changes.entry(changed(outcome)).or_default() += 1;
    }

    /// Each label's median and 95th percentile in milliseconds, its `observed` counts and
    /// its `changes` counts.
    fn summary(&self) -> Result<Value> {
        let mut summary = serde_json::Map::new();
        for (label, calls) in &self.0 {
            let times = &calls.times;
            let p95 = proc::p95(times).ok_or_else(|| Failure::new("no round trips"))?;
            summary.insert(
                (*label).to_owned(),
                json!({
                    "median": median_millis(times)?,
                    "p95": as_millis(p95),
                    "observed": calls.observed,
                    "changes": calls.changes,
                }),
            );
        }
        Ok(Value::Object(summary))
    }
}

/// The keys of an outcome's `changes`, sorted and joined by `+`, with the fields its
/// `windows` entries changed, as in `focus+windows[size]`; or `none`.
fn changed(outcome: &Value) -> String {
    let Some(changes) = outcome.get("changes").and_then(Value::as_object) else {
        return "none".to_owned();
    };
    let mut keys: Vec<String> = changes
        .iter()
        .map(|(key, value)| match value.as_array() {
            Some(windows) if key == "windows" => format!("windows[{}]", fields(windows)),
            _ => key.clone(),
        })
        .collect();
    keys.sort_unstable();
    keys.join("+")
}

/// The fields `windows` entries changed, besides their `id`, sorted and joined by `,`.
fn fields(windows: &[Value]) -> String {
    let fields: BTreeSet<&str> = windows
        .iter()
        .filter_map(Value::as_object)
        .flat_map(|window| window.keys().map(String::as_str))
        .filter(|field| *field != "id")
        .collect();
    fields.into_iter().collect::<Vec<_>>().join(",")
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
    arguments.insert("format".to_owned(), json!("json"));
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

/// The workspace without windows that niri keeps below `own`, on the same output.
fn empty_workspace(session: &mut Session<'_>, own: Option<u64>) -> Result<u64> {
    let Response::Workspaces(workspaces) = session.request(&Request::Workspaces)? else {
        return Err(Failure::new(
            "niri answered Workspaces with another response",
        ));
    };
    let own = workspaces
        .iter()
        .find(|workspace| Some(workspace.id) == own)
        .ok_or_else(|| {
            Failure::new(format!(
                "latency: the fixture has no workspace in {workspaces:?}"
            ))
        })?;
    workspaces
        .iter()
        .find(|workspace| {
            workspace.output == own.output
                && Some(workspace.idx) == own.idx.checked_add(1)
                && workspace.active_window_id.is_none()
        })
        .map(|workspace| workspace.id)
        .ok_or_else(|| Failure::new(format!("latency: no empty workspace below {own:?}")))
}
