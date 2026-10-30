//! The keyboard tools' work (plan §6, §11): what `key` and `type_text` accept, the
//! `expect` check, and what a call reports of the keys that went out. The keys go out
//! through the native keyboard in `native`.

use serde::Serialize;

use crate::act::{self, Observed, Outcome};
use crate::control::desk::partway;
use crate::error::{CallError, ErrorName, ToolError};
use crate::niri::waiter::View;

use super::Input;
use super::leaked_tags;
use super::paste::Aftercare;

/// `key`'s combinations per call.
pub(crate) const MAX_KEYS: usize = 16;
/// `type_text`'s characters per call, counted in Unicode scalar values.
pub(crate) const MAX_TEXT: usize = 1000;
/// How a `focus_mismatch` refusal ends when niri reports no window focused.
pub(crate) const NO_FOCUS: &str = "no window has keyboard focus";

/// What focus must be before typing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Expect {
    Window(u64),
    App(String),
    /// Typing anywhere, for example into a shell panel that holds keyboard focus.
    Unchecked,
}

/// What the `expect` check found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Focus {
    Matched,
    Unchecked,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Typing {
    /// Combinations such as `ctrl+shift+t`, pressed in order: modifiers, then one key's
    /// keysym name.
    Keys(Vec<String>),
    /// Text, then `Return` once all of it went out when `submit` is set.
    Text { text: String, submit: bool },
}

/// The modifiers a combination may hold, by the names it accepts, each with the one name
/// `keymap` looks it up by.
const MODIFIERS: [(&str, &str); 8] = [
    ("shift", "shift"),
    ("ctrl", "ctrl"),
    ("control", "ctrl"),
    ("alt", "alt"),
    ("altgr", "altgr"),
    ("super", "logo"),
    ("logo", "logo"),
    ("win", "logo"),
];

impl Typing {
    pub(crate) const fn tool(&self) -> &'static str {
        match self {
            Self::Keys(_) => "key",
            Self::Text { .. } => "type_text",
        }
    }

    /// Refuses what can't be typed whole before anything is: an empty text or key list,
    /// text over `MAX_TEXT`, more than `MAX_KEYS` keys, or a combination that doesn't parse.
    fn check(&self) -> Result<(), CallError> {
        match self {
            Self::Keys(keys) => check_keys(keys),
            Self::Text { text, .. } => check_text(text),
        }
    }
}

fn check_keys(keys: &[String]) -> Result<(), CallError> {
    if keys.is_empty() || keys.len() > MAX_KEYS {
        return Err(CallError::InvalidArguments(format!(
            "`keys` takes 1 to {MAX_KEYS} combinations, not {}",
            keys.len()
        )));
    }
    for combo in keys {
        parse_combo(combo).map_err(CallError::InvalidArguments)?;
    }
    Ok(())
}

fn check_text(text: &str) -> Result<(), CallError> {
    let length = text.chars().count();
    if length == 0 {
        return Err(CallError::InvalidArguments(
            "`text` must not be empty".to_owned(),
        ));
    }
    // Typed, control characters are keys: a line break is Return, which sends a message
    // before the rest of it, a tab moves to another field of the same window, where
    // `expect` can't see it, and others are BackSpace, Escape, Clear or Delete.
    if let Some(at) = text.chars().position(char::is_control) {
        return Err(CallError::InvalidArguments(format!(
            "`text` holds a control character at character {at}, such as a line break or a tab, which would be pressed as a key such as Return, Tab, BackSpace or Escape; pass `submit: true` to press Return once all of the text went out, press keys with `key`, or pass `via: \"paste\"` for text with several lines; nothing was typed"
        )));
    }
    if length > MAX_TEXT {
        return Err(ToolError::new(
            ErrorName::TextTooLong,
            format!("{length} characters; at most {MAX_TEXT} per call, so split the text, ending no part with a `>`, even one followed by spaces, or pass `via: \"paste\"`; nothing was typed"),
        )
        .into());
    }
    if let Some(leak) = leaked_tags::leaked(text) {
        return Err(
            ToolError::new(ErrorName::LeakedTag, format!("{leak}; nothing was typed")).into(),
        );
    }
    Ok(())
}

/// How much of a call went out.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct Sent {
    pub(super) chars: usize,
    pub(super) keys: usize,
    /// The key after these was let go and failed, so it may have gone out.
    pub(super) underway: bool,
}

impl Sent {
    /// Counts one key: a character, or a key that types none.
    pub(super) const fn add_key(&mut self, character: bool) {
        if character {
            self.chars += 1;
        } else {
            self.keys += 1;
        }
    }

