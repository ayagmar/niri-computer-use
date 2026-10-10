//! A `paste` whose call goes away once its key may be on its way: by a stop, by the
//! client cancelling the request and by the client's end, with wtype and natively, and by
//! the server's or engine's death while wtype survives it. The GTK entry must then hold
//! the agent's text, never the user's copy the keeper saved. For a call that goes away,
//! the clipboard must offer the user's copy again afterwards; for a dead server, which
//! can't say whether the key went out, it must keep offering the agent's text, until the
//! next paste replaces it and clears the clipboard rather than put it back.
//!
//! The call goes away while the key is late in one of two ways. A `wtype` wrapper holds
//! the key back past the keeper's two-second read wait, so only a keeper whose end waits
//! for the key passes. Or the key goes out at once and the entry's process is stopped
//! for a second, so the application reads the clipboard late, and only a keeper that
//! still waits for the read passes.
//!
//! The wrapper is reached through `PATH`. In shared runs the engine is killed first, so
//! the wrapper's client starts the next one with its `PATH`.

use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::json;

use super::paste::{Offered, listed, offered, own};
use super::text_entry::{APP_ID, clear, entry_shows};
use super::{SERVER_DEADLINE, WAIT, crash, guardian, stop};
use crate::clipboard::SECRET_HINT;
use crate::failure::{Context as _, Failure, Result};
use crate::mcp::{Client, field, structured};
use crate::runner::Process;
use crate::session::{self, Session};

const TEXT: &str = "The agent's text, never the user's";
/// What a paste after a `kept` outcome pastes.
const AFTER: &str = "A later paste replaces the kept text";
/// The type the keeper marks its text with, beside the text.
const PASTE_TYPE: &str = "application/x-niri-computer-use-paste";
/// Past the keeper's two-second read wait, and inside wtype's three-second deadline.
const HELD: Duration = Duration::from_millis(2200);
/// About what a real `wtype` takes, well inside the keeper's read wait.
const BRIEF: Duration = Duration::from_millis(300);
/// From the paste call to its going away, once the key went out.
const GONE: Duration = Duration::from_millis(300);
/// How long the entry's process is stopped, inside the keeper's read wait.
const LATE: Duration = Duration::from_secs(1);
/// Past the keeper's read wait and quiet time, for a clipboard that must not be restored.
const KEPT: Duration = Duration::from_secs(3);
/// The wrapper's own bound on its wait, inside wtype's deadline.
const WRAPPER_POLLS: u32 = 140;
const STARTED: &str = "wtype-started";
const GO: &str = "wtype-go";

/// How a call goes away.
#[derive(Clone, Copy)]
enum Gone {
    Stopped,
    Cancelled,
    Ended,
}

pub(super) fn run(
    session: &mut Session<'_>,
    owner: &mut Client,
    server: &str,
    entry: i32,
) -> Result<()> {
    let wrapper = wrapper(session)?;
    let shared = end_engine(session, owner)?;
    let client = start(session, server, &wrapper, "harness-m7-dropped-paste")?;
    if shared {
        guardian::reconnect(session, owner)?;
    }
    held(session, client, server)?;
    clear_as(session, owner)?;
    let wtype = start(session, server, &wrapper, "harness-m7-late-paste")?;
    late(session, wtype, server, entry)?;
    clear_as(session, owner)?;
    let native = start_native(session, server)?;
    late(session, native, server, entry)?;
    clear_as(session, owner)?;
    died(session, owner, server, &wrapper)?;
    clear_as(session, owner)?;
    after_kept(session, owner)?;
    session.log(&format!(
        "M7 paste dropped once its key may be on its way: stop, cancel and the client's end with the wtype key held {} ms, and with wtype and natively with the entry's read {} ms late, left the agent's text in the GTK entry and the user's copy offered again; the {} death with the key held {} ms left the agent's text in the entry and on the clipboard, and the next paste replaced it and dropped it",
        HELD.as_millis(),
        LATE.as_millis(),
        if shared { "engine's" } else { "server's" },
        BRIEF.as_millis()
    ))
}

