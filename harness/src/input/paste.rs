//! M7 `paste` into the GTK entry, read back from the entry, while a clipboard owner offers
//! three types with different contents: afterwards the same types offer the same bytes.
//! A focus mismatch and the stop flag refuse before the clipboard is touched: the owner
//! keeps the selection and the entry stays empty. With nothing copied, nothing is copied
//! afterwards.

use std::ffi::OsString;
use std::fs;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::{WAIT, stop};
use crate::clipboard::{CANCELLED, SECRET_HINT, TYPES};
use crate::failure::{Context as _, Failure, Result};
use crate::keyboard::CORPUS;
use crate::mcp::{Client, field, structured};
use crate::runner::Process;
use crate::session::Session;

use super::text_entry::{APP_ID, clear, entry_shows};

/// Over four times `type_text`'s limit.
const REPEATS: usize = 40;
const OWNER_DEADLINE: Duration = Duration::from_secs(60);
/// How long an emptied clipboard must stay empty.
const QUIET: Duration = Duration::from_secs(1);

/// Each type the clipboard offers, in order, with its bytes.
pub(super) type Offered = Vec<(String, Vec<u8>)>;

pub(super) fn run(
    session: &mut Session<'_>,
    client: &mut Client,
    server: &str,
    backend: &str,
) -> Result<()> {
    let text = CORPUS.repeat(REPEATS);
    let (owner, copied) = own(session, backend)?;
    refusals(session, client, server, &copied)?;
    let start = Instant::now();
    let pasted = paste(session, client, &text)?;
    entry_shows(session, &text)?;
    let elapsed = start.elapsed();
    expect_outcome(&pasted, "restored")?;
    // The keeper took the selection from the owner, which then exits.
    owner.wait()?;
    let after = offered(session)?;
    if after != copied {
        return Err(Failure::new(format!(
            "M7 paste {backend}: the clipboard after is {after:?}, not {copied:?}"
        )));
    }
    clear(session, client)?;
    session.log(&format!(
        "M7 paste {backend}: {} characters read back from the GTK entry {:.0} ms after the call began; the owner's {} types and their bytes offered again; focus_mismatch and stopped left the owner's selection and the entry untouched",
        text.chars().count(),
        elapsed.as_secs_f64() * 1000.0,
        copied.len()
    ))
}

/// Starts the clipboard owner and waits until the clipboard offers its types.
pub(super) fn own(session: &mut Session<'_>, backend: &str) -> Result<(Process, Offered)> {
    let owner = copy(session, backend, false)?;
    let expected = owners_copy();
    session.wait_until("m7-paste-owner", "the owner's clipboard", WAIT, |session| {
        // A type listed by the selection before the owner's can be gone by its read.
        Ok(offered(session)
            .is_ok_and(|offered| offered == expected)
            .then_some(()))
    })?;
    Ok((owner, expected))
}

/// What the clipboard owner offers.
pub(super) fn owners_copy() -> Offered {
    TYPES
        .iter()
        .map(|(mime, bytes)| ((*mime).to_owned(), bytes.to_vec()))
        .collect()
}

/// Starts the clipboard owner, which takes the selection, with its log named after `label`.
/// A `secret` owner also offers the password-manager hint set to `secret`.
pub(super) fn copy(session: &Session<'_>, label: &str, secret: bool) -> Result<Process> {
    let cancelled = session.test_dir().root().join(CANCELLED);
    if cancelled.exists() {
        fs::remove_file(&cancelled).context("remove the owner's cancel note")?;
    }
    let harness = std::env::current_exe().context("find the harness binary")?;
    let mut args: Vec<OsString> = vec![
        "clipboard".into(),
        session.test_dir().root().into(),
        OWNER_DEADLINE.as_millis().to_string().into(),
    ];
    if secret {
        args.push("--secret".into());
    }
    let program = harness
        .to_str()
        .ok_or_else(|| Failure::new("the harness path isn't UTF-8"))?;
    session.start(
        program,
        &args,
        session.artifact(&format!("clipboard-{label}.log")),
        OWNER_DEADLINE,
    )
}