    pub(super) const fn nothing(self) -> bool {
        self.chars == 0 && self.keys == 0
    }

    /// These, and the next key let go and failed.
    pub(super) const fn underway(self) -> Self {
        Self {
            underway: true,
            ..self
        }
    }

    /// What went out, as the detail of a failure partway says it.
    fn describe(self, typing: &Typing) -> String {
        let (counted, total) = match typing {
            Typing::Keys(keys) => (format!("pressed {} of {} keys", self.keys, keys.len()), 0),
            Typing::Text { text, .. } => {
                let total = text.chars().count();
                (format!("typed {} of {total} characters", self.chars), total)
            }
        };
        let submit = matches!(typing, Typing::Text { submit: true, .. });
        // Once all the text went out, the key under way was the submitting `Return`.
        let more = self.underway && !(submit && self.chars == total);
        let counted = if more {
            format!("{counted}, and more may have gone out")
        } else {
            counted
        };
        match (submit, self.keys, self.underway) {
            (false, _, _) => counted,
            (true, 0, true) if !more => {
                format!("{counted}, and the submitting Return may have gone out")
            }
            (true, 0, _) => format!("{counted}, not yet the submitting Return"),
            (true, _, _) => format!("{counted} and the submitting Return"),
        }
    }
}

/// `ctrl+shift+t` → the modifiers' names and the key's keysym name.
pub(super) fn parse_combo(combo: &str) -> Result<(Vec<&'static str>, &str), String> {
    let mut parts: Vec<&str> = combo.split('+').collect();
    let key = parts.pop().unwrap_or_default();
    let keysym = !key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if !keysym {
        return Err(format!(
            "{combo:?} doesn't end in a keysym name such as `a`, `Return`, `F5` or `slash`"
        ));
    }
    let mut modifiers = Vec::new();
    for part in parts {
        let modifier = MODIFIERS
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(part))
            .map(|(_, modifier)| *modifier)
            .ok_or_else(|| {
                format!("unknown modifier {part:?}; use shift, ctrl, alt, altgr or super")
            })?;
        if modifiers.contains(&modifier) {
            return Err(format!("modifier {part:?} appears twice in {combo:?}"));
        }
        modifiers.push(modifier);
    }
    Ok((modifiers, key))
}

/// Checks `expect` against the window with keyboard focus.
pub(crate) fn check_expect(expect: &Expect, view: &View) -> Result<Focus, ToolError> {
    let focused = view.focused_window().and_then(|id| view.windows().get(&id));
    let matched = match expect {
        Expect::Unchecked => return Ok(Focus::Unchecked),
        Expect::Window(id) => focused.is_some_and(|window| window.id == *id),
        Expect::App(app_id) => {
            focused.is_some_and(|window| window.app_id.as_deref() == Some(app_id.as_str()))
        }
    };
    if matched {
        return Ok(Focus::Matched);
    }
    let actual = focused.map_or_else(
        || NO_FOCUS.to_owned(),
        |window| format!("window {} with app_id {:?}", window.id, window.app_id),
    );
    let expected = match expect {
        Expect::Window(id) => format!("window_id {id}"),
        Expect::App(app_id) => format!("app_id {app_id:?}"),
        Expect::Unchecked => "nothing".to_owned(),
    };
    Err(ToolError::new(
        ErrorName::FocusMismatch,
        format!("expected {expected}; {actual}"),
    ))
}

/// Types `typing` through the native keyboard. `observed` is `sent` once every key went
/// out, or `interrupted` when focus moved off the window that had it; then the keys after
/// that one aren't typed, and the outcome says how much was.
pub(crate) async fn type_input(
    input: Input<'_>,
    typing: Typing,
    expect: Expect,
) -> Result<Outcome, CallError> {
    act::aim_at(input.niri, act::About::Focused).await;
    type_input_then(input, typing, expect, &mut None).await
}

/// `type_input`, where the first key takes `aftercare` right before it can go out, and
/// keeps the input-dirty marker until it has said the key went out. `aftercare` is still
/// there when nothing went out.
pub(crate) async fn type_input_then(
    input: Input<'_>,
    typing: Typing,
    expect: Expect,
    aftercare: &mut Option<Aftercare>,
) -> Result<Outcome, CallError> {
    typing.check()?;
    super::native::type_input(input, typing, expect, aftercare).await
}

/// What a stop or a moved lease reports once `sent` went out: `uncertain`, with what was
/// typed.
pub(super) fn typed_so_far(view: &View, focus: Focus, typing: &Typing, sent: Sent) -> Outcome {
    let answered = Outcome::uncertain(Some(true), Some(view), String::new());
    ended(answered, focus, typing, sent)
}

