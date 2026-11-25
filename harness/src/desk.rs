//! A background desk inside the nested niri: `desk start` makes one, a server on it with
//! `serve --desk` types into a GTK 4 entry launched inside it from a preset, and the
//! nested niri, the desk's host, never sees the window, the focus change or a workspace
//! change. The nested Noctalia is the host's lock source. The stop key, sent as niri's
//! `spawn` of `stop` on the host as the keybind sends it, stops the desk's agent, and
//! `resume` lets it act again. `watch --desk` names the desk. Everything the desk makes is
//! under `TEST_DIR`; `desk stop` removes it, also when a check fails.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use niri_ipc::{Action, Request, Response, Window, Workspace};
use serde_json::{Value, json};

use crate::failure::{Context as _, Failure, Result};
use crate::mcp::{Client, field, report, structured};
use crate::session::Session;

/// The desk's name, short so its sockets fit under `TEST_DIR`.
const NAME: &str = "h1";
const SIZE: &str = "1280x720";
const NOCTALIA_DEADLINE: Duration = Duration::from_secs(110);
/// `desk start` and `desk stop` each, including their supervisor's waits.
const DESK_DEADLINE: Duration = Duration::from_secs(60);
const CLIENT_DEADLINE: Duration = Duration::from_secs(80);
const READY: Duration = Duration::from_secs(20);
const WAIT: Duration = Duration::from_secs(10);
const ENTRY: &str = "org.ncu.Entry";
const TYPED: &str = "typed in a desk";

pub(crate) fn run(session: &mut Session<'_>, server: &str) -> Result<()> {
    let noctalia = session.start(
        "noctalia",
        &[],
        session.artifact("noctalia.log"),
        NOCTALIA_DEADLINE,
    )?;
    write_policy(session)?;
    let host = Host::read(session)?;
    desk(
        session,
        server,
        &["start".into(), "--size".into(), SIZE.into(), NAME.into()],
    )?;
    session.log(&format!("desk: {NAME} started at {SIZE}"))?;
    let checked = on_desk(session, server, &host);
    let stopped = desk(session, server, &["stop".into(), NAME.into()]);
    let gone = desk_root(session).exists().then(|| {
        Failure::new(format!(
            "{} outlived desk stop",
            desk_root(session).display()
        ))
    });
    copy_logs(session);
    checked?;
    stopped?;
    if let Some(left) = gone {
        return Err(left);
    }
    session.log("desk: desk stop removed the desk")?;
    noctalia.stop().map(drop)
}

/// The checks on a running desk.
fn on_desk(session: &mut Session<'_>, server: &str, host: &Host) -> Result<()> {
    let args = ["serve".into(), "--desk".into(), NAME.into()];
    let mut client = Client::start_command(session, server, &args, "desk", CLIENT_DEADLINE)?;
    ready(session, &mut client)?;
    types(session, &mut client)?;
    host.unchanged(session)?;
    watches(session, server)?;
    stops(session, server, &mut client)?;
    client.stop()
}

/// The host's view before the desk: the focused window, the workspaces, the windows.
struct Host {
    focused: Option<u64>,
    workspaces: Vec<Workspace>,
    windows: Vec<u64>,
}

impl Host {
    fn read(session: &mut Session<'_>) -> Result<Self> {
        Ok(Self {
            focused: focused(session)?,
            workspaces: workspaces(session)?,
            windows: windows(session)?.iter().map(|window| window.id).collect(),
        })
    }

    /// The nested niri has the same focus, workspaces and windows as before the desk.
    fn unchanged(&self, session: &mut Session<'_>) -> Result<()> {
        let now = Self::read(session)?;
        expect(
            now.focused == self.focused,
            "the nested niri's focused window unchanged",
            &format!("{:?}, then {:?}", self.focused, now.focused),
        )?;
        expect(
            now.workspaces == self.workspaces,
            "the nested niri's workspaces unchanged",
            &format!("{:?}, then {:?}", self.workspaces, now.workspaces),
        )?;
        expect(
            now.windows == self.windows,
            "no new window on the nested niri",
            &format!("{:?}, then {:?}", self.windows, now.windows),
        )?;
        session.log("desk: the nested niri's focus, workspaces and windows never changed")
    }
}

