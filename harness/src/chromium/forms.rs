//! `--chromium`'s form fields: short numbers typed and pasted into each field of
//! `fixtures/chromium-forms.html`, a page of its own for each, read back as the text area
//! is. Typing must arrive exactly except in the masked field, which is known to keep only
//! some keys, the first among them; pasting must arrive exactly everywhere.

use serde_json::json;

use super::{Launch, Page, TERM_GRACE, arrived, clear, shown};
use crate::failure::Result;
use crate::mcp::Client;
use crate::session::Session;

const TEXTS: [&str; 4] = ["100", "25", "10", "1234567890"];

/// What the project says typed text arrives as in a field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Typed {
    Exact,
    /// Some keys are undone, never the first: a mask that needs one of the page's timers
    /// to run between two inputs, which Chromium holds back until the page's next frame,
    /// while native keys arrive well within one.
    FirstKeys,
}

/// Each name also names its page, profile and log, short enough for Chromium's singleton
/// socket under `TEST_DIR/<name>-tmp` to fit a Unix socket's 108 bytes.
const FIELDS: [(&str, Typed); 3] = [
    ("number", Typed::Exact),
    ("controlled", Typed::Exact),
    ("masked", Typed::FirstKeys),
];

/// Types and pastes every text into every field. Returns the promised cases that arrived
/// otherwise.
pub(super) fn run(
    session: &mut Session<'_>,
    client: &mut Client,
    browser: &str,
) -> Result<Vec<String>> {
    let mut differed = Vec::new();
    for (name, typed) in FIELDS {
        let query = format!("?field={name}");
        let launch = Launch {
            name,
            html: include_str!("../../fixtures/chromium-forms.html"),
            query: &query,
            env: &[],
            flags: &[],
        };
        let mut process = launch.start(session, browser)?;
        let page = Page::open(session, &mut process, name)?;
        for text in TEXTS {
            differed.extend(typed_case(session, client, &page, (name, typed), text)?);
            differed.extend(pasted_case(session, client, &page, name, text)?);
        }
        process.terminate(TERM_GRACE)?;
    }
    Ok(differed)
}

/// Types `text` into the field, logs what arrived and empties it. Returns the failure
/// when it broke the promise.
fn typed_case(
    session: &mut Session<'_>,
    client: &mut Client,
    page: &Page,
    (name, typed): (&str, Typed),
    text: &str,
) -> Result<Option<String>> {
    let offset = page.log_len()?;
    let received = arrived(session, client, page, json!({"text": text}))?;
    let undone = page.read_log(offset)?.matches("\"ncu-undone ").count();
    let (outcome, failed) = typed_report(typed, text, &received, undone);
    session.log(&format!(
        "Chromium form, {name}, type_text {text:?}: {outcome}"
    ))?;
    clear(session, client, page)?;
    Ok(failed.map(|got| format!("{text:?} typed into {name} as {got}")))
}

/// Pastes `text` into the field, logs what arrived and empties it. Returns the failure
/// when it arrived otherwise.
fn pasted_case(
    session: &mut Session<'_>,
    client: &mut Client,
    page: &Page,
    name: &str,
    text: &str,
) -> Result<Option<String>> {
    let args = json!({"text": text, "via": "paste", "paste_keys": "ctrl+v"});
    let received = arrived(session, client, page, args)?;
    let failed = (!received.iter().copied().eq(text.encode_utf16())).then(|| shown(&received));
    let outcome = failed.as_ref().map_or_else(
        || "exact".to_owned(),
        |got| format!("DIFFERS, received {got}"),
    );
    session.log(&format!("Chromium form, {name}, paste {text:?}: {outcome}"))?;
    clear(session, client, page)?;
    Ok(failed.map(|got| format!("{text:?} pasted into {name} as {got}")))
}

/// The outcome for the log, and what arrived when it breaks the promise.
fn typed_report(
    typed: Typed,
    text: &str,
    received: &[u16],
    undone: usize,
) -> (String, Option<String>) {
    let sent: Vec<u16> = text.encode_utf16().collect();
    if received == sent {
        let outcome = match typed {
            Typed::Exact => "exact",
            Typed::FirstKeys => "exact, though known to keep only some keys: a fix landed",
        };
        return (outcome.to_owned(), None);
    }
    if typed == Typed::FirstKeys
        && first_keys(&sent, received)
        && undone == sent.len() - received.len()
    {
        let outcome = format!(
            "known loss (inputs the mask undid, as they came before its timer ran: {undone}), received {}",
            shown(received)
        );
        return (outcome, None);
    }
    (
        format!("DIFFERS, received {}", shown(received)),
        Some(shown(received)),
    )
}

/// Whether `received` is `sent`'s first unit and then some of the others, in order.
fn first_keys(sent: &[u16], received: &[u16]) -> bool {
    let (Some(first), Some(got)) = (sent.first(), received.first()) else {
        return false;
    };
    let mut rest = sent.iter().skip(1);
    first == got
        && received
            .iter()
            .skip(1)
            .all(|unit| rest.any(|key| key == unit))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn units(text: &str) -> Vec<u16> {
        text.encode_utf16().collect()
    }

    #[test]
    fn a_masked_field_may_drop_any_key_but_the_first() {
        for kept in ["1", "13", "1357", "1234567890"] {
            let undone = 10 - kept.len();
            let (_, failed) = typed_report(Typed::FirstKeys, "1234567890", &units(kept), undone);
            assert_eq!(failed, None, "{kept}");
        }
        // Keys lost without the mask undoing them are not its known loss.
        assert!(
            typed_report(Typed::FirstKeys, "1234567890", &units("13"), 0)
                .1
                .is_some()
        );
        // Nothing, a lost first key, a reordering or a key never sent breaks the promise.
        for other in ["", "2", "31", "1x", "11234567890"] {
            let (_, failed) = typed_report(Typed::FirstKeys, "1234567890", &units(other), 0);
            assert!(failed.is_some(), "{other:?}");
        }
        let (line, _) = typed_report(Typed::FirstKeys, "10", &units("10"), 0);
        assert!(line.contains("a fix landed"), "{line}");
    }

    #[test]
    fn any_other_field_must_get_every_key() {
        assert_eq!(typed_report(Typed::Exact, "100", &units("100"), 0).1, None);
        assert_eq!(
            typed_report(Typed::Exact, "100", &units("1"), 0)
                .1
                .as_deref(),
            Some("\"1\"")
        );
    }
}