/// The outcome with what went out: for a call that stopped early, how many characters or
/// keys; for `submit`, whether `Return` was pressed. An `interrupted` outcome's detail says
/// what changed while keys went out; it becomes what went out and where some of it may have
/// gone, and is dropped when nothing did.
pub(super) fn ended(outcome: Outcome, focus: Focus, typing: &Typing, sent: Sent) -> Outcome {
    let detail = match (outcome.observed, outcome.detail) {
        (Observed::Interrupted, Some(changed)) if !sent.nothing() => {
            Some(format!("{}; {changed}", sent.describe(typing)))
        }
        (Observed::Interrupted, _) => None,
        (_, detail) => detail,
    };
    let (typed, pressed, submitted) = match typing {
        Typing::Keys(keys) => (None, (sent.keys < keys.len()).then_some(sent.keys), None),
        Typing::Text { text, submit } => {
            let total = text.chars().count();
            let submitted = submit.then_some(sent.keys == 1);
            ((sent.chars < total).then_some(sent.chars), None, submitted)
        }
    };
    Outcome {
        focus: Some(focus),
        typed,
        pressed,
        submitted,
        detail,
        ..outcome
    }
}

/// A key's failure, saying how much went out before it, or may have, and without
/// `lock_stale`'s "try once more" once something did (`partway`).
pub(super) fn partly(error: ToolError, typing: &Typing, sent: Sent) -> ToolError {
    if sent.nothing() && !sent.underway {
        return error;
    }
    let error = partway(error);
    ToolError {
        detail: format!("{}; then {}", sent.describe(typing), error.detail),
        may_have_sent: true,
        ..error
    }
}
#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::niri::waiter::tests::{view, window};

    fn text(text: &str, submit: bool) -> Typing {
        Typing::Text {
            text: text.to_owned(),
            submit,
        }
    }

    fn keys(keys: &[&str]) -> Typing {
        Typing::Keys(keys.iter().map(|&key| key.to_owned()).collect())
    }

    #[test]
    fn a_combo_is_its_modifiers_then_one_keysym_name() {
        let parsed = |combo| parse_combo(combo).unwrap();
        assert_eq!(parsed("ctrl+shift+t"), (vec!["ctrl", "shift"], "t"));
        assert_eq!(parsed("Super+Return"), (vec!["logo"], "Return"));
        assert_eq!(parsed("F12"), (vec![], "F12"));
        for bad in [
            "",
            "ctrl+",
            "ctrl+ctrl+a",
            "hyper+a",
            "ctrl+-",
            "a b",
            "ctrl+control+a",
        ] {
            assert!(keys(&[bad]).check().is_err(), "{bad}");
        }
    }

    #[test]
    fn one_bad_combination_refuses_the_whole_list_before_anything_is_pressed() {
        assert!(keys(&["ctrl+l", "Return"]).check().is_ok());
        assert!(matches!(
            keys(&["ctrl+l", "hyper+a"]).check(),
            Err(CallError::InvalidArguments(message)) if message.contains("unknown modifier")
        ));
        assert!(keys(&[]).check().is_err());
        assert!(keys(&["a"; MAX_KEYS]).check().is_ok());
        assert!(keys(&["a"; MAX_KEYS + 1]).check().is_err());
    }

    #[test]
    fn text_is_capped_in_scalar_values() {
        assert!(text(&"é".repeat(1000), false).check().is_ok());
        let long = text(&"→".repeat(1001), false).check().unwrap_err();
        let CallError::Tool(error) = long else {
            panic!("{long:?}")
        };
        assert_eq!(error.name, ErrorName::TextTooLong);
        assert!(
            error.detail.starts_with("1001 characters"),
            "{}",
            error.detail
        );
        assert!(
            error
                .detail
                .contains("ending no part with a `>`, even one followed by spaces,"),
            "{}",
            error.detail
        );
        assert!(matches!(
            text("", true).check(),
            Err(CallError::InvalidArguments(_))
        ));
    }

    #[test]
    fn the_outcome_says_how_much_went_out_and_whether_it_was_submitted() {
        let current = view(vec![window(1, Some("a"), 1, true)]);
        let report = |typing: &Typing, chars, keys| {
            let outcome = Outcome::seen(Observed::Interrupted, &current, Vec::new());
            let ended = ended(
                outcome,
                Focus::Matched,
                typing,
                Sent {
                    chars,
                    keys,
                    underway: false,
                },
            );
            (ended.typed, ended.pressed, ended.submitted)
        };
        let message = text(&"x".repeat(250), true);
        assert_eq!(report(&message, 100, 0), (Some(100), None, Some(false)));
        // Every character went out, but focus moved before `Return`.
        assert_eq!(report(&message, 250, 0), (None, None, Some(false)));
        assert_eq!(report(&message, 250, 1), (None, None, Some(true)));
        assert_eq!(report(&text("x", false), 1, 0), (None, None, None));
        let list = keys(&["Down", "Down", "Return"]);
        assert_eq!(report(&list, 0, 2), (None, Some(2), None));
        assert_eq!(report(&list, 0, 3), (None, None, None));
        // A key let go may have gone out, but `Return` comes after it.
        let failed = partly(
            ToolError::new(ErrorName::UpstreamError, "niri closed the connection"),
            &message,
            Sent {
                chars: 200,
                keys: 0,
                underway: true,
            },
        );
        assert_eq!(
            failed.detail,
            "typed 200 of 250 characters, and more may have gone out, not yet the submitting Return; then niri closed the connection"
        );
        // All the text went out and only `Return` was refused, which the detail must say.
        let refused = partly(
            ToolError::new(ErrorName::ScreenLocked, "that call wasn't sent"),
            &message,
            Sent {
                chars: 250,
                keys: 0,
                underway: false,
            },
        );
        assert_eq!(
            refused.detail,
            "typed 250 of 250 characters, not yet the submitting Return; then that call wasn't sent"
        );
    }

    #[test]
    fn expect_checks_the_focused_window() {
        let focused = view(vec![
            window(1, Some("foot"), 1, true),
            window(2, Some("x"), 1, false),
        ]);
        let nothing = view(vec![window(1, Some("foot"), 1, false)]);
        let check =
            |expect: Expect, view: &View| check_expect(&expect, view).map_err(|error| error.name);
        assert_eq!(check(Expect::Window(1), &focused), Ok(Focus::Matched));
        assert_eq!(
            check(Expect::App("foot".to_owned()), &focused),
            Ok(Focus::Matched)
        );
        assert_eq!(check(Expect::Unchecked, &nothing), Ok(Focus::Unchecked));
        let mismatch = Err(ErrorName::FocusMismatch);
        assert_eq!(check(Expect::Window(2), &focused), mismatch);
        assert_eq!(check(Expect::App("x".to_owned()), &focused), mismatch);
        // A lock screen or a shell panel leaves no window focused.
        assert_eq!(check(Expect::Window(1), &nothing), mismatch);
        let detail = check_expect(&Expect::Window(2), &focused)
            .unwrap_err()
            .detail;
        assert!(detail.contains("window 1"), "{detail}");
    }

    /// `name` with each ASCII letter's case picked by the matching bit of `cases`.
    fn recased(name: &str, cases: u64) -> String {
        name.chars()
            .enumerate()
            .map(|(at, c)| {
                if cases >> (at % 64) & 1 == 1 {
                    c.to_ascii_uppercase()
                } else {
                    c
                }
            })
            .collect()
    }

    proptest! {
        #![proptest_config(crate::test_support::properties(512))]

        /// Distinct modifiers, under any name and in any case, before a keysym name parse
        /// to the modifiers in the order given and that very key.
        #[test]
        fn a_combination_parses_to_its_modifiers_and_key(
            picked in prop::sample::subsequence(MODIFIERS.to_vec(), 0..=MODIFIERS.len()).prop_shuffle(),
            cases in any::<u64>(),
            key in "[A-Za-z0-9_]{1,12}",
        ) {
            let mut wanted: Vec<&str> = Vec::new();
            let mut names = Vec::new();
            for (name, modifier) in picked {
                if !wanted.contains(&modifier) {
                    wanted.push(modifier);
                    names.push(recased(name, cases));
                }
            }
            names.push(key.clone());
            let combo = names.join("+");
            prop_assert_eq!(parse_combo(&combo), Ok((wanted, key.as_str())));
        }

        /// Any text is accepted or refused as `key`'s combination without a panic, through
        /// the parser and the keymap's lookup the audit log uses, and a combination that
        /// parses names the key it ends with.
        #[test]
        fn any_combination_is_parsed_or_refused(combo in any::<String>()) {
            let known = crate::input::keymap::known_combo(&combo);
            match parse_combo(&combo) {
                Ok((_, key)) => {
                    prop_assert!(!key.is_empty() && combo.ends_with(key), "{combo:?}");
                }
                Err(_) => prop_assert!(!known, "{combo:?}"),
            }
        }
    }
}
