//! Keyboard records include wev's continuation lines. Only complete records count.

use super::{complete, parse};
use crate::failure::{Failure, Result};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Modifiers {
    pub(crate) depressed: u32,
    pub(crate) latched: u32,
    pub(crate) locked: u32,
    pub(crate) group: u32,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Key<'a> {
    pub(crate) code: u32,
    pub(crate) pressed: bool,
    pub(crate) symbol: &'a str,
    pub(crate) text: &'a str,
    pub(crate) modifiers: Option<Modifiers>,
}

#[derive(Debug, Default)]
pub(crate) struct Trace<'a> {
    pub(crate) keys: Vec<Key<'a>>,
    pub(crate) modifiers: Option<Modifiers>,
    pub(crate) focused: bool,
}

pub(crate) fn has_input(log: &str) -> bool {
    complete(log)
        .lines()
        .filter_map(parse)
        .any(|event| event.interface == "wl_keyboard" && matches!(event.name, "key" | "modifiers"))
}

/// Reads ordered key and modifier records. A malformed complete record fails rather
/// than silently removing an event from an exact-count check.
pub(crate) fn trace(log: &str) -> Result<Trace<'_>> {
    let mut trace = Trace::default();
    let mut lines = complete(log).lines().peekable();
    while let Some(line) = lines.next() {
        let Some(event) = parse(line).filter(|event| event.interface == "wl_keyboard") else {
            continue;
        };
        match event.name {
            "enter" => trace.focused = true,
            "leave" => trace.focused = false,
            "keymap" => trace.modifiers = Some(Modifiers::default()),
            "key" => {
                let Some(next) = lines.peek().copied() else {
                    break;
                };
                if next.starts_with('[') {
                    return Err(Failure::new("wev key has no continuation"));
                }
                trace.keys.push(key(event.detail, next, trace.modifiers)?);
                lines.next();
            }
            "modifiers" => {
                let continuation: Vec<&str> = lines.by_ref().take(3).collect();
                if continuation.len() < 3 {
                    break;
                }
                trace.modifiers = Some(modifiers(event.detail, &continuation)?);
            }
            _ => {}
        }
    }
    Ok(trace)
}

fn number(text: &str, field: &str) -> Result<u32> {
    text.split_once(field)
        .and_then(|(_, rest)| rest.split([';', ' ', ',']).next())
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| Failure::new(format!("wev has an invalid {field}: {text}")))
}

fn key<'a>(detail: &str, continuation: &'a str, modifiers: Option<Modifiers>) -> Result<Key<'a>> {
    let state = number(detail, "state: ")?;
    let symbol = continuation
        .trim_start()
        .strip_prefix("sym: ")
        .and_then(|rest| rest.split_once(" ("))
        .map(|(symbol, _)| symbol.trim_end());
    let text = continuation
        .split_once(", utf8: '")
        .and_then(|(_, text)| text.strip_suffix('\''));
    match (symbol, text, state) {
        (Some(symbol), Some(text), 0 | 1) => Ok(Key {
            code: number(detail, "key: ")?,
            pressed: state == 1,
            symbol,
            text,
            modifiers,
        }),
        _ => Err(Failure::new(format!(
            "wev has an invalid key: {detail}; {continuation}"
        ))),
    }
}

fn modifiers(detail: &str, lines: &[&str]) -> Result<Modifiers> {
    let mut masks = lines
        .iter()
        .zip(["depressed: ", "latched: ", "locked: "])
        .map(|(line, field)| {
            line.trim_start()
                .strip_prefix(field)
                .and_then(|rest| rest.split([':', ' ']).next())
                .and_then(|value| u32::from_str_radix(value, 16).ok())
                .ok_or_else(|| Failure::new(format!("wev has invalid modifiers: {line}")))
        });
    Ok(Modifiers {
        depressed: masks
            .next()
            .ok_or_else(|| Failure::new("missing depressed"))??,
        latched: masks
            .next()
            .ok_or_else(|| Failure::new("missing latched"))??,
        locked: masks
            .next()
            .ok_or_else(|| Failure::new("missing locked"))??,
        group: number(detail, "group: ")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const MODS: &str = "[ 1: wl_keyboard] modifiers: serial: 1; group: 0\n                      depressed: 00000004: Control \n                      latched: 00000000\n                      locked: 00000000\n";
    const KEY: &str = "[ 1: wl_keyboard] key: serial: 1; time: 0; key: 9; state: 1 (pressed)\n                      sym: a            (97), utf8: 'é'\n";

    #[test]
    fn keys_keep_the_preceding_modifiers_and_unicode() {
        let log = format!("{MODS}{KEY}");
        let seen = trace(&log).unwrap();
        assert_eq!(seen.keys.len(), 1);
        assert_eq!(seen.keys[0].text, "é");
        assert_eq!(seen.keys[0].modifiers.unwrap().depressed, 4);
        assert_eq!(seen.keys[0].code, 9);
    }

    #[test]
    fn partial_records_wait_for_all_continuation_lines() {
        for end in 0..KEY.len() {
            if KEY.is_char_boundary(end) {
                assert_eq!(trace(KEY.get(..end).unwrap()).unwrap().keys, []);
            }
        }
        for end in 0..MODS.len() {
            assert_eq!(trace(MODS.get(..end).unwrap()).unwrap().modifiers, None);
        }
    }

    #[test]
    fn malformed_complete_records_fail() {
        assert!(trace(&KEY.replace("state: 1", "state: 2")).is_err());
        assert!(trace(&KEY.replace("sym:", "unknown:")).is_err());
        assert!(trace(&MODS.replace("00000004", "badmask")).is_err());
        assert!(
            trace("[ 1: wl_keyboard] key: key: 9; state: 1\n[ 1: wl_keyboard] leave\n").is_err()
        );
    }

    #[test]
    fn focus_and_keymap_state_follow_event_order() {
        let log = format!(
            "[ 1: wl_keyboard] enter: serial: 1\n{MODS}[ 1: wl_keyboard] keymap: format: 1 (xkb v1), size: 8\n{KEY}"
        );
        let seen = trace(&log).unwrap();
        assert!(seen.focused);
        assert_eq!(seen.keys[0].modifiers, Some(Modifiers::default()));
        assert!(
            !trace(&format!("{log}[ 1: wl_keyboard] leave: serial: 2\n"))
                .unwrap()
                .focused
        );
        assert!(has_input(MODS));
        assert!(has_input(KEY));
        assert!(!has_input("[ 1: wl_keyboard] keymap: format: 1\n"));
    }
}