/// The wtype key held in the wrapper while the call goes away each way.
fn held(session: &mut Session<'_>, mut client: Client, server: &str) -> Result<()> {
    for gone in [Gone::Stopped, Gone::Cancelled, Gone::Ended] {
        let pasting = Pasting::start(session, &mut client, true)?;
        let id = pasting.id;
        go_away(session, &mut client, server, gone, id)?;
        session::pause(HELD);
        let_go(session)?;
        pasting.restored(session)?;
        came_back(session, &mut client, server, gone, id)?;
        if !matches!(gone, Gone::Ended) {
            clear(session, &mut client)?;
        }
    }
    client.wait()
}

/// The key goes out at once, and the entry's process reads the clipboard late, while the
/// call goes away each way.
fn late(session: &mut Session<'_>, mut client: Client, server: &str, entry: i32) -> Result<()> {
    for gone in [Gone::Stopped, Gone::Cancelled, Gone::Ended] {
        let (owner, copied) = own(session, &format!("late-{}", client.pid()))?;
        signal(session, "-STOP", entry)?;
        let pasting = Pasting::paste(&mut client, owner, copied)?;
        let id = pasting.id;
        session::pause(GONE);
        go_away(session, &mut client, server, gone, id)?;
        session::pause(LATE.saturating_sub(GONE));
        signal(session, "-CONT", entry)?;
        pasting.restored(session)?;
        came_back(session, &mut client, server, gone, id)?;
        if !matches!(gone, Gone::Ended) {
            clear(session, &mut client)?;
        }
    }
    client.wait()
}

fn go_away(
    session: &Session<'_>,
    client: &mut Client,
    server: &str,
    gone: Gone,
    id: u64,
) -> Result<()> {
    match gone {
        Gone::Stopped => session.run(server, &["stop".into()]).map(drop),
        Gone::Cancelled => client.cancel(id),
        Gone::Ended => client.close_input(),
    }
}

/// After a stop, the call said `stopped`, and once its input is done the lease is taken
/// again after `resume`.
fn came_back(
    session: &mut Session<'_>,
    client: &mut Client,
    server: &str,
    gone: Gone,
    id: u64,
) -> Result<()> {
    if !matches!(gone, Gone::Stopped) {
        return Ok(());
    }
    let result = client.result(session, id)?;
    if field(&result, "/structuredContent/error") != "stopped" {
        return Err(Failure::new(format!(
            "M7 dropped paste: the stop didn't stop it: {result}"
        )));
    }
    stop::marker_gone(session)?;
    stop::resume(session, client, server)
}

/// The server or engine dies with wtype held in the wrapper, leaving its marker for
/// `recover`, which `owner` runs.
fn died(session: &mut Session<'_>, owner: &mut Client, server: &str, wrapper: &Path) -> Result<()> {
    let mut dying = start(session, server, wrapper, "harness-m7-dying-paste")?;
    let pasting = Pasting::start(session, &mut dying, true)?;
    guardian::kill(session, dying)?;
    session::pause(BRIEF);
    let_go(session)?;
    pasting.kept(session)?;
    guardian::reconnect(session, owner)?;
    crash::recover(session, owner, server, "has already exited")
}

