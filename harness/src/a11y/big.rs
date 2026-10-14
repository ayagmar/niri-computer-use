//! `--big`: windows with one big container, listed through the server. Each window has a
//! `Save` button and a `Search` entry above the container and an `Apply` button and a
//! `Notes` entry below it (`fixtures/big.py`). A window the walk reads whole must list
//! all four with a bound ref. Each listing's time and size are logged.

use std::ffi::OsString;
use std::fs;
use std::time::Duration;

use serde_json::{Value, json};

use super::{Bus, windows};
use crate::failure::{Context as _, Failure, Result};
use crate::mcp::{self, Client, field, structured};
use crate::runner::Process;
use crate::session::Session;

/// Within what is left of the run's deadline.
const NOCTALIA_DEADLINE: Duration = Duration::from_secs(225);
const SERVER_DEADLINE: Duration = Duration::from_secs(220);
const FIXTURE_DEADLINE: Duration = Duration::from_secs(60);
const READY: Duration = Duration::from_secs(20);
/// Building a few thousand widgets, then the window.
const STARTUP: Duration = Duration::from_secs(30);
/// The window's own controls, outside the container.
const CONTROLS: [&str; 4] = ["Save", "Search", "Apply", "Notes"];

/// A window of `fixtures/big.py`: its toolkit, the container's kind and its size.
struct Case {
    toolkit: &'static str,
    kind: &'static str,
    count: &'static str,
}

impl Case {
    fn label(&self) -> String {
        format!("{} {} {}", self.toolkit, self.kind, self.count)
    }
}

/// Each over the 2,000 objects the walk read before.
const CASES: [Case; 4] = [
    Case {
        toolkit: "gtk4",
        kind: "buttons",
        count: "3000",
    },
    Case {
        toolkit: "gtk4",
        kind: "rows",
        count: "1500",
    },
    Case {
        toolkit: "gtk3",
        kind: "buttons",
        count: "3000",
    },
    Case {
        toolkit: "gtk3",
        kind: "tree",
        count: "5000",
    },
];

pub(crate) fn run(session: &mut Session<'_>, server: &str) -> Result<()> {
    let bus = Bus::start(session)?;
    let noctalia = session.start(
        "noctalia",
        &[],
        session.artifact("noctalia.log"),
        NOCTALIA_DEADLINE,
    )?;
    let mut client = Client::start(session, server, "harness-big", SERVER_DEADLINE)?;
    mcp::ready(session, &mut client, "big-ready", READY)?;
    structured(&client.call(session, "acquire_desktop", json!({}))?)?;
    for case in &CASES {
        let fixture = start(session, case)?;
        let window = window(session, &bus, case, fixture.pid())?;
        whole(session, &mut client, case, window)?;
        fixture.stop()?;
    }
    structured(&client.call(session, "release_desktop", json!({"restore_focus": false}))?)?;
    client.stop()?;
    noctalia.stop()?;
    bus.stop()
}

fn start(session: &Session<'_>, case: &Case) -> Result<Process> {
    let root = session.test_dir().root();
    let file = root.join("big.py");
    fs::write(&file, include_str!("../../fixtures/big.py"))
        .context(format!("write {}", file.display()))?;
    let args: Vec<OsString> = vec![
        "-I".into(),
        file.into(),
        root.into(),
        case.toolkit.into(),
        case.kind.into(),
        case.count.into(),
    ];
    let log = session.artifact(&format!("fixture-big-{}-{}.log", case.toolkit, case.kind));
    session.start("python3", &args, log, FIXTURE_DEADLINE)
}

/// The fixture's window, once its app is on the bus.
fn window(session: &mut Session<'_>, bus: &Bus, case: &Case, pid: i32) -> Result<u64> {
    let what = case.label();
    let window = session.wait_until("big-window", &what, STARTUP, |session| {
        Ok(windows(session)?
            .into_iter()
            .find(|window| window.pid == Some(pid)))
    })?;
    session.wait_until("big-registered", &what, STARTUP, |session| {
        Ok(bus.registered(session, pid)?.then_some(()))
    })?;
    Ok(window.id)
}

/// The walk reads the window whole, and lists each of its own controls with a bound ref.
fn whole(session: &mut Session<'_>, client: &mut Client, case: &Case, window: u64) -> Result<()> {
    let label = case.label();
    let args = json!({"window_id": window, "limit": 500, "format": "json"});
    let (result, took) = client.timed_call("elements", args)?;
    let listing = structured(&result)?;
    session.log(&format!(
        "big: {label}: listed in {took:?}, {} objects walked, capped {}",
        field(&listing, "/walked"),
        field(&listing, "/capped_reason"),
    ))?;
    if field(&listing, "/capped") != false {
        return Err(Failure::new(format!(
            "big: {label}: the walk was cut short: {}",
            field(&listing, "/capped_reason")
        )));
    }
    for name in CONTROLS {
        let element = control(client, window, name)?;
        if field(&element, "/element_ref").is_null() || !field(&element, "/unbound").is_null() {
            return Err(Failure::new(format!(
                "big: {label}: {name} has no bound ref: {element}"
            )));
        }
    }
    session.log(&format!("big: {label}: {CONTROLS:?} listed and bound"))
}

/// The first element `elements` lists by `name`.
fn control(client: &mut Client, window: u64, name: &str) -> Result<Value> {
    let args = json!({"window_id": window, "name_contains": name, "format": "json"});
    let (result, _) = client.timed_call("elements", args)?;
    let listing = structured(&result)?;
    let element = field(&listing, "/elements/0").clone();
    if element.is_null() {
        return Err(Failure::new(format!("{name} isn't listed: {listing}")));
    }
    Ok(element)
}