/// Waits until the desk's server names the desk and reads the host's Noctalia as unlocked.
fn ready(session: &mut Session<'_>, client: &mut Client) -> Result<()> {
    let status = session.wait_until(
        "desk-ready",
        "status naming the desk, with the host's screen unlocked",
        READY,
        |session| {
            let status = structured(&client.call(session, "status", json!({}))?)?;
            let ready =
                field(&status, "/desk") == NAME && field(&status, "/lock/state") == "unlocked";
            Ok(ready.then_some(status))
        },
    )?;
    expect(
        field(&status, "/noctalia") == "not_installed",
        "a desk without a shell of its own",
        &field(&status, "/noctalia").to_string(),
    )?;
    session.log(&format!(
        "desk status: lock {}, stop key {} ({}), accessibility {}",
        field(&status, "/lock"),
        field(&status, "/stop_key"),
        field(&status, "/stop_key_detail"),
        field(&status, "/accessibility/available"),
    ))
}

/// Launches the entry inside the desk and types into it.
fn types(session: &mut Session<'_>, client: &mut Client) -> Result<()> {
    structured(&client.call(session, "acquire_desktop", json!({"label": "desk"}))?)?;
    structured(&client.call(session, "launch", json!({"preset": "desk-entry"}))?)?;
    structured(&client.call(
        session,
        "wait_for",
        json!({"until": {"window": {"app_id": ENTRY}}, "timeout_ms": 15_000}),
    )?)?;
    let typed = client.call(
        session,
        "type_text",
        json!({"text": TYPED, "expect": {"app_id": ENTRY}}),
    )?;
    structured(&typed)?;
    let file = session.test_dir().root().join("entry-text");
    session.wait_until("desk-typed", "the entry's text", WAIT, |_| {
        Ok((fs::read_to_string(&file).ok().as_deref() == Some(TYPED)).then_some(()))
    })?;
    session.log(&format!(
        "desk: typed {TYPED:?} into a GTK 4 entry inside the desk"
    ))
}

/// `watch --desk` names the desk in its first line.
fn watches(session: &mut Session<'_>, server: &str) -> Result<()> {
    let log = session.artifact("watch-desk.log");
    let args = ["watch".into(), "--desk".into(), NAME.into()];
    let watch = session.start(server, &args, log.clone(), WAIT)?;
    let hello = session.wait_until("desk-watch", "watch's hello", WAIT, |_| {
        Ok(fs::read_to_string(&log)
            .ok()
            .and_then(|text| text.lines().next().map(str::to_owned))
            .and_then(|line| serde_json::from_str::<Value>(&line).ok()))
    })?;
    watch.stop()?;
    expect(
        field(&hello, "/desk") == NAME,
        "watch's hello naming the desk",
        &hello.to_string(),
    )?;
    session.log("desk: watch --desk named the desk")
}

/// The stop key on the host stops the desk's agent; `resume` lets it act again.
fn stops(session: &mut Session<'_>, server: &str, client: &mut Client) -> Result<()> {
    session.request(&Request::Action(Action::Spawn {
        command: vec![server.to_owned(), "stop".to_owned()],
    }))?;
    session.wait_until(
        "desk-stop",
        "the desk's server stopped, its lease given up",
        WAIT,
        |session| {
            let status = structured(&client.call(session, "status", json!({}))?)?;
            let stopped =
                field(&status, "/stop") == true && field(&status, "/lease/holder") == &Value::Null;
            Ok(stopped.then_some(()))
        },
    )?;
    let refused = report(&client.call(session, "acquire_desktop", json!({"label": "desk"}))?);
    expect(
        field(&refused, "/error") == "stopped",
        "the desk's server refusing the lease while stopped",
        &refused.to_string(),
    )?;
    session.log("desk: the host's stop key stopped the desk's agent")?;
    let resumed = session.run(server, &["resume".into()])?;
    expect(
        resumed.status.success(),
        "resume",
        &String::from_utf8_lossy(&resumed.stderr),
    )?;
    structured(&client.call(session, "acquire_desktop", json!({"label": "desk"}))?)?;
    structured(&client.call(session, "release_desktop", json!({"restore_focus": false}))?)?;
    session.log("desk: after resume, the desk's agent took the lease again")
}

