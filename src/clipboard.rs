//! The clipboard's text, read through wlr data-control on a Wayland connection of its own
//! to niri, so it needs no keyboard focus and works without Noctalia. What its owner marked
//! as a secret isn't read.

use serde::Serialize;
use tokio::time::Instant;

use crate::error::{ErrorName, ToolError};
use crate::niri::selection::{self, Offer, Selection};
use crate::niri::{self, Display, Socket};

/// The hint KDE's and other clipboard managers honour: don't keep this in the history.
/// Password managers such as `KeePassXC` mark what they copy with it.
pub(crate) const HINT: &str = "x-kde-passwordManagerHint";
/// The hint's value for a secret.
pub(crate) const SECRET: &[u8] = b"secret";
const MAX_TEXT: usize = 1024 * 1024;
/// The hint's value is one word; a longer one isn't `secret`, and fails the read.
const MAX_HINT: usize = 64;

/// The clipboard's text, or why there is none.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Clipboard {
    text: Option<String>,
    /// `nothing_copied`, `no_text` or `secret` when `text` is null.
    reason: Option<&'static str>,
}

impl Clipboard {
    const fn withheld(reason: &'static str) -> Self {
        Self {
            text: None,
            reason: Some(reason),
        }
    }
}

/// Reads the hint and the text from one offer, so a copy made between the two can't pass
/// a secret off as plain text: niri's PID, binding, then both reads, each within its own
/// deadline. The connection's peer must be niri's PID.
pub(crate) async fn read_text(socket: &Socket, display: &Display) -> Result<Clipboard, ToolError> {
    let display = display.path().map_err(Clone::clone)?;
    let selection = Selection::bind(display, niri::pid(socket).await?).await?;
    let Some(offer) = selection.current() else {
        return Ok(Clipboard::withheld("nothing_copied"));
    };
    let deadline = Instant::now() + selection::STEP;
    let hint = hint(&offer, deadline).await?;
    let mime = match pick(offer.types(), hint.as_deref()) {
        Ok(mime) => mime,
        Err(reason) => return Ok(Clipboard::withheld(reason)),
    };
    let text = String::from_utf8(offer.receive(mime, MAX_TEXT, deadline).await?).map_err(|_| {
        ToolError::new(
            ErrorName::UpstreamError,
            "the clipboard's text isn't valid UTF-8",
        )
    })?;
    Ok(Clipboard {
        text: Some(text),
        reason: None,
    })
}

/// The hint's value, when the offer has one. A hint that can't be read fails the read.
async fn hint(offer: &Offer<'_>, deadline: Instant) -> Result<Option<Vec<u8>>, ToolError> {
    if !offers_hint(offer.types()) {
        return Ok(None);
    }
    offer.receive(HINT, MAX_HINT, deadline).await.map(Some)
}

fn offers_hint(types: &[String]) -> bool {
    types.iter().any(|mime| mime == HINT)
}

/// The type to read the text from, or why none is read: `secret` when the owner's hint
/// says so, whatever else it offers, else the type `wl-paste --type text` reads, or
/// `no_text`. A paste's own text, which its keeper serves until another copy when it can't
/// put the clipboard back, carries the hint too, and stays withheld: it may be another
/// client's paste on a shared engine, or a password typed through `paste`, and the agent
/// that pasted it already has it.
fn pick<'a>(types: &'a [String], hint: Option<&[u8]>) -> Result<&'a str, &'static str> {
    if hint == Some(SECRET) {
        return Err("secret");
    }
    text_type(types).ok_or("no_text")
}

/// The type wl-paste 2.3.0 reads for `--type text`: `text/plain;charset=utf-8`, then
/// `text/plain`, then the first type offered that `is_text` calls text
/// (`src/wl-paste.c:66–113`, `:143–148`).
fn text_type(types: &[String]) -> Option<&str> {
    let offered = |wanted: &str| {
        types
            .iter()
            .map(String::as_str)
            .find(|mime| *mime == wanted)
    };
    offered("text/plain;charset=utf-8")
        .or_else(|| offered("text/plain"))
        .or_else(|| types.iter().map(String::as_str).find(|mime| is_text(mime)))
}

/// wl-clipboard 2.3.0's `mime_type_is_text` (`src/util/string.c:23–55`).
fn is_text(mime: &str) -> bool {
    const SUFFIXES: [&str; 6] = ["script", "xml", "yaml", "csv", "ini", "pgp-keys"];
    mime.starts_with("text/")
        || matches!(mime, "TEXT" | "STRING" | "UTF8_STRING")
        || mime.contains("json")
        || mime.contains("application/vnd.ms-publisher")
        || SUFFIXES.iter().any(|suffix| mime.ends_with(suffix))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::keeper;

    fn types(types: &[&str]) -> Vec<String> {
        types.iter().map(|mime| (*mime).to_owned()).collect()
    }

    const USERS: [&str; 3] = ["text/html", "text/plain;charset=utf-8", "image/png"];

    #[test]
    fn a_copy_its_owner_marks_secret_is_withheld() {
        let mut offered = types(&USERS);
        offered.push(HINT.to_owned());
        assert_eq!(pick(&offered, Some(SECRET)), Err("secret"));
    }

    /// A hint of another value, such as KDE's `public`, or none at all, is no secret.
    #[test]
    fn other_hints_and_no_hint_read_the_text() {
        let mut offered = types(&USERS);
        assert_eq!(pick(&offered, None), Ok("text/plain;charset=utf-8"));
        offered.push(HINT.to_owned());
        assert_eq!(
            pick(&offered, Some(b"public")),
            Ok("text/plain;charset=utf-8")
        );
        assert_eq!(
            pick(&offered, Some(b"secret\n")),
            Ok("text/plain;charset=utf-8")
        );
    }

    /// The text a paste's keeper kept serving is withheld like a password manager's.
    #[test]
    fn a_kept_paste_is_withheld() {
        let offered = types(&keeper::offered());
        let hint = offers_hint(&offered).then_some(SECRET);
        assert_eq!(pick(&offered, hint), Err("secret"));
    }

    #[test]
    fn the_text_type_is_the_one_wl_paste_reads() {
        assert_eq!(
            text_type(&types(&["text/plain", "text/plain;charset=utf-8"])),
            Some("text/plain;charset=utf-8")
        );
        assert_eq!(
            text_type(&types(&["UTF8_STRING", "text/html", "text/plain"])),
            Some("text/plain")
        );
        assert_eq!(
            text_type(&types(&["image/png", "application/json", "STRING"])),
            Some("application/json")
        );
        assert_eq!(text_type(&types(&["image/png", "application/x-ncu"])), None);
        assert_eq!(pick(&types(&["image/png"]), None), Err("no_text"));
    }

    #[test]
    fn text_types_are_wl_clipboards() {
        for mime in [
            "text/uri-list",
            "TEXT",
            "STRING",
            "UTF8_STRING",
            "application/ld+json",
            "application/javascript",
            "application/xml",
            "application/x-yaml",
            "text/csv",
            "application/x-ini",
            "application/pgp-keys",
            "application/vnd.ms-publisher",
        ] {
            assert!(is_text(mime), "{mime}");
        }
        for mime in ["image/png", "application/octet-stream", HINT, "Text"] {
            assert!(!is_text(mime), "{mime}");
        }
    }
}
