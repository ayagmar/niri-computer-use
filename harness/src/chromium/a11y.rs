//! `--chromium-a11y`: Chromium's pages on the accessibility bus, through the server. A
//! Chromium-based browser reads `org.a11y.Status.IsEnabled` once, when it starts, and puts
//! itself on the bus only if it was true; it builds its tree only once an assistive
//! technology asks an object for its attributes. The server turns the session's
//! accessibility on with a listener of its own when `elements` finds a window's app off
//! the bus, and asks a frame without its tree for its attributes.
//!
//! The run checks that a browser started while accessibility was off isn't listed and is
//! reported as needing a restart, that the server turned accessibility on, that a browser
//! started after lists the fixture's field and button and that activating the button
//! reaches the page, that a fresh page's first listing says the page is building its tree,
//! and that accessibility goes back off when the server ends. It logs what listing a long
//! page costs and what a running browser does when accessibility turns on or off.
//! Everything runs on the nested session bus, which is checked to be under `TEST_DIR/run`
//! first.

use std::ffi::OsString;
use std::time::{Duration, Instant};

use niri_ipc::{Request, Response, Window};
use serde_json::{Value, json};

use super::Launch;
use crate::a11y::{self, Bus};
use crate::failure::{Context as _, Failure, Result};
use crate::mcp::{self, Client, field, report, structured};
use crate::nested;
use crate::runner::Process;
use crate::session::{Session, pause};

const PAGE: &str = include_str!("../../fixtures/chromium-a11y.html");
const TITLE: &str = "ncu-a11y:";
/// Within what is left of the run's deadline.
const NOCTALIA_DEADLINE: Duration = Duration::from_secs(220);
const SERVER_DEADLINE: Duration = Duration::from_secs(210);
const READY: Duration = Duration::from_secs(20);
/// A fresh profile's first start, with the page loaded.
const STARTUP: Duration = Duration::from_secs(30);
const WAIT: Duration = Duration::from_secs(5);
/// How long a browser is watched for a change after accessibility turns on or off.
const WATCH: Duration = Duration::from_secs(3);
const TERM_GRACE: Duration = Duration::from_secs(5);
/// Long pages: one within the walk's node cap, one past it.
const ROWS: [u32; 2] = [300, 2000];

pub(crate) fn run(session: &mut Session<'_>, server: &str, browser: &str) -> Result<()> {
    let status = Status::private(session)?;
    let bus = Bus::start(session)?;
    status.expect(session, false, "before the server")?;
    let noctalia = session.start(
        "noctalia",
        &[],
        session.artifact("noctalia.log"),
        NOCTALIA_DEADLINE,
    )?;
    let mut client = Client::start(session, server, "harness-chromium-a11y", SERVER_DEADLINE)?;
    mcp::ready(session, &mut client, "chromium-a11y-ready", READY)?;
    structured(&client.call(session, "acquire_desktop", json!({}))?)?;
    let before = Page::open(session, browser, "before", 0)?;
    before.refused(session, &mut client, "was off, so the server turned it on")?;
    status.expect(session, true, "after the server's listener")?;
    let after = Page::open(session, browser, "after", 0)?;
    after.registered(session, &bus)?;
    let field = after.element(session, &mut client, "entry", "ncu field")?;
    after.set_text(session, &mut client, &field)?;
    let button = after.element(session, &mut client, "button", "ncu send")?;
    after.activate(session, &mut client, &button)?;
    after.frame(session, &bus)?;
    before.watch(session, &bus, "after accessibility turned on")?;
    before.refused(session, &mut client, "the session's accessibility is on")?;
    for rows in ROWS {
        long_page(session, &mut client, &bus, browser, rows)?;
    }
    structured(&client.call(session, "release_desktop", json!({"restore_focus": false}))?)?;
    client.stop()?;
    status.expect(session, false, "after the server ended")?;
    after.watch(session, &bus, "after accessibility turned off")?;
    after.process.terminate(TERM_GRACE)?;
    before.process.terminate(TERM_GRACE)?;
    noctalia.stop()?;
    bus.stop()
}