/// A clipboard its owner marked secret can't be saved, so `paste` refuses with
/// `clipboard_unsaved` before any key: the owner keeps the selection and the entry stays
/// empty.
pub(super) fn secret(session: &mut Session<'_>, client: &mut Client) -> Result<()> {
    let owner = copy(session, "secret", true)?;
    let mut expected = owners_copy();
    expected.push((SECRET_HINT.0.to_owned(), SECRET_HINT.1.to_vec()));
    session.wait_until(
        "m7-paste-secret",
        "the secret owner's clipboard",
        WAIT,
        |session| Ok((offered(session)? == expected).then_some(())),
    )?;
    let args = json!({"text": "x", "keys": "ctrl+v", "expect": {"app_id": APP_ID}});
    refused(session, client, args, "clipboard_unsaved")?;
    entry_shows(session, "")?;
    if session.test_dir().root().join(CANCELLED).exists() || offered(session)? != expected {
        return Err(Failure::new(
            "M7 paste: a refused secret clipboard changed the clipboard",
        ));
    }
    owner.stop()?;
    session.log(
        "M7 paste with a clipboard marked secret: clipboard_unsaved, the owner kept the selection and the entry stayed empty",
    )
}

/// A wrong `expect` and the stop flag both refuse, and leave everything as it was.
fn refusals(
    session: &mut Session<'_>,
    client: &mut Client,
    server: &str,
    copied: &Offered,
) -> Result<()> {
    let elsewhere =
        json!({"text": "x", "keys": "ctrl+v", "expect": {"app_id": "org.ncu.Elsewhere"}});
    refused(session, client, elsewhere, "focus_mismatch")?;
    session.run(server, &["stop".into()])?;
    let here = json!({"text": "x", "keys": "ctrl+v", "expect": {"app_id": APP_ID}});
    refused(session, client, here, "stopped")?;
    stop::resume(session, client, server)?;
    entry_shows(session, "")?;
    if session.test_dir().root().join(CANCELLED).exists() || offered(session)? != *copied {
        return Err(Failure::new(
            "M7 paste: a refused paste changed the clipboard",
        ));
    }
    Ok(())
}

fn refused(session: &mut Session<'_>, client: &mut Client, args: Value, error: &str) -> Result<()> {
    let result = client.call(session, "paste", args)?;
    if field(&result, "/structuredContent/error") != error {
        return Err(Failure::new(format!(
            "M7 paste: expected {error}, saw {result}"
        )));
    }
    Ok(())
}

fn paste(session: &mut Session<'_>, client: &mut Client, text: &str) -> Result<Value> {
    let args = json!({"text": text, "keys": "ctrl+v", "expect": {"app_id": APP_ID}});
    structured(&client.call(session, "paste", args)?)
}

fn expect_outcome(result: &Value, clipboard: &str) -> Result<()> {
    let expected = json!({"read": true, "clipboard": clipboard});
    if field(result, "/observed") != "sent"
        || field(result, "/focus") != "matched"
        || field(result, "/paste") != &expected
    {
        return Err(Failure::new(format!("M7 paste: {result}")));
    }
    Ok(())
}

/// With nothing copied yet in the session, `paste` leaves nothing copied. Only before the
/// first copy: once something was copied, Noctalia's clipboard service adopts the last
/// selection whenever the clipboard goes empty. It ignores the pasted text, which carries
/// the password-manager hint, so it has nothing of it to adopt.
pub(super) fn empty(session: &mut Session<'_>, client: &mut Client, backend: &str) -> Result<()> {
    if !offered(session)?.is_empty() {
        return Err(Failure::new(
            "M7 paste: something was copied before the empty-clipboard check",
        ));
    }
    let pasted = paste(session, client, CORPUS)?;
    entry_shows(session, CORPUS)?;
    expect_outcome(&pasted, "cleared")?;
    session.still_absent(
        "m7-paste-empty",
        QUIET,
        || Ok(!offered(session)?.is_empty()),
    )?;
    clear(session, client)?;
    session.log(&format!(
        "M7 paste {backend} with nothing copied: read back from the GTK entry; the clipboard stayed empty for {} ms after",
        QUIET.as_millis()
    ))
}

/// The clipboard's types and each one's bytes, read with `wl-paste` in the nested
/// session; empty when nothing is copied.
pub(super) fn offered(session: &Session<'_>) -> Result<Offered> {
    listed(session)?
        .iter()
        .map(|mime| {
            let args = ["--no-newline".into(), "--type".into(), mime.into()];
            let output = session.run("wl-paste", &args)?;
            Ok((mime.clone(), output.stdout))
        })
        .collect()
}

/// The clipboard's types, without reading any of them; empty when nothing is copied.
pub(super) fn listed(session: &Session<'_>) -> Result<Vec<String>> {
    match session.run("wl-paste", &["--list-types".into()]) {
        Ok(output) => Ok(String::from_utf8(output.stdout)
            .context("wl-paste's types")?
            .lines()
            .map(str::to_owned)
            .collect()),
        Err(failure) if failure.to_string().contains("Nothing is copied") => Ok(Vec::new()),
        Err(failure) => Err(failure),
    }
}
