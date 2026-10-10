//! M9's nested acceptance: a private accessibility bus in the nested session, with the
//! fixtures registered on it. The bus launcher claims `org.a11y.Bus` on the nested
//! session bus, which has no service directories, so the registry daemon is started by
//! hand; nothing is activated from the host. Before any check reads the bus, its socket
//! must resolve under `TEST_DIR/run`, and every application the registry lists must be a
//! process of the nested session.

use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use serde_json::Value;

use crate::failure::{Context as _, Failure, Result};
use crate::nested;
use crate::runner::Process;
use crate::session::Session;

const LAUNCHER: &str = "/usr/lib/at-spi-bus-launcher";
const REGISTRY: &str = "/usr/lib/at-spi2-registryd";
/// Within what is left of the run's deadline.
const BUS_DEADLINE: Duration = Duration::from_secs(230);
const STARTUP: Duration = Duration::from_secs(10);
const FIXTURE_DEADLINE: Duration = Duration::from_secs(220);
const REGISTRY_NAME: &str = "org.a11y.atspi.Registry";
const ROOT: &str = "/org/a11y/atspi/accessible/root";

pub(crate) fn run(session: &mut Session<'_>) -> Result<()> {
    let bus = Bus::start(session)?;
    bus.only_nested_apps(session, "before the fixtures")?;
    let gtk = start_gtk(session)?;
    session.wait_until(
        "a11y-gtk",
        "the GTK 4 fixture on the bus",
        STARTUP,
        |session| Ok((!bus.apps(session)?.is_empty()).then_some(())),
    )?;
    bus.only_nested_apps(session, "with the GTK 4 fixture")?;
    gtk.stop()?;
    bus.stop()
}

/// The nested accessibility bus: its launcher, the registry daemon, and its address.
#[derive(Debug)]
pub(crate) struct Bus {
    launcher: Process,
    registry: Process,
    pub(crate) address: String,
}

impl Bus {
    /// Starts the launcher, waits for the address on the session bus and checks that its
    /// socket is under `TEST_DIR/run`, then starts the registry and waits for its name.
    pub(crate) fn start(session: &mut Session<'_>) -> Result<Self> {
        let session_bus =
            std::env::var("DBUS_SESSION_BUS_ADDRESS").context("read DBUS_SESSION_BUS_ADDRESS")?;
        let args = ["--launch-immediately", "--a11y=1"].map(OsString::from);
        let launcher = session.start(
            LAUNCHER,
            &args,
            session.artifact("at-spi-bus.log"),
            BUS_DEADLINE,
        )?;
        owned(session, &session_bus, "org.a11y.Bus")?;
        let reply = busctl(
            session,
            &session_bus,
            &[
                "org.a11y.Bus",
                "/org/a11y/bus",
                "org.a11y.Bus",
                "GetAddress",
            ],
        )?;
        let address = string_reply(&reply)?;
        let socket = nested::dbus_socket(&address)?;
        nested::resolve_under(&socket, &session.test_dir().run())?;
        session.log(&format!("M9: accessibility bus at {}", socket.display()))?;
        let registry = session.start(
            REGISTRY,
            &[],
            session.artifact("at-spi-registry.log"),
            BUS_DEADLINE,
        )?;
        owned(session, &address, REGISTRY_NAME)?;
        Ok(Self {
            launcher,
            registry,
            address,
        })
    }

    /// The registered applications: their unique bus names.
    pub(crate) fn apps(&self, session: &Session<'_>) -> Result<Vec<String>> {
        let reply = busctl(
            session,
            &self.address,
            &[
                REGISTRY_NAME,
                ROOT,
                "org.a11y.atspi.Accessible",
                "GetChildren",
            ],
        )?;
        children(&reply)
    }

    /// Requires every application on the bus to be a process of the nested session: a
    /// descendant of the nested niri, which is the supervisor's parent.
    pub(crate) fn only_nested_apps(&self, session: &mut Session<'_>, when: &str) -> Result<()> {
        let niri = rustix::process::getppid()
            .ok_or_else(|| Failure::new("the supervisor has no parent"))?
            .as_raw_nonzero()
            .get();
        let apps = self.apps(session)?;
        let mut pids = Vec::new();
        for app in &apps {
            let pid = self.pid(session, app)?;
            if !descends_from(pid, niri)? {
                return Err(Failure::new(format!(
                    "M9: application {app} on the accessibility bus is pid {pid}, outside the nested session"
                )));
            }
            pids.push(pid);
        }
        session.log(&format!(
            "M9: {when}, the registry lists {} applications, all nested: pids {pids:?}",
            apps.len()
        ))
    }

    /// The process ID of a bus connection, as the bus daemon knows it.
    pub(crate) fn pid(&self, session: &Session<'_>, name: &str) -> Result<i32> {
        let reply = busctl(
            session,
            &self.address,
            &[
                "org.freedesktop.DBus",
                "/org/freedesktop/DBus",
                "org.freedesktop.DBus",
                "GetConnectionUnixProcessID",
                "s",
                name,
            ],
        )?;
        let pid = field_u64(&reply)?;
        i32::try_from(pid).map_err(|_| Failure::new(format!("pid {pid} is out of range")))
    }

    pub(crate) fn stop(self) -> Result<()> {
        self.registry.stop()?;
        self.launcher.stop().map(drop)
    }
}

