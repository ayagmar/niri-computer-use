//! What a toolkit puts on the accessibility bus for an entry that only hides its text,
//! beside its plain entry and, for reference, its password entry: GTK 4's
//! `Gtk.Entry(visibility=False)` with a free-form input purpose and Qt Quick's `TextField`
//! in `NoEcho` mode. `secret_field` refuses the role `password_text`. Qt gives its hidden
//! entry that role, so it is refused like a password field; GTK 4 gives its hidden entry
//! the role `text`, and this requires that nothing but the text tells it apart from the
//! plain entry, as the docs say. The fixtures' text is synthetic, so reading it is safe.

use serde_json::{Value, json};

use super::{Bus, ROOT, Toolkit, busctl};
use crate::failure::{Failure, Result};
use crate::session::Session;

pub(super) const ACCESSIBLE: &str = "org.a11y.atspi.Accessible";
const TEXT_INTERFACE: &str = "org.a11y.atspi.Text";
const ACTION_INTERFACE: &str = "org.a11y.atspi.Action";
/// AT-SPI's roles `password_text` and `text`.
const ENTRIES: [u64; 2] = [40, 61];
/// How deep the walk goes; the fixtures' entries sit a few levels down.
const DEPTH: usize = 8;
/// What the hidden entry and the plain one must expose alike: all but the text.
const ALIKE: [&str; 6] = [
    "/role",
    "/states",
    "/interfaces",
    "/attributes",
    "/text_attributes",
    "/actions",
];

/// Records the hidden, plain and password entries of `toolkit`'s fixture `pid`, and
/// requires Qt's hidden entry to be a password field and GTK 4's to expose what the plain
/// one does but for its text: the server can't tell it apart, as its docs say.
pub(crate) fn record(
    session: &mut Session<'_>,
    bus: &Bus,
    pid: i32,
    toolkit: Toolkit,
) -> Result<()> {
    let app = app(session, bus, pid)?;
    let mut entries = Vec::new();
    walk(session, bus, (&app, ROOT), DEPTH, &mut entries)?;
    let find = |name: &str, role: u64| {
        entries
            .iter()
            .find(|(found, found_role, _)| found == name && *found_role == role)
            .map(|(_, _, path)| path.clone())
            .ok_or_else(|| Failure::new(format!("M9b: no {name} among {entries:?}")))
    };
    let [password_text, text] = ENTRIES;
    let (hidden_role, password_name) = if toolkit == Toolkit::Qt {
        // Qt marks a `NoEcho` field as a password field and drops a password field's name.
        (password_text, "")
    } else {
        (text, "Password entry")
    };
    let hidden = describe(session, bus, &app, &find("Hidden entry", hidden_role)?);
    let plain = describe(session, bus, &app, &find("Plain entry", text)?);
    let password = describe(session, bus, &app, &find(password_name, password_text)?);
    session.log(&format!(
        "M9b: {toolkit:?} hidden entry {hidden}; plain entry {plain}; password entry {password}"
    ))?;
    if hidden_role == password_text {
        return Ok(());
    }
    let differs: Vec<&str> = ALIKE
        .into_iter()
        .filter(|at| hidden.pointer(at) != plain.pointer(at))
        .collect();
    if !differs.is_empty() {
        return Err(Failure::new(format!(
            "M9b: the {toolkit:?} hidden entry differs from the plain entry in {differs:?} now: {hidden}; update the secret_field docs"
        )));
    }
    Ok(())
}

/// The fixture's connection on the bus.
pub(super) fn app(session: &Session<'_>, bus: &Bus, pid: i32) -> Result<String> {
    for app in bus.apps(session)? {
        if bus.pid(session, &app)? == pid {
            return Ok(app);
        }
    }
    Err(Failure::new(format!("M9b: no application with pid {pid}")))
}

/// Collects the name, role and path of every entry, with the role `text` or
/// `password_text`, under `at`.
fn walk(
    session: &Session<'_>,
    bus: &Bus,
    at: (&str, &str),
    depth: usize,
    found: &mut Vec<(String, u64, String)>,
) -> Result<()> {
    let (app, path) = at;
    let role = call(session, bus, app, path, &[ACCESSIBLE, "GetRole"])?;
    if let Some(role) = role
        .pointer("/data/0")
        .and_then(Value::as_u64)
        .filter(|role| ENTRIES.contains(role))
    {
        found.push((name(session, bus, app, path)?, role, path.to_owned()));
    }
    if depth == 0 {
        return Ok(());
    }
    let children = call(session, bus, app, path, &[ACCESSIBLE, "GetChildren"])?;
    let children = children
        .pointer("/data/0")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for child in children {
        if let (Some(child_app), Some(child_path)) = (
            child.pointer("/0").and_then(Value::as_str),
            child.pointer("/1").and_then(Value::as_str),
        ) {
            walk(session, bus, (child_app, child_path), depth - 1, found)?;
        }
    }
    Ok(())
}

pub(super) fn name(session: &Session<'_>, bus: &Bus, app: &str, path: &str) -> Result<String> {
    let args = [
        format!("--address={}", bus.address),
        "--json=short".to_owned(),
        "get-property".to_owned(),
        app.to_owned(),
        path.to_owned(),
        ACCESSIBLE.to_owned(),
        "Name".to_owned(),
    ]
    .map(Into::into);
    let output = session.run("busctl", &args)?;
    let reply: Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| Failure::new(format!("parse busctl's reply: {error}")))?;
    Ok(reply
        .pointer("/data")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned())
}

/// What the object at `path` exposes; a call that fails is recorded as its error.
fn describe(session: &Session<'_>, bus: &Bus, app: &str, path: &str) -> Value {
    let read = |call_args: &[&str]| {
        call(session, bus, app, path, call_args).map_or_else(
            |error| json!({"error": error.to_string()}),
            |reply| reply.pointer("/data/0").cloned().unwrap_or(Value::Null),
        )
    };
    json!({
        "role": read(&[ACCESSIBLE, "GetRole"]),
        "states": read(&[ACCESSIBLE, "GetState"]),
        "interfaces": read(&[ACCESSIBLE, "GetInterfaces"]),
        "attributes": read(&[ACCESSIBLE, "GetAttributes"]),
        // busctl would take -1, "to the end", for an option.
        "text": read(&[TEXT_INTERFACE, "GetText", "ii", "0", "100"]),
        "text_attributes": read(&[TEXT_INTERFACE, "GetDefaultAttributes"]),
        "actions": read(&[ACTION_INTERFACE, "GetActions"]),
    })
}

pub(super) fn call(
    session: &Session<'_>,
    bus: &Bus,
    app: &str,
    path: &str,
    call: &[&str],
) -> Result<Value> {
    let mut args = vec![app, path];
    args.extend_from_slice(call);
    busctl(session, &bus.address, &args)
}