/// `org.a11y.Status` on the nested session bus, which the run only reads.
#[derive(Debug)]
struct Status {
    address: String,
}

impl Status {
    /// The session bus, once its socket resolves under `TEST_DIR/run`.
    fn private(session: &Session<'_>) -> Result<Self> {
        let address =
            std::env::var("DBUS_SESSION_BUS_ADDRESS").context("read DBUS_SESSION_BUS_ADDRESS")?;
        nested::resolve_under(&nested::dbus_socket(&address)?, &session.test_dir().run())?;
        Ok(Self { address })
    }

    fn enabled(&self, session: &Session<'_>) -> Result<bool> {
        let args = [
            format!("--address={}", self.address).as_str(),
            "--json=short",
            "get-property",
            "org.a11y.Bus",
            "/org/a11y/bus",
            "org.a11y.Status",
            "IsEnabled",
        ]
        .map(OsString::from);
        let output = session.run("busctl", &args)?;
        let reply: Value =
            serde_json::from_slice(&output.stdout).context("parse busctl's reply")?;
        reply
            .pointer("/data")
            .and_then(Value::as_bool)
            .ok_or_else(|| Failure::new(format!("expected a boolean, got {reply}")))
    }

    /// Waits for `IsEnabled` to be `on`.
    fn expect(&self, session: &mut Session<'_>, on: bool, when: &str) -> Result<()> {
        let what = format!("IsEnabled {on} {when}");
        session.wait_until("chromium-a11y-status", &what, WAIT, |session| {
            Ok((self.enabled(session)? == on).then_some(()))
        })?;
        session.log(&format!("Chromium a11y: {what}"))
    }
}

/// A browser of its own showing the fixture, and its window.
#[derive(Debug)]
struct Page {
    process: Process,
    window: Window,
    name: String,
}

impl Page {
    fn open(session: &mut Session<'_>, browser: &str, name: &str, rows: u32) -> Result<Self> {
        let query = format!("?rows={rows}");
        let launch = Launch {
            name,
            html: PAGE,
            query: &query,
            env: &[],
            flags: &[],
        };
        let mut process = launch.start(session, browser)?;
        let title = format!("{TITLE}{rows}:");
        let started = Instant::now();
        let window = session.wait_until("chromium-a11y-window", &title, STARTUP, |session| {
            process.ensure_running().map_err(|failure| {
                Failure::new(format!(
                    "the browser ended before its window opened: {failure}"
                ))
            })?;
            titled(session, &title, process.pid())
        })?;
        session.log(&format!(
            "Chromium a11y: {name}'s window {} after {:?}",
            window.id,
            started.elapsed()
        ))?;
        Ok(Self {
            process,
            window,
            name: name.to_owned(),
        })
    }

    fn pid(&self) -> Result<i32> {
        self.window
            .pid
            .ok_or_else(|| Failure::new(format!("niri knows no pid for {}", self.name)))
    }

    /// `elements` must fail `not_accessible`, its detail saying `why` and to ask the user.
    fn refused(&self, session: &mut Session<'_>, client: &mut Client, why: &str) -> Result<()> {
        let result = client.call(session, "elements", json!({"window_id": self.window.id}))?;
        let error = report(&result);
        let detail = field(&error, "/detail").as_str().unwrap_or_default();
        if field(&error, "/error") != "not_accessible"
            || !detail.contains(why)
            || !detail.contains("ask the user")
        {
            return Err(Failure::new(format!(
                "Chromium a11y: {}'s elements: {result}",
                self.name
            )));
        }
        session.log(&format!(
            "Chromium a11y: {}: not_accessible, {detail}",
            self.name
        ))
    }