/// Waits until `name` has an owner on the bus at `address`.
fn owned(session: &mut Session<'_>, address: &str, name: &str) -> Result<()> {
    let what = format!("{name} on the bus");
    session.wait_until("a11y-name", &what, STARTUP, |session| {
        let reply = busctl(
            session,
            address,
            &[
                "org.freedesktop.DBus",
                "/org/freedesktop/DBus",
                "org.freedesktop.DBus",
                "NameHasOwner",
                "s",
                name,
            ],
        )?;
        Ok((reply.pointer("/data/0") == Some(&Value::Bool(true))).then_some(()))
    })
}

/// One method call with `busctl`, whose reply comes back as JSON:
/// `{"type": <signature>, "data": [<values>]}`.
fn busctl(session: &Session<'_>, address: &str, call: &[&str]) -> Result<Value> {
    let mut args = vec![
        OsString::from(format!("--address={address}")),
        "--json=short".into(),
        "call".into(),
    ];
    args.extend(call.iter().map(OsString::from));
    let output = session.run("busctl", &args)?;
    serde_json::from_slice(&output.stdout).context("parse busctl's reply")
}

fn string_reply(reply: &Value) -> Result<String> {
    reply
        .pointer("/data/0")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| Failure::new(format!("expected a string reply, got {reply}")))
}

fn field_u64(reply: &Value) -> Result<u64> {
    reply
        .pointer("/data/0")
        .and_then(Value::as_u64)
        .ok_or_else(|| Failure::new(format!("expected a number reply, got {reply}")))
}

/// The bus names of a `GetChildren` reply, `a(so)`.
fn children(reply: &Value) -> Result<Vec<String>> {
    let list = reply
        .pointer("/data/0")
        .and_then(Value::as_array)
        .ok_or_else(|| Failure::new(format!("expected a(so), got {reply}")))?;
    list.iter()
        .map(|child| {
            child
                .pointer("/0")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| Failure::new(format!("expected (so), got {child}")))
        })
        .collect()
}

/// Whether `pid` is `ancestor` or one of its descendants, following `/proc/<pid>/stat`.
fn descends_from(pid: i32, ancestor: i32) -> Result<bool> {
    let mut current = pid;
    while current > 1 {
        if current == ancestor {
            return Ok(true);
        }
        let stat = fs::read_to_string(format!("/proc/{current}/stat"))
            .context(format!("read /proc/{current}/stat"))?;
        current = parent_of(&stat)
            .ok_or_else(|| Failure::new(format!("unreadable /proc/{current}/stat")))?;
    }
    Ok(false)
}

/// The parent PID in a `/proc/<pid>/stat` line: the second field after the command, which
/// is in parentheses and may itself hold spaces and parentheses.
fn parent_of(stat: &str) -> Option<i32> {
    let (_, rest) = stat.rsplit_once(')')?;
    rest.split_whitespace().nth(1)?.parse().ok()
}

/// Starts the GTK 4 fixture's accessibility window.
fn start_gtk(session: &Session<'_>) -> Result<Process> {
    let fixture = session.test_dir().root().join("gtk.py");
    fs::write(&fixture, include_str!("../fixtures/gtk.py")).context("write the GTK fixture")?;
    let args = fixture_args(session, fixture, "a11y");
    session.start(
        "python3",
        &args,
        session.artifact("gtk-a11y.log"),
        FIXTURE_DEADLINE,
    )
}

fn fixture_args(session: &Session<'_>, fixture: PathBuf, mode: &str) -> Vec<OsString> {
    vec![
        "-I".into(),
        fixture.into(),
        session.test_dir().root().into(),
        mode.into(),
    ]
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn reads_busctl_replies() {
        let address = json!({"type": "s", "data": ["unix:path=/t/run/at-spi/bus,guid=1"]});
        assert_eq!(
            string_reply(&address).unwrap(),
            "unix:path=/t/run/at-spi/bus,guid=1"
        );
        assert_eq!(
            field_u64(&json!({"type": "u", "data": [4711]})).unwrap(),
            4711
        );
        let apps = json!({"type": "a(so)", "data": [[
            [":1.3", "/org/a11y/atspi/accessible/root"],
            [":1.7", "/org/a11y/atspi/accessible/root"]
        ]]});
        assert_eq!(children(&apps).unwrap(), [":1.3", ":1.7"]);
        assert_eq!(
            children(&json!({"type": "a(so)", "data": [[]]})).unwrap(),
            Vec::<String>::new()
        );
        assert!(string_reply(&json!({"type": "s", "data": [3]})).is_err());
        assert!(field_u64(&json!({})).is_err());
        assert!(children(&json!({"type": "a(so)", "data": [[[7, "/"]]]})).is_err());
    }

    #[test]
    fn reads_the_parent_after_a_command_with_spaces_and_parentheses() {
        let stat = "4711 (qml (x) 6) S 4700 4711 4711 0 -1 4194560";
        assert_eq!(parent_of(stat), Some(4700));
        assert_eq!(parent_of("1 (init) S 0 1 1"), Some(0));
        assert_eq!(parent_of("garbage"), None);
    }

    #[test]
    fn a_process_descends_from_itself_and_its_ancestors_only() {
        let me = i32::try_from(std::process::id()).unwrap();
        let parent = rustix::process::getppid().unwrap().as_raw_nonzero().get();
        assert!(descends_from(me, me).unwrap());
        assert!(descends_from(me, parent).unwrap());
        assert!(!descends_from(parent, me).unwrap());
    }
}
