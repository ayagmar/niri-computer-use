//! Reading what `wev` logged of keys: exact pairs, chords and decoded text, and the C10
//! corpus the keyboard checks type.

use std::ffi::OsString;
use std::fs;
use std::path::Path;
use std::time::Duration;

use crate::failure::{Context as _, Failure, Result};
use crate::session::Session;
use crate::wev::keyboard::{self, Modifiers, Trace};

const WAIT: Duration = Duration::from_secs(5);
pub(crate) const CORPUS: &str = include_str!("../corpus.txt");
/// Twenty-four Greek letters and six arrows: more symbols a US layout lacks than its map
/// has spare keys, so native types them in parts with a map each.
pub(crate) const PARTS: &str = "αβγδεζηθικλμνξοπρστυφχψω←↑→↓↔↕";
pub(crate) fn read(path: &Path) -> Result<String> {
    fs::read_to_string(path).context(format!("read {}", path.display()))
}

pub(crate) fn offset(path: &Path) -> Result<usize> {
    Ok(read(path)?.len())
}

pub(crate) fn since(path: &Path, offset: usize) -> Result<String> {
    let log = read(path)?;
    log.get(offset..)
        .map(str::to_owned)
        .ok_or_else(|| Failure::new("wev log shrank or offset split UTF-8"))
}

pub(crate) fn args(args: &[&str]) -> Vec<OsString> {
    args.iter().map(OsString::from).collect()
}

/// Wait for the final release and, for chords, the final all-zero modifiers record.
pub(crate) fn observed(
    session: &mut Session<'_>,
    log: &Path,
    start: usize,
    count: usize,
    chord: bool,
) -> Result<String> {
    session.wait_until(
        "keyboard-events",
        "the expected keys and final modifiers",
        WAIT,
        |_| {
            let log = since(log, start)?;
            let trace = keyboard::trace(&log)?;
            let ready = trace.keys.len() >= count * 2
                && (!chord || trace.modifiers == Some(Modifiers::default()));
            Ok(ready.then_some(log))
        },
    )
}

pub(crate) fn chord(trace: &Trace<'_>, symbol: &str, mask: u32, count: usize) -> Result<()> {
    pairs(trace, count)?;
    let expected = Modifiers {
        depressed: mask,
        ..Modifiers::default()
    };
    if trace
        .keys
        .iter()
        .any(|key| key.symbol != symbol || key.modifiers != Some(expected))
        || trace.modifiers != Some(Modifiers::default())
    {
        return Err(Failure::new(format!(
            "expected {count} {symbol} pairs with mask {mask} and final modifiers 0; got {trace:?}"
        )));
    }
    Ok(())
}

fn pairs(trace: &Trace<'_>, count: usize) -> Result<()> {
    if trace.keys.len() != count * 2
        || trace
            .keys
            .as_chunks::<2>()
            .0
            .iter()
            .any(|[press, release]| {
                !press.pressed
                    || release.pressed
                    || press.code != release.code
                    || press.symbol != release.symbol
            })
    {
        return Err(Failure::new(format!(
            "expected exactly {count} ordered press/release pairs; got {trace:?}"
        )));
    }
    Ok(())
}

pub(crate) fn text(trace: &Trace<'_>, expected: &str) -> Result<()> {
    pairs(trace, expected.chars().count())?;
    let decoded: String = trace
        .keys
        .iter()
        .filter(|key| key.pressed)
        .map(|key| key.text)
        .collect();
    if decoded != expected {
        return Err(Failure::new(format!(
            "decoded text differs: expected {expected:?}, got {decoded:?}"
        )));
    }
    Ok(())
}

/// The C10 corpus has 100 characters, ASCII among them, and `é`, `ß` and `→`.
pub(crate) fn validate_corpus(corpus: &str) -> Result<()> {
    if corpus.chars().count() != 100
        || !['é', 'ß', '→'].into_iter().all(|ch| corpus.contains(ch))
        || !corpus.chars().any(|ch| ch.is_ascii_alphabetic())
    {
        return Err(Failure::new(
            "C10 corpus must have 100 characters, ASCII, é, ß and →",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wev::keyboard::Key;

    fn fixture<'a>(symbol: &'a str, text: &'a str, mask: u32) -> Trace<'a> {
        let modifiers = Modifiers {
            depressed: mask,
            ..Modifiers::default()
        };
        Trace {
            keymaps: Vec::new(),
            entered: false,
            keys: vec![
                Key {
                    time: 0,
                    code: 9,
                    pressed: true,
                    symbol,
                    text,
                    modifiers: Some(modifiers),
                },
                Key {
                    time: 0,
                    code: 9,
                    pressed: false,
                    symbol,
                    text: "",
                    modifiers: Some(modifiers),
                },
            ],
            modifiers: Some(Modifiers::default()),
            focused: true,
        }
    }

    #[test]
    fn exact_pairs_require_order_matching_keys_and_counts() {
        let mut seen = fixture("a", "a", 4);
        chord(&seen, "a", 4, 1).unwrap();
        assert!(chord(&seen, "a", 5, 1).is_err());
        assert!(chord(&seen, "F12", 4, 1).is_err());
        assert!(chord(&seen, "a", 4, 0).is_err());
        seen.keys[1].code = 10;
        assert!(pairs(&seen, 1).is_err());
        seen.keys[1].code = 9;
        seen.keys.swap(0, 1);
        assert!(pairs(&seen, 1).is_err());
        assert!(pairs(&Trace::default(), 1).is_err());
    }

    #[test]
    fn chords_require_final_released_modifiers() {
        let mut seen = fixture("F12", "", 5);
        chord(&seen, "F12", 5, 1).unwrap();
        seen.modifiers = seen.keys[0].modifiers;
        assert!(chord(&seen, "F12", 5, 1).is_err());
    }

    #[test]
    fn text_compares_decoded_unicode_not_just_event_counts() {
        let seen = fixture("eacute", "é", 0);
        text(&seen, "é").unwrap();
        assert!(text(&seen, "e").is_err());
        assert!(text(&seen, "éé").is_err());
    }

    #[test]
    fn committed_corpus_counts_characters_and_has_all_required_scripts() {
        validate_corpus(CORPUS).unwrap();
        assert_eq!(CORPUS.chars().count(), 100);
        assert!(CORPUS.len() > 100);
        assert!(validate_corpus(&format!("{CORPUS}\n")).is_err());
        assert!(validate_corpus(&CORPUS.replace('→', "x")).is_err());
    }
}