/// A paste after `died`'s `kept`: the dead server's keeper still offers its text, marked
/// as a secret, which the next paste replaces rather than refuses, and clears afterwards
/// rather than put back.
fn after_kept(session: &mut Session<'_>, owner: &mut Client) -> Result<()> {
    structured(&owner.call(session, "acquire_desktop", json!({}))?)?;
    if !listed(session)?.iter().any(|mime| mime == PASTE_TYPE) {
        return Err(Failure::new(
            "M7 paste after kept: the kept text isn't marked as a paste's",
        ));
    }
    let args = json!({"text": AFTER, "keys": "ctrl+v", "expect": {"app_id": APP_ID}});
    let pasted = structured(&owner.call(session, "paste", args)?)?;
    entry_shows(session, AFTER)?;
    let dropped = field(&pasted, "/detail")
        .as_str()
        .is_some_and(|detail| detail.contains("earlier paste's text"));
    if field(&pasted, "/paste") != &json!({"read": true, "clipboard": "cleared"}) || !dropped {
        return Err(Failure::new(format!("M7 paste after kept: {pasted}")));
    }
    session.still_absent("m7-after-kept", BRIEF, || {
        Ok(listed(session)?.iter().any(|mime| mime == PASTE_TYPE))
    })?;
    clear(session, owner)?;
    structured(&owner.call(session, "release_desktop", json!({"restore_focus": false}))?).map(drop)
}

/// In a shared run, kills the engine `owner` reaches, so the next server starts one with
/// its own `PATH`. Returns whether the run is shared.
fn end_engine(session: &mut Session<'_>, owner: &mut Client) -> Result<bool> {
    let status = structured(&owner.call(session, "status", json!({}))?)?;
    if field(&status, "/engine/mode") != "shared" {
        return Ok(false);
    }
    let pid = field(&status, "/engine/pid")
        .as_u64()
        .ok_or_else(|| Failure::new(format!("status names no engine: {status}")))?;
    guardian::kill_engine(pid)?;
    Ok(true)
}

/// A server whose `wtype` is the wrapper in `wrapper`, holding the lease.
fn start(session: &mut Session<'_>, server: &str, wrapper: &Path, name: &str) -> Result<Client> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let mut assignment = OsString::from("PATH=");
    assignment.push(wrapper);
    assignment.push(":");
    assignment.push(path);
    let args = [assignment, server.into(), "serve".into()];
    let mut client = Client::start_command(session, "env", &args, name, SERVER_DEADLINE)?;
    structured(&client.call(session, "acquire_desktop", json!({}))?)?;
    Ok(client)
}

/// A server with the native keyboard, holding the lease.
fn start_native(session: &mut Session<'_>, server: &str) -> Result<Client> {
    let args = [
        "NIRI_COMPUTER_USE_KEYBOARD=native".into(),
        server.into(),
        "serve".into(),
    ];
    let mut client = Client::start_command(
        session,
        "env",
        &args,
        "harness-m7-late-native",
        SERVER_DEADLINE,
    )?;
    structured(&client.call(session, "acquire_desktop", json!({}))?)?;
    Ok(client)
}

/// Writes the wrapper, which notes that it started, waits for `GO` up to its bound, and
/// runs the real `wtype`. `GO` exists between checks, so other keys aren't held.
fn wrapper(session: &Session<'_>) -> Result<PathBuf> {
    let root = session.test_dir().root();
    let real = std::env::var_os("PATH")
        .and_then(|path| {
            std::env::split_paths(&path)
                .map(|dir| dir.join("wtype"))
                .find(|program| program.is_file())
        })
        .ok_or_else(|| Failure::new("wtype isn't on PATH"))?;
    let dir = root.join("slow-wtype");
    fs::create_dir_all(&dir).context("create the wtype wrapper's directory")?;
    let (started, go) = (root.join(STARTED), root.join(GO));
    let script = format!(
        "#!/bin/sh\n: > '{}'\ni=0\nwhile [ ! -e '{}' ] && [ \"$i\" -lt {WRAPPER_POLLS} ]; do sleep 0.02; i=$((i + 1)); done\nexec '{}' \"$@\"\n",
        started.display(),
        go.display(),
        real.display()
    );
    let program = dir.join("wtype");
    fs::write(&program, script).context("write the wtype wrapper")?;
    fs::set_permissions(&program, fs::Permissions::from_mode(0o700))
        .context("make the wtype wrapper executable")?;
    let_go(session)?;
    Ok(dir)
}

