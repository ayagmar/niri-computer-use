//! A modal dialog that takes keyboard focus while `type_text` types: the GTK 4 `draft`
//! fixture opens one as soon as its entry first holds text. Until niri maps the dialog and
//! focuses it, niri sends the keys to the form, and GTK passes them on to its modal dialog,
//! which nothing outside the app can see; once niri has focused the dialog, it sends the
//! keys to the dialog's own surface, and the call must end `interrupted`, never `sent`,
//! whether or not niri's event stream has said so by the last key. Rounds run until niri
//! sent a key to the dialog's surface in one. Optional dev fixture, like A05's button.

use std::ffi::OsString;
use std::fs;
use std::time::Duration;

use serde_json::{Value, json};

use crate::failure::{Context as _, Failure, Result};
use crate::mcp::{Client, field, structured};
use crate::runner::Process;
use crate::session::Session;

const CHECK: &str = "M7 modal mid-type";
const APP_ID: &str = "org.ncu.Draft";
/// Repeated `REPEATS` times, 990 characters, near the 1000 typed text may be, so that niri
/// may map the dialog and give it focus while the keys still go out. GTK maps it only once
/// it has handled the keys already sent, so not every round gets that far.
const SENTENCE: &str = "Meeting moved to 3pm. ";
const REPEATS: usize = 45;
/// How many rounds may end before niri sent a key to the dialog's surface.
const ROUNDS: usize = 12;
/// How long the fixture runs before it quits by itself.
const FIXTURE_SECONDS: u64 = 60;
const READY: Duration = Duration::from_secs(15);
const DISMISSED: Duration = Duration::from_secs(5);

pub(super) fn run(session: &mut Session<'_>, client: &mut Client) -> Result<()> {
    if !super::activation::gtk4(session, CHECK)? {
        return Ok(());
    }
    let fixture = session.test_dir().root().join("workflow.py");
    fs::write(&fixture, include_str!("../../fixtures/workflow.py"))
        .context(format!("write {}", fixture.display()))?;
    structured(&client.call(session, "acquire_desktop", json!({}))?)?;
    for round in 1..=ROUNDS {
        let (keys, typed) = self::round(session, client, round)?;
        if keys.to_dialog > 0 {
            structured(&client.call(
                session,
                "release_desktop",
                json!({"restore_focus": false}),
            )?)?;
            return session.log(&format!(
                "{CHECK}: in round {round}, niri sent {} keys to the dialog's surface and type_text ended interrupted {}; before it, {} rounds where GTK passed all of the dialog's keys on from the form, which type_text can't see",
                keys.to_dialog,
                typed.map_or_else(
                    || "after its last key".to_owned(),
                    |typed| format!("after {typed} characters")
                ),
                round - 1
            ));
        }
    }
    Err(Failure::new(format!(
        "{CHECK}: in none of {ROUNDS} rounds did niri send a key to the dialog's surface, so nothing was raced"
    )))
}

/// One fixture run: types the message and dismisses the dialog. Every key niri sent to the
/// dialog's surface must make the call `interrupted`; a call whose keys GTK passed on from
/// the form must at least report the dialog opening and taking focus in `changes`. Returns
/// the keys and, for a call that stopped early, how many characters it typed.
fn round(
    session: &mut Session<'_>,
    client: &mut Client,
    round: usize,
) -> Result<(DialogKeys, Option<u64>)> {
    let events = session.test_dir().root().join("draft-events.jsonl");
    if events.exists() {
        fs::remove_file(&events).context(format!("remove {}", events.display()))?;
    }
    let process = start(session, round)?;
    let window = draft_window(session, client, &process)?;
    let result = structured(&client.call(
        session,
        "type_text",
        json!({"text": SENTENCE.repeat(REPEATS), "expect": {"window_id": window}}),
    )?)?;
    let keys = dialog_keys(session)?;
    let detail = field(&result, "/detail").as_str().unwrap_or_default();
    let interrupted =
        field(&result, "/observed") == "interrupted" && detail.contains("may have reached");
    let reported = field(&result, "/changes/focus/from") == window
        && field(&result, "/changes/opened/0/title") == "Autosave failed";
    if keys.to_dialog + keys.to_form == 0 || keys.to_dialog > 0 && !interrupted || !reported {
        return Err(Failure::new(format!(
            "{CHECK} round {round}: niri sent {} keys to the dialog and GTK passed on {} from the form, and type_text answered {result}",
            keys.to_dialog, keys.to_form
        )));
    }
    dismiss(session, client, window)?;
    process.stop()?;
    Ok((keys, field(&result, "/typed").as_u64()))
}

