//! A background desk inside the nested niri: `desk start` makes one, a server on it with
//! `serve --desk` types into a GTK 4 entry launched inside it from a preset, and the
//! nested niri, the desk's host, never sees the window, the focus change or a workspace
//! change. The nested Noctalia is the host's lock source. The stop key, sent as niri's
//! `spawn` of `stop` on the host as the keybind sends it, stops the desk's agent, and
//! `resume` lets it act again. `watch --desk` names the desk. Everything the desk makes is
//! under `TEST_DIR`; `desk stop` removes it, also when a check fails.
//!
//! The entry leaves a process behind that ignores the display, double-forked under its own
//! session. It is gone after the orderly `desk stop`; and on a second start, it outlives a
//! `SIGKILL` of the supervisor and is gone after `desk stop`, which stops the desk's
//! systemd scope. A desk needs the user's systemd manager, which the nested session's
//! runtime directory reaches through a link to its private socket; the desk's scopes are
//! the only units this makes, and `desk stop` removes each.

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
    user_manager(session)?;
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
    orphan_gone(session, 1, "the orderly desk stop")?;
    session.log("desk: desk stop removed the desk and what its app left")?;
    killed(session, server)?;
    noctalia.stop().map(drop)
}

/// Links the nested runtime directory's `systemd/private` to the user's systemd manager's,
/// which `systemd-run --user --scope` and `systemctl --user` reach first.
fn user_manager(session: &Session<'_>) -> Result<()> {
    let uid = rustix::process::geteuid().as_raw();
    let private = PathBuf::from(format!("/run/user/{uid}/systemd/private"));
    if !private.exists() {
        return Err(Failure::new(format!(
            "a desk runs in a systemd user scope, and {} isn't there: no user manager",
            private.display()
        )));
    }
    let dir = session.test_dir().run().join("systemd");
    fs::create_dir_all(&dir).context(format!("create {}", dir.display()))?;
    std::os::unix::fs::symlink(&private, dir.join("private"))
        .context(format!("link {}", dir.join("private").display()))
}

/// A second desk whose supervisor is killed: the entry's orphan outlives it, since
/// nothing but the scope holds it, and `desk stop` stops all of it.
fn killed(session: &mut Session<'_>, server: &str) -> Result<()> {
    desk(
        session,
        server,
        &["start".into(), "--size".into(), SIZE.into(), NAME.into()],
    )?;
    let checked = orphan_left(session, server).and_then(|()| kill_supervisor(session));
    let stopped = desk(session, server, &["stop".into(), NAME.into()]);
    copy_logs(session);
    checked?;
    stopped?;
    expect(
        !desk_root(session).exists(),
        "desk stop to remove a desk whose supervisor was killed",
        &desk_root(session).display().to_string(),
    )?;
    orphan_gone(session, 2, "desk stop after its supervisor was killed")?;
    session.log("desk: desk stop stopped a desk whose supervisor was killed, orphan and all")
}

/// Launches the entry on the running desk, which leaves its second orphan.
fn orphan_left(session: &mut Session<'_>, server: &str) -> Result<()> {
    let args = ["serve".into(), "--desk".into(), NAME.into()];
    let mut client = Client::start_command(session, server, &args, "desk-killed", CLIENT_DEADLINE)?;
    ready(session, &mut client)?;
    structured(&client.call(session, "acquire_desktop", json!({"label": "desk"}))?)?;
    structured(&client.call(session, "launch", json!({"preset": "desk-entry"}))?)?;
    structured(&client.call(
        session,
        "wait_for",
        json!({"until": {"window": {"app_id": ENTRY}}, "timeout_ms": 15_000}),
    )?)?;
    structured(&client.call(session, "release_desktop", json!({"restore_focus": false}))?)?;
    client.stop()?;
    orphan(session, 2).map(drop)
}

/// Kills the desk's supervisor and waits until it is gone; the entry's orphan lives on.
fn kill_supervisor(session: &mut Session<'_>) -> Result<()> {
    let file = desk_root(session).join("desk.json");
    let text = fs::read_to_string(&file).context(format!("read {}", file.display()))?;
    let record: Value = serde_json::from_str(&text).context("parse the desk's record")?;
    let pid = record
        .pointer("/supervisor/pid")
        .and_then(Value::as_i64)
        .and_then(|pid| i32::try_from(pid).ok())
        .and_then(rustix::process::Pid::from_raw)
        .ok_or_else(|| Failure::new(format!("no supervisor PID in {text}")))?;
    rustix::process::kill_process(pid, rustix::process::Signal::KILL)
        .context("kill the desk's supervisor")?;
    let raw = pid.as_raw_nonzero().get().unsigned_abs();
    session.wait_until("desk-killed", "the killed supervisor gone", WAIT, |_| {
        Ok((!alive(raw)).then_some(()))
    })?;
    let orphan = orphan(session, 2)?;
    expect(
        alive(orphan),
        "the entry's orphan to outlive its supervisor's SIGKILL",
        &orphan.to_string(),
    )?;
    session.log(&format!(
        "desk: killed the supervisor (PID {raw}); the entry's orphan (PID {orphan}) lives on"
    ))
}

/// The PID of the entry's `nth` orphan, once it has written it.
fn orphan(session: &mut Session<'_>, nth: usize) -> Result<u32> {
    let file = session.test_dir().root().join("orphans");
    session.wait_until("desk-orphan", "the entry's orphan", WAIT, |_| {
        Ok(fs::read_to_string(&file)
            .ok()
            .and_then(|text| text.lines().nth(nth - 1).map(str::to_owned))
            .and_then(|pid| pid.trim().parse().ok()))
    })
}

/// The entry's `nth` orphan is gone after `what`.
fn orphan_gone(session: &mut Session<'_>, nth: usize, what: &str) -> Result<()> {
    let pid = orphan(session, nth)?;
    expect(
        !alive(pid),
        &format!("the entry's orphan gone after {what}"),
        &format!("PID {pid} still runs"),
    )
}

/// Whether `pid` runs: its `/proc` entry exists and it isn't a zombie.
fn alive(pid: u32) -> bool {
    fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|line| {
        line.rsplit_once(") ")
            .is_some_and(|(_, rest)| !rest.starts_with('Z'))
    })
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
        "--orphan",
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