fn let_go(session: &Session<'_>) -> Result<()> {
    fs::write(session.test_dir().root().join(GO), "").context("let the wtype wrapper go")
}

/// Stops or continues the entry's process.
fn signal(session: &Session<'_>, signal: &str, pid: i32) -> Result<()> {
    session
        .run("kill", &[signal.into(), pid.to_string().into()])
        .map(drop)
}

/// A paste through `client`, with the user's copy on the clipboard before it.
struct Pasting {
    owner: Process,
    copied: Offered,
    id: u64,
}

impl Pasting {
    /// Copies the user's copy, then pastes; with `held`, the key waits in the wrapper.
    fn start(session: &mut Session<'_>, client: &mut Client, held: bool) -> Result<Self> {
        let (owner, copied) = own(session, &format!("dropped-{}", client.pid()))?;
        if !held {
            return Self::paste(client, owner, copied);
        }
        let root = session.test_dir().root().to_owned();
        let started = root.join(STARTED);
        if started.exists() {
            fs::remove_file(&started).context("remove the wrapper's note")?;
        }
        fs::remove_file(root.join(GO)).context("hold the wtype wrapper")?;
        let pasting = Self::paste(client, owner, copied)?;
        session.wait_until(
            "m7-dropped-held",
            "the key held in the wrapper",
            WAIT,
            |_| Ok(started.exists().then_some(())),
        )?;
        Ok(pasting)
    }

    fn paste(client: &mut Client, owner: Process, copied: Offered) -> Result<Self> {
        let id = client.start_call(
            "paste",
            json!({"text": TEXT, "keys": "ctrl+v", "expect": {"app_id": APP_ID}}),
        )?;
        Ok(Self { owner, copied, id })
    }

    /// The entry holds the agent's text, and the clipboard offers the user's copy again.
    fn restored(self, session: &mut Session<'_>) -> Result<()> {
        entry_shows(session, TEXT)?;
        // The keeper took the selection before the key, which ends the owner.
        self.owner.wait()?;
        // Only the types are listed until then: the keeper counts every read of the text as
        // the paste's, and waits for them to stop before it restores.
        let types: Vec<String> = self.copied.iter().map(|(mime, _)| mime.clone()).collect();
        session.wait_until(
            "m7-dropped-restored",
            "the user's copy offered again",
            WAIT,
            |session| Ok((listed(session)? == types).then_some(())),
        )?;
        let after = offered(session)?;
        if after != self.copied {
            return Err(Failure::new(format!(
                "M7 dropped paste: the clipboard after is {after:?}, not {:?}",
                self.copied
            )));
        }
        Ok(())
    }

    /// The entry holds the agent's text, and the clipboard keeps offering it, marked, past
    /// the keeper's read wait.
    fn kept(self, session: &mut Session<'_>) -> Result<()> {
        entry_shows(session, TEXT)?;
        self.owner.wait()?;
        session.still_absent("m7-dropped-kept", KEPT, || {
            Ok(!listed(session)?.iter().any(|mime| mime == SECRET_HINT.0))
        })?;
        let text = offered(session)?
            .into_iter()
            .find(|(mime, _)| mime == "text/plain;charset=utf-8")
            .map(|(_, bytes)| bytes);
        if text.as_deref() != Some(TEXT.as_bytes()) {
            return Err(Failure::new(format!(
                "M7 dropped paste: the clipboard after the death offers {text:?}, not the agent's text"
            )));
        }
        Ok(())
    }
}

/// Empties the entry with `owner`'s lease, then gives it back.
fn clear_as(session: &mut Session<'_>, owner: &mut Client) -> Result<()> {
    structured(&owner.call(session, "acquire_desktop", json!({}))?)?;
    clear(session, owner)?;
    structured(&owner.call(session, "release_desktop", json!({"restore_focus": false}))?).map(drop)
}