    /// Waits for the browser to be on the bus.
    fn registered(&self, session: &mut Session<'_>, bus: &Bus) -> Result<()> {
        let pid = self.pid()?;
        let started = Instant::now();
        session.wait_until(
            "chromium-a11y-registered",
            "the browser on the bus",
            WAIT,
            |session| Ok(bus.registered(session, pid)?.then_some(())),
        )?;
        session.log(&format!(
            "Chromium a11y: {} on the bus {:?} after its window",
            self.name,
            started.elapsed()
        ))
    }

    /// Logs whether the browser is on the bus, `WATCH` from now.
    fn watch(&self, session: &mut Session<'_>, bus: &Bus, when: &str) -> Result<()> {
        pause(WATCH);
        let on = bus.registered(session, self.pid()?)?;
        session.log(&format!(
            "Chromium a11y: {} {} the bus {WATCH:?} {when}",
            self.name,
            if on { "is on" } else { "is still off" }
        ))
    }

    /// The `element_ref` of the element with `role` and `name` as `elements` lists it,
    /// which must be bound.
    fn element(
        &self,
        session: &mut Session<'_>,
        client: &mut Client,
        role: &str,
        name: &str,
    ) -> Result<String> {
        let args = json!({
            "window_id": self.window.id, "role": role, "name_contains": name, "format": "json"
        });
        let (result, took) = client.timed_call("elements", args)?;
        let listing = structured(&result)?;
        let element = field(&listing, "/elements/0");
        let id = field(element, "/element_ref")
            .as_str()
            .filter(|_| field(element, "/unbound").is_null())
            .ok_or_else(|| Failure::new(format!("Chromium a11y: no bound {name} in {listing}")))?
            .to_owned();
        session.log(&format!(
            "Chromium a11y: {}'s {name} listed in {took:?}, {} objects walked: {element}",
            self.name,
            field(&listing, "/walked")
        ))?;
        Ok(id)
    }

    /// Logs what `set_element_text` on the field does: Chromium 155's text fields have no
    /// `EditableText` interface.
    fn set_text(&self, session: &mut Session<'_>, client: &mut Client, id: &str) -> Result<()> {
        let args = json!({
            "element": id, "text": "typed", "expect": {"window_id": self.window.id}
        });
        let result = client.call(session, "set_element_text", args)?;
        session.log(&format!(
            "Chromium a11y: set_element_text on the field: {}",
            field_text(&result)
        ))
    }

    /// Activates the button, whose click the page's title must then show.
    fn activate(&self, session: &mut Session<'_>, client: &mut Client, id: &str) -> Result<()> {
        let args = json!({"element": id, "expect": {"window_id": self.window.id}});
        let result = structured(&client.call(session, "activate_element", args)?)?;
        let title = format!("{TITLE}0:sent");
        session.wait_until(
            "chromium-a11y-click",
            "the button's click",
            WAIT,
            |session| {
                Ok(windows(session)?
                    .iter()
                    .any(|window| {
                        window.id == self.window.id
                            && window.title.as_deref() == Some(title.as_str())
                    })
                    .then_some(()))
            },
        )?;
        session.log(&format!(
            "Chromium a11y: activate_element reached the page: {result}"
        ))
    }

    /// Logs the frame's extents beside niri's window size, which `elements` compares.
    fn frame(&self, session: &mut Session<'_>, bus: &Bus) -> Result<()> {
        let pid = self.pid()?;
        let mut name = None;
        for app in bus.apps(session)? {
            if bus.pid(session, &app)? == pid {
                name = Some(app);
            }
        }
        let name = name.ok_or_else(|| Failure::new("the browser left the bus"))?;
        let accessible = "org.a11y.atspi.Accessible";
        let frames = bus.call(session, &[&name, a11y::ROOT, accessible, "GetChildren"])?;
        let frame = field(&frames, "/data/0/0/1").as_str().unwrap_or_default();
        let component = "org.a11y.atspi.Component";
        let extents = bus.call(session, &[&name, frame, component, "GetExtents", "u", "1"])?;
        session.log(&format!(
            "Chromium a11y: frame extents {} beside niri's window size {:?}",
            field(&extents, "/data/0"),
            self.window.layout.window_size
        ))
    }
}

