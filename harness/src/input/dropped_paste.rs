//! A `paste` whose call goes away once its key is on its way: by a stop, by the client
//! cancelling the request and by the client's end. The key goes out at once and the
//! entry's process is stopped for a second, so the application reads the clipboard late,
//! and only a keeper that still waits for the read passes. The GTK entry must then hold
//! the agent's text, never the user's copy the keeper saved, and the clipboard must offer
//! the user's copy again afterwards.
//!
//! A server that dies after its keeper admitted the key can't say whether the key went
//! out, so the keeper must keep offering the agent's text, until the next paste replaces
//! it and clears the clipboard rather than put it back. A native key goes out right after
//! the admission, so the death in between is played by ending a keeper's stdin directly
//! (see `keeper::gone_after_k`).

use std::time::Duration;

use serde_json::json;

use super::paste::{Offered, listed, offered, own, withheld};
use super::text_entry::{APP_ID, clear, entry_shows};
use super::{SERVER_DEADLINE, WAIT, keeper, stop};
use crate::clipboard::SECRET_HINT;
use crate::failure::{Failure, Result};
use crate::mcp::{Client, field, structured};
use crate::runner::Process;
use crate::session::{self, Session};

const TEXT: &str = "The agent's text, never the user's";
/// What a paste after a `kept` outcome pastes.
const AFTER: &str = "A later paste replaces the kept text";
/// The type the keeper marks its text with, beside the text.
const PASTE_TYPE: &str = "application/x-niri-computer-use-paste";
/// Well inside the keeper's read wait, for a clipboard that must stay as it is.
const BRIEF: Duration = Duration::from_millis(300);
/// From the paste call to its going away, once the key went out.
const GONE: Duration = Duration::from_millis(300);
/// How long the entry's process is stopped, inside the keeper's read wait.
const LATE: Duration = Duration::from_secs(1);
/// Past the keeper's read wait and quiet time, for a clipboard that must not be restored.
const KEPT: Duration = Duration::from_secs(3);

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
    let client = start(session, server)?;
    late(session, client, server, entry)?;
    clear_as(session, owner)?;
    let keeper = died(session, server)?;
    after_kept(session, owner)?;
    keeper.wait()?;
    session.log(&format!(
        "M7 paste dropped once its key was on its way: stop, cancel and the client's end with the entry's read {} ms late left the agent's text in the GTK entry and the user's copy offered again; a keeper whose server ended after it admitted the key kept the agent's text on the clipboard for {} ms, and the next paste replaced it and dropped it",
        LATE.as_millis(),
        KEPT.as_millis()
    ))
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

/// A keeper whose server ended after it admitted the key, with the user's copy saved:
/// the clipboard must keep offering the agent's text, marked, past the keeper's read wait.
/// Returns the keeper, which serves until the next paste takes the selection.
fn died(session: &mut Session<'_>, server: &str) -> Result<Process> {
    let (owner, _) = own(session, "died")?;
    let keeper = keeper::gone_after_k(session, server, TEXT.as_bytes(), "died")?;
    // The keeper took the selection, which ends the owner.
    owner.wait()?;
    session.still_absent("m7-dropped-kept", KEPT, || {
        Ok(!listed(session)?.iter().any(|mime| mime == SECRET_HINT.0))
    })?;
    let text = offered(session)?
        .into_iter()
        .find(|(mime, _)| mime == "text/plain;charset=utf-8")
        .map(|(_, bytes)| bytes);
    if text.as_deref() != Some(TEXT.as_bytes()) {
        return Err(Failure::new(format!(
            "M7 dropped paste: the clipboard after the server's end offers {text:?}, not the agent's text"
        )));
    }
    Ok(keeper)
}

/// A paste after `died`'s `kept`: the dead server's keeper still offers its text, copied
/// before this lease, which `clipboard_read` withholds, and the next paste replaces rather
/// than refuses, and clears afterwards rather than put back.
fn after_kept(session: &mut Session<'_>, owner: &mut Client) -> Result<()> {
    structured(&owner.call(session, "acquire_desktop", json!({}))?)?;
    if !listed(session)?.iter().any(|mime| mime == PASTE_TYPE) {
        return Err(Failure::new(
            "M7 paste after kept: the kept text isn't marked as a paste's",
        ));
    }
    withheld(session, owner, TEXT, "copied_before_lease")?;
    let args = json!({"text": AFTER, "via": "paste", "paste_keys": "ctrl+v", "expect": {"app_id": APP_ID}});
    let pasted = structured(&owner.call(session, "type_text", args)?)?;
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

/// A server with the native keyboard, holding the lease.
fn start(session: &mut Session<'_>, server: &str) -> Result<Client> {
    let mut client = Client::start(session, server, "harness-m7-late-paste", SERVER_DEADLINE)?;
    structured(&client.call(session, "acquire_desktop", json!({}))?)?;
    Ok(client)
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
    fn paste(client: &mut Client, owner: Process, copied: Offered) -> Result<Self> {
        let id = client.start_call(
            "type_text",
            json!({"text": TEXT, "via": "paste", "paste_keys": "ctrl+v", "expect": {"app_id": APP_ID}}),
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
}

/// Empties the entry with `owner`'s lease, then gives it back.
fn clear_as(session: &mut Session<'_>, owner: &mut Client) -> Result<()> {
    structured(&owner.call(session, "acquire_desktop", json!({}))?)?;
    clear(session, owner)?;
    structured(&owner.call(session, "release_desktop", json!({"restore_focus": false}))?).map(drop)
}
