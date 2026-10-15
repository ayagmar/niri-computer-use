//! `--big`: windows with one big container, listed through the server. Each window has a
//! `Save` button and a `Search` entry above the container and an `Apply` button and a
//! `Notes` entry below it (`fixtures/big.py`). A window the walk reads whole must list
//! all four with a bound ref. One past the walk's 20,000 objects, or with more children
//! than the app will list, must still list them, since the container's children are read
//! last, but as `unbound: cut_short`, since the container may hold a look-alike of each.
//! Each listing's time and size are logged.

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
/// Building up to 22,000 rows, then the window.
const STARTUP: Duration = Duration::from_secs(30);
/// The window's own controls, outside the container.
const CONTROLS: [&str; 4] = ["Save", "Search", "Apply", "Notes"];

/// A window of `fixtures/big.py`: its toolkit, the container's kind and its size, and
/// what ends its walk short, if anything.
struct Case {
    toolkit: &'static str,
    kind: &'static str,
    count: &'static str,
    cut: Option<&'static str>,
}

impl Case {
    fn label(&self) -> String {
        format!("{} {} {}", self.toolkit, self.kind, self.count)
    }
}

/// Each over the 2,000 objects the walk read before. The fifth is over the walk's 20,000;
/// the last has a table of 66,003 children, which at-spi2-atk refuses to list past 65,536.
const CASES: [Case; 6] = [
    Case {
        toolkit: "gtk4",
        kind: "buttons",
        count: "3000",
        cut: None,
    },
    Case {
        toolkit: "gtk4",
        kind: "rows",
        count: "1500",
        cut: None,
    },
    Case {
        toolkit: "gtk3",
        kind: "buttons",
        count: "3000",
        cut: None,
    },
    Case {
        toolkit: "gtk3",
        kind: "tree",
        count: "5000",
        cut: None,
    },
    Case {
        toolkit: "gtk3",
        kind: "tree",
        count: "8000",
        cut: Some("node_cap"),
    },
    Case {
        toolkit: "gtk3",
        kind: "tree",
        count: "22000",
        cut: Some("children_refused"),
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
        listed(session, &mut client, case, window)?;
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

/// The walk ends as `case` says, and lists each of the window's own controls, with a
/// bound ref when it read the whole window and `cut_short` when it didn't.
fn listed(session: &mut Session<'_>, client: &mut Client, case: &Case, window: u64) -> Result<()> {
    let label = case.label();
    let args = json!({"window_id": window, "limit": 500, "format": "json"});
    let (result, took) = client.timed_call("elements", args)?;
    let listing = structured(&result)?;
    session.log(&format!(
        "big: {label}: listed in {took:?}, {} objects walked, capped {}",
        field(&listing, "/walked"),
        field(&listing, "/capped_reason"),
    ))?;
    if field(&listing, "/capped_reason").as_str() != case.cut {
        return Err(Failure::new(format!(
            "big: {label}: expected the walk cut by {:?}, got {}",
            case.cut,
            field(&listing, "/capped_reason")
        )));
    }
    let unbound = case.cut.map(|_| "cut_short");
    for name in CONTROLS {
        let element = control(client, window, name)?;
        if field(&element, "/element_ref").is_null()
            || field(&element, "/unbound").as_str() != unbound
        {
            return Err(Failure::new(format!(
                "big: {label}: expected {name} with a ref, unbound {unbound:?}: {element}"
            )));
        }
    }
    session.log(&format!(
        "big: {label}: {CONTROLS:?} listed, unbound {unbound:?}"
    ))
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