/// Lists a page of `rows` rows: once as the server first reads it, which has its tree
/// built, then once the page's field is listed, and once more. Logs what each took and
/// gave, and how long the page's own elements took to appear.
fn long_page(
    session: &mut Session<'_>,
    client: &mut Client,
    bus: &Bus,
    browser: &str,
    rows: u32,
) -> Result<()> {
    let page = Page::open(session, browser, &format!("rows{rows}"), rows)?;
    page.registered(session, bus)?;
    let args = json!({"window_id": page.window.id, "limit": 500, "format": "json"});
    let built = Instant::now();
    let first = listing(session, client, &args, &format!("{rows} rows, first"))?;
    if first.get("building") != Some(&json!(true)) {
        return Err(Failure::new(format!(
            "the first listing of {rows} rows didn't say the page was building its tree: {}",
            summary(&first)
        )));
    }
    let field = json!({"window_id": page.window.id, "role": "entry", "name_contains": "ncu field"});
    session.wait_until(
        "chromium-a11y-page",
        "the page's elements",
        STARTUP,
        |session| {
            let result = client.call(session, "elements", field.clone())?;
            let text = field_text(&result);
            Ok(text.contains("\"ncu field\"").then_some(()))
        },
    )?;
    session.log(&format!(
        "Chromium a11y: {rows} rows, the page's field listed {:?} after the first listing",
        built.elapsed()
    ))?;
    for pass in ["built", "again"] {
        listing(session, client, &args, &format!("{rows} rows, {pass}"))?;
    }
    page.process.terminate(TERM_GRACE).map(drop)
}

/// Lists with `args` and logs what it took and gave, as `when`.
fn listing(
    session: &mut Session<'_>,
    client: &mut Client,
    args: &Value,
    when: &str,
) -> Result<Value> {
    let (result, took) = client.timed_call("elements", args.clone())?;
    let listing = structured(&result)?;
    session.log(&format!(
        "Chromium a11y: {when} listing in {took:?}: {}",
        summary(&listing)
    ))?;
    Ok(listing)
}

fn field_text(result: &Value) -> &str {
    field(result, "/content/0/text")
        .as_str()
        .unwrap_or_default()
}

/// How many elements a listing has, how many refs it gives and why some are refused.
fn summary(listing: &Value) -> String {
    let elements = field(listing, "/elements")
        .as_array()
        .cloned()
        .unwrap_or_default();
    let refs = elements
        .iter()
        .filter(|element| !element["element_ref"].is_null())
        .count();
    let mut unbound = std::collections::BTreeMap::<String, usize>::new();
    for why in elements
        .iter()
        .filter_map(|element| element["unbound"].as_str())
    {
        *unbound.entry(why.to_owned()).or_default() += 1;
    }
    let usable = refs.saturating_sub(unbound.values().sum());
    format!(
        "{} listed, {refs} refs of which {usable} bound, unbound {unbound:?}, {} walked, truncated {}, capped {} ({}), building {}",
        elements.len(),
        field(listing, "/walked"),
        field(listing, "/truncated"),
        field(listing, "/capped"),
        field(listing, "/capped_reason"),
        listing.get("building").is_some()
    )
}

/// The window titled `title` of a process under `browser`, if one is open.
fn titled(session: &mut Session<'_>, title: &str, browser: i32) -> Result<Option<Window>> {
    let mut found = None;
    for window in windows(session)? {
        let pid = window.pid.unwrap_or_default();
        if window.title.as_deref() == Some(title) && a11y::descends_from(pid, browser)? {
            found = Some(window);
        }
    }
    Ok(found)
}

fn windows(session: &mut Session<'_>) -> Result<Vec<Window>> {
    let Response::Windows(windows) = session.request(&Request::Windows)? else {
        return Err(Failure::new("niri answered Windows with another response"));
    };
    Ok(windows)
}