/// Runs `desk <args>` to completion, failing with its output.
fn desk(session: &Session<'_>, server: &str, args: &[OsString]) -> Result<()> {
    let mut all: Vec<OsString> = vec!["desk".into()];
    all.extend_from_slice(args);
    let name = args.first().and_then(|arg| arg.to_str()).unwrap_or("desk");
    let log = session.artifact(&format!("desk-{name}.log"));
    let output = session
        .start(server, &all, log.clone(), DESK_DEADLINE)?
        .wait()?;
    if output.status.success() {
        return Ok(());
    }
    let said = fs::read_to_string(&log).unwrap_or_default();
    Err(Failure::new(format!(
        "desk {name} failed ({}): {said}",
        output.status
    )))
}

/// The policy: no lease notification, and the entry as a preset.
fn write_policy(session: &Session<'_>) -> Result<()> {
    let root = session.test_dir().root();
    let fixture = root.join("gtk.py");
    fs::write(&fixture, include_str!("../fixtures/gtk.py"))
        .context(format!("write {}", fixture.display()))?;
    let argv = [
        crate::environment::PYTHON,
        "-I",
        text(&fixture)?,
        text(root)?,
        "entry",
    ];
    let argv = serde_json::to_string(&argv).context("encode the preset")?;
    // An interpreter with arguments, and a preset's variables, need `unrestricted`. niri's
    // winit window in the headless cage shows GTK's Cairo drawing.
    let policy = format!(
        "{}unrestricted = true\n\n[[preset]]\nname = \"desk-entry\"\nargv = {argv}\napp_id = \"{ENTRY}\"\nenv = {{ GSK_RENDERER = \"cairo\" }}\n",
        crate::actions::QUIET
    );
    let dir = session.test_dir().config().join("niri-computer-use");
    fs::create_dir_all(&dir).context(format!("create {}", dir.display()))?;
    fs::write(dir.join("policy.toml"), &policy).context("write the policy file")?;
    fs::write(session.artifact("policy.toml"), policy).context("copy the policy file")
}

fn text(path: &Path) -> Result<&str> {
    path.to_str()
        .ok_or_else(|| Failure::new(format!("{} isn't UTF-8", path.display())))
}

fn desk_root(session: &Session<'_>) -> PathBuf {
    session.test_dir().run().join("ncu-d").join(NAME)
}

/// Copies the desk's logs into the artifacts, as far as they can be read.
fn copy_logs(session: &Session<'_>) {
    let logs = session
        .test_dir()
        .state()
        .join("niri-computer-use/desks")
        .join(NAME);
    for entry in fs::read_dir(logs).into_iter().flatten().flatten() {
        let name = format!("desk-{}", entry.file_name().to_string_lossy());
        fs::copy(entry.path(), session.artifact(&name)).ok();
    }
}

fn focused(session: &mut Session<'_>) -> Result<Option<u64>> {
    let Response::FocusedWindow(window) = session.request(&Request::FocusedWindow)? else {
        return Err(Failure::new(
            "niri answered FocusedWindow with another response",
        ));
    };
    Ok(window.map(|window| window.id))
}

fn workspaces(session: &mut Session<'_>) -> Result<Vec<Workspace>> {
    let Response::Workspaces(workspaces) = session.request(&Request::Workspaces)? else {
        return Err(Failure::new(
            "niri answered Workspaces with another response",
        ));
    };
    Ok(workspaces)
}

fn windows(session: &mut Session<'_>) -> Result<Vec<Window>> {
    let Response::Windows(windows) = session.request(&Request::Windows)? else {
        return Err(Failure::new("niri answered Windows with another response"));
    };
    Ok(windows)
}

fn expect(holds: bool, what: &str, saw: &str) -> Result<()> {
    if holds {
        Ok(())
    } else {
        Err(Failure::new(format!("expected {what}; saw {saw}")))
    }
}