fn start(session: &Session<'_>, round: usize) -> Result<Process> {
    let root = session.test_dir().root();
    let args: Vec<OsString> = vec![
        "-I".into(),
        root.join("workflow.py").into(),
        root.into(),
        "draft".into(),
        FIXTURE_SECONDS.to_string().into(),
    ];
    let log = session.artifact(&format!("draft-{round}.log"));
    session.start(
        crate::environment::PYTHON,
        &args,
        log,
        Duration::from_secs(FIXTURE_SECONDS + 15),
    )
}

/// The Draft window of `process`, once it has keyboard focus.
fn draft_window(session: &mut Session<'_>, client: &mut Client, process: &Process) -> Result<u64> {
    let pid = process.pid();
    session.wait_until(
        "modal-ready",
        "the focused Draft window",
        READY,
        |session| {
            let desktop = structured(&client.call(session, "desktop_state", json!({}))?)?;
            let focused = field(&desktop, "/focused_window").as_u64();
            let ours = field(&desktop, "/windows").as_array().and_then(|windows| {
                windows.iter().find(|window| {
                    field(window, "/id").as_u64() == focused
                        && field(window, "/app_id") == APP_ID
                        && field(window, "/pid").as_i64() == Some(i64::from(pid))
                })
            });
            Ok(ours.and(focused))
        },
    )
}

/// The fixture's records, in order.
fn events(session: &Session<'_>) -> Result<Vec<Value>> {
    let path = session.test_dir().root().join("draft-events.jsonl");
    let text = fs::read_to_string(&path).context(format!("read {}", path.display()))?;
    text.lines()
        .map(|line| serde_json::from_str(line).context(format!("parse {line:?}")))
        .collect()
}

/// The keys the dialog got: how many niri sent to the dialog's surface, and how many to the
/// form's, which GTK passed on to its modal dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DialogKeys {
    to_dialog: usize,
    to_form: usize,
}

fn dialog_keys(session: &Session<'_>) -> Result<DialogKeys> {
    let events = events(session)?;
    let count = |to: &str| {
        events
            .iter()
            .filter(|event| field(event, "/event") == "dialog-key" && field(event, "/to") == to)
            .count()
    };
    Ok(DialogKeys {
        to_dialog: count("dialog"),
        to_form: count("form"),
    })
}

/// Escape in the dialog, once niri has it focused instead of the form `window`, closes it.
fn dismiss(session: &mut Session<'_>, client: &mut Client, window: u64) -> Result<()> {
    let dialog = session.wait_until("modal-focused", "the focused dialog", READY, |session| {
        let desktop = structured(&client.call(session, "desktop_state", json!({}))?)?;
        let focused = field(&desktop, "/focused_window").as_u64();
        Ok(focused.filter(|&id| id != window))
    })?;
    structured(&client.call(
        session,
        "key",
        json!({"keys": ["Escape"], "expect": {"window_id": dialog}}),
    )?)?;
    session.wait_until(
        "modal-dismissed",
        "the dialog dismissed",
        DISMISSED,
        |session| {
            Ok(events(session)?
                .iter()
                .any(|event| field(event, "/event") == "dismissed")
                .then_some(()))
        },
    )
}
