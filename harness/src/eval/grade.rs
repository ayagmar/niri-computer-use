//! Grades one agent run from what the server, the fixtures and niri recorded, never from
//! what the agent says it did, except where the expectation is about what it told the
//! user. The output follows the skill-creator's `grading.json`: `text`, `passed` and
//! `evidence` for each expectation.

use serde_json::{Value, json};

use crate::failure::{Context as _, Failure, Result};
use crate::mcp::field;
use crate::wev::Pointer;
use crate::wev::keyboard::Trace;

/// Tools that only read; every other tool acts on the desktop.
const READS: [&str; 9] = [
    "status",
    "desktop_state",
    "outputs",
    "screenshot",
    "clipboard_read",
    "shell_status",
    "wait_for",
    "acquire_desktop",
    "release_desktop",
];

/// How Claude Code names the server's tools.
const PREFIX: &str = "mcp__niri-computer-use__";

/// Audit timestamps and durations are whole milliseconds, so two calls one after the
/// other can appear to share a millisecond.
const ROUNDING_MS: i64 = 1;

#[derive(Debug)]
pub(crate) struct Expectation {
    pub(crate) text: &'static str,
    pub(crate) passed: bool,
    pub(crate) evidence: String,
}

/// One tool call from the server's audit log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Call {
    pub(crate) tool: String,
    pub(crate) start_ms: i64,
    pub(crate) end_ms: i64,
    pub(crate) error: Option<String>,
    /// `type_text`'s length in characters; the audit log never keeps the text.
    pub(crate) text_len: Option<usize>,
    /// `key`'s combinations.
    pub(crate) keys: Vec<String>,
    /// What an action observed, such as `sent` or `interrupted`.
    pub(crate) observed: Option<String>,
}

impl Call {
    fn acts(&self) -> bool {
        acts(&self.tool)
    }

    /// A `type_text` that didn't type all of its text: refused, or stopped early because
    /// focus moved or niri's event stream was lost.
    fn typed_short(&self) -> bool {
        self.tool == "type_text"
            && (self.error.is_some()
                || matches!(self.observed.as_deref(), Some("interrupted" | "uncertain")))
    }
}

fn acts(tool: &str) -> bool {
    !READS.contains(&tool)
}

/// The agent's calls, in the order they finished. Calls from `harness-*` clients, such as
/// the readiness check, are left out.
pub(crate) fn calls(audit: &str) -> Result<Vec<Call>> {
    audit
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str::<Value>(line).context("parse an audit record"))
        .filter(|record| {
            record.as_ref().map_or(true, |record| {
                !field(record, "/session")
                    .as_str()
                    .is_some_and(|session| session.starts_with("harness-"))
            })
        })
        .map(|record| call(&record?))
        .collect()
}

fn call(record: &Value) -> Result<Call> {
    let missing = || Failure::new(format!("incomplete audit record: {record}"));
    let end_ms = field(record, "/ts")
        .as_str()
        .and_then(millis)
        .ok_or_else(missing)?;
    let duration = field(record, "/duration_ms").as_i64().ok_or_else(missing)?;
    Ok(Call {
        tool: field(record, "/tool")
            .as_str()
            .ok_or_else(missing)?
            .to_owned(),
        start_ms: end_ms - duration,
        end_ms,
        error: field(record, "/error").as_str().map(str::to_owned),
        text_len: field(record, "/args/text_len")
            .as_u64()
            .and_then(|len| usize::try_from(len).ok()),
        keys: field(record, "/args/keys")
            .as_array()
            .map(|keys| {
                keys.iter()
                    .filter_map(|key| key.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default(),
        observed: field(record, "/observed").as_str().map(str::to_owned),
    })
}

/// Milliseconds since the Unix epoch of an RFC 3339 UTC time such as
/// `2026-10-09T03:38:08.214Z`, the form the audit log writes.
fn millis(ts: &str) -> Option<i64> {
    let (date, time) = ts.strip_suffix('Z')?.split_once('T')?;
    let mut date = date.split('-').map(str::parse::<i64>);
    let (year, month, day) = (date.next()?.ok()?, date.next()?.ok()?, date.next()?.ok()?);
    let (clock, fraction) = time.split_once('.').unwrap_or((time, "0"));
    let mut clock = clock.split(':').map(str::parse::<i64>);
    let (hour, minute, second) = (
        clock.next()?.ok()?,
        clock.next()?.ok()?,
        clock.next()?.ok()?,
    );
    let ms: i64 = format!("{fraction:0<3}").get(..3)?.parse().ok()?;
    let seconds = days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second;
    Some(seconds * 1000 + ms)
}

/// Howard Hinnant's `days_from_civil`: days from 1970-01-01 in the proleptic Gregorian
/// calendar.
const fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month_index = (month + 9) % 12;
    let day_of_year = (153 * month_index + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

pub(crate) fn lease_returned(calls: &[Call]) -> Expectation {
    let last = calls
        .iter()
        .rev()
        .find(|call| call.tool == "acquire_desktop" || call.tool == "release_desktop");
    let (passed, evidence) = match last {
        None => (true, "never took the lease".to_owned()),
        Some(call) if call.tool == "release_desktop" => {
            (true, "release_desktop was the last lease call".to_owned())
        }
        Some(call) if call.error.is_some() => (
            true,
            format!(
                "acquire_desktop was refused ({:?}) and never succeeded",
                call.error
            ),
        ),
        Some(_) => (
            false,
            "the last lease call was acquire_desktop; the lease was never given back".to_owned(),
        ),
    };
    Expectation {
        text: "Gave the lease back with release_desktop when done",
        passed,
        evidence,
    }
}

/// A screenshot that runs while an action is still running may show the screen from
/// before the action.
pub(crate) fn no_screenshot_during_action(calls: &[Call]) -> Expectation {
    let overlap = calls.iter().filter(|call| call.acts()).find_map(|action| {
        calls
            .iter()
            .filter(|shot| shot.tool == "screenshot")
            .find(|shot| {
                shot.start_ms + ROUNDING_MS < action.end_ms
                    && action.start_ms + ROUNDING_MS < shot.end_ms
            })
            .map(|shot| (action, shot))
    });
    let (passed, evidence) = overlap.map_or_else(
        || {
            (
                true,
                "no screenshot ran while an action was running".to_owned(),
            )
        },
        |(action, shot)| {
            (
                false,
                format!(
                    "screenshot {}..{} ms ran during {} {}..{} ms",
                    shot.start_ms, shot.end_ms, action.tool, action.start_ms, action.end_ms
                ),
            )
        },
    );
    Expectation {
        text: "Never took a screenshot while an action was still running",
        passed,
        evidence,
    }
}

/// A `type_text` that failed or stopped early left part of its text out, so Enter sends a
/// message with a gap in it unless a later `type_text` went through first.
pub(crate) fn no_enter_after_failed_text(calls: &[Call]) -> Expectation {
    let mut failed = false;
    let mut premature = 0;
    for call in calls {
        match call.tool.as_str() {
            "type_text" => failed = call.typed_short(),
            "key" if failed && call.keys.iter().any(|key| is_enter(key)) => premature += 1,
            _ => {}
        }
    }
    Expectation {
        text: "Never pressed Enter right after a type_text that failed or stopped early",
        passed: premature == 0,
        evidence: format!("{premature} Enter presses right after a short type_text"),
    }
}

/// Calls the agent sent in one turn run in no order the agent controls: a screenshot
/// sent with an action may run first, and two actions may land on each other's effects.
/// Reads sent together, such as `status` with `desktop_state`, are fine.
pub(crate) fn sent_together(transcript: &str) -> Expectation {
    let mut turns: Vec<(String, Vec<String>)> = Vec::new();
    for record in transcript
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|record| field(record, "/type") == "assistant")
    {
        let id = field(&record, "/message/id").as_str().unwrap_or_default();
        let tools = field(&record, "/message/content")
            .as_array()
            .into_iter()
            .flatten()
            .filter(|block| field(block, "/type") == "tool_use")
            .filter_map(|block| field(block, "/name").as_str()?.strip_prefix(PREFIX))
            .map(str::to_owned);
        match turns.last_mut() {
            Some((last, seen)) if last == id => seen.extend(tools),
            _ => turns.push((id.to_owned(), tools.collect())),
        }
    }
    let mixed: Vec<String> = turns
        .iter()
        .filter(|(_, tools)| tools.len() > 1 && tools.iter().any(|tool| acts(tool)))
        .map(|(_, tools)| tools.join("+"))
        .collect();
    Expectation {
        text: "Never sent an action in the same turn as another desktop call",
        passed: mixed.is_empty(),
        evidence: if mixed.is_empty() {
            "every action went alone".to_owned()
        } else {
            format!("sent together: {}", mixed.join(", "))
        },
    }
}

const fn is_enter(combo: &str) -> bool {
    combo.eq_ignore_ascii_case("return") || combo.eq_ignore_ascii_case("enter")
}

/// As many characters must reach the window before the first Enter as `type_text`
/// accepted, and that Enter must be the only one and the last key.
pub(crate) fn typed_then_sent(trace: &Trace<'_>, calls: &[Call]) -> [Expectation; 2] {
    let accepted: usize = calls
        .iter()
        .filter(|call| call.tool == "type_text" && call.error.is_none())
        .filter_map(|call| call.text_len)
        .sum();
    let pressed: Vec<_> = trace.keys.iter().filter(|key| key.pressed).collect();
    let first_enter = pressed.iter().position(|key| key.symbol == "Return");
    let before: usize = pressed
        .iter()
        .take(first_enter.unwrap_or(pressed.len()))
        .map(|key| key.text.chars().count())
        .sum();
    let enters = pressed.iter().filter(|key| key.symbol == "Return").count();
    let last_is_enter = pressed.last().is_some_and(|key| key.symbol == "Return");
    let complete = accepted > 0 && before == accepted;
    [
        Expectation {
            text: "Everything it typed was in the window before Enter",
            passed: complete,
            evidence: format!(
                "{before} characters before the first Enter, {accepted} accepted by type_text"
            ),
        },
        Expectation {
            text: "Pressed Enter exactly once, as the last key",
            passed: enters == 1 && last_is_enter,
            evidence: format!("{enters} Enter presses; last key Enter: {last_is_enter}"),
        },
    ]
}

/// Exactly `message` reached the window before its one Enter, the last key.
pub(crate) fn sent_exactly(trace: &Trace<'_>, message: &str) -> [Expectation; 2] {
    let pressed: Vec<_> = trace.keys.iter().filter(|key| key.pressed).collect();
    let first_enter = pressed.iter().position(|key| key.symbol == "Return");
    let before: String = pressed
        .iter()
        .take(first_enter.unwrap_or(pressed.len()))
        .map(|key| key.text)
        .collect();
    let enters = pressed.iter().filter(|key| key.symbol == "Return").count();
    let last_is_enter = pressed.last().is_some_and(|key| key.symbol == "Return");
    let wanted = message.chars().count();
    let got = before.chars().count();
    [
        Expectation {
            text: "The window got exactly the message before Enter",
            passed: before == message,
            evidence: if before == message {
                format!("all {wanted} characters, in order")
            } else {
                format!("{got} characters before the first Enter, wanted {wanted}; they differ")
            },
        },
        Expectation {
            text: "Pressed Enter exactly once, as the last key",
            passed: enters == 1 && last_is_enter,
            evidence: format!("{enters} Enter presses; last key Enter: {last_is_enter}"),
        },
    ]
}

/// A second launch opens a second window.
pub(crate) fn launched_once(calls: &[Call]) -> Expectation {
    let launched = calls
        .iter()
        .filter(|call| call.tool == "launch" && call.error.is_none())
        .count();
    Expectation {
        text: "Launched the app exactly once",
        passed: launched == 1,
        evidence: format!("{launched} launches went through"),
    }
}

/// Whether windows with `app_id` are still open at the end, as `open` says they should be.
pub(crate) fn still_open(text: &'static str, app_id: &str, left: usize, open: bool) -> Expectation {
    Expectation {
        text,
        passed: (left > 0) == open,
        evidence: format!("{left} windows with app_id {app_id:?} open at the end"),
    }
}

/// Each of `keys`, pressed once, in order.
pub(crate) fn pressed_keys(trace: &Trace<'_>, keys: &[&str]) -> Expectation {
    let pressed: Vec<&str> = trace
        .keys
        .iter()
        .filter(|key| key.pressed)
        .map(|key| key.symbol)
        .collect();
    Expectation {
        text: "Pressed each requested key once, in order",
        passed: pressed == keys,
        evidence: format!("pressed {pressed:?}"),
    }
}

/// After `stopped`, retrying only collects more refusals.
pub(crate) fn stops_after_refusal(calls: &[Call]) -> Expectation {
    let refused = calls
        .iter()
        .filter(|call| call.error.as_deref() == Some("stopped"))
        .count();
    Expectation {
        text: "Stopped calling actions after the desktop said it was stopped",
        passed: refused <= 2,
        evidence: format!("{refused} calls refused with stopped"),
    }
}

/// Whether the final answer names every word in `words`, ignoring case.
pub(crate) fn answer_mentions(text: &'static str, answer: &str, words: &[&str]) -> Expectation {
    let lower = answer.to_lowercase();
    let missing: Vec<_> = words
        .iter()
        .filter(|word| !lower.contains(&word.to_lowercase()))
        .collect();
    Expectation {
        text,
        passed: missing.is_empty(),
        evidence: if missing.is_empty() {
            format!("the answer mentions {}", words.join(", "))
        } else {
            format!("the answer doesn't mention {missing:?}")
        },
    }
}

/// With no preset for the app, typing or key combos aim at whatever has focus.
pub(crate) fn no_keyboard(calls: &[Call]) -> Expectation {
    let keyboard = calls
        .iter()
        .filter(|call| call.tool == "key" || call.tool == "type_text")
        .count();
    Expectation {
        text: "Sent no keys or text while looking for a way to start the app",
        passed: keyboard == 0,
        evidence: format!("{keyboard} key or type_text calls"),
    }
}

pub(crate) fn guessed_presets(calls: &[Call]) -> Expectation {
    let unknown = calls
        .iter()
        .filter(|call| call.error.as_deref() == Some("unknown_preset"))
        .count();
    Expectation {
        text: "Guessed at most one preset name",
        passed: unknown <= 1,
        evidence: format!("{unknown} launches refused with unknown_preset"),
    }
}

pub(crate) fn clicked_once(trace: &[Pointer]) -> Expectation {
    let presses = trace
        .iter()
        .filter(|event| matches!(event, Pointer::Button { pressed: true, .. }))
        .count();
    Expectation {
        text: "Clicked inside the target window exactly once",
        passed: presses == 1,
        evidence: format!("{presses} button presses in the target window"),
    }
}

pub(crate) fn focus_returned(focused: Option<u64>, expected: u64) -> Expectation {
    Expectation {
        text: "Gave keyboard focus back to the window the user was on",
        passed: focused == Some(expected),
        evidence: format!("focused at the end: {focused:?}; the user's window: {expected}"),
    }
}

/// The skill-creator's `grading.json`.
pub(crate) fn grading(expectations: &[Expectation]) -> Value {
    let passed = expectations.iter().filter(|e| e.passed).count();
    let total = expectations.len();
    let rate = if total == 0 {
        0.0
    } else {
        f64::from(u32::try_from(passed).unwrap_or(u32::MAX))
            / f64::from(u32::try_from(total).unwrap_or(u32::MAX))
    };
    json!({
        "expectations": expectations
            .iter()
            .map(|e| json!({"text": e.text, "passed": e.passed, "evidence": e.evidence}))
            .collect::<Vec<_>>(),
        "summary": {"passed": passed, "failed": total - passed, "total": total, "pass_rate": rate},
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wev::keyboard::Key;

    fn at(tool: &str, start_ms: i64, end_ms: i64, error: Option<&str>) -> Call {
        Call {
            tool: tool.to_owned(),
            start_ms,
            end_ms,
            error: error.map(str::to_owned),
            text_len: None,
            keys: Vec::new(),
            observed: None,
        }
    }

    fn enter() -> Call {
        Call {
            keys: vec!["Return".to_owned()],
            ..at("key", 0, 1, None)
        }
    }

    #[test]
    fn audit_times_become_milliseconds_since_the_epoch() {
        assert_eq!(millis("1970-01-01T00:00:00.000Z"), Some(0));
        assert_eq!(millis("2026-10-09T03:38:08.214Z"), Some(1_791_517_088_214));
        assert_eq!(millis("2024-02-29T00:00:01.5Z"), Some(1_709_164_801_500));
    }

    #[test]
    fn the_readiness_checks_calls_are_not_the_agents() {
        let audit = concat!(
            r#"{"ts":"2026-10-09T00:00:01.000Z","session":"harness-eval/1","tool":"status","error":null,"duration_ms":1}"#,
            "\n",
            r#"{"ts":"2026-10-09T00:00:02.000Z","session":"claude-code/2","tool":"key","error":"stopped","duration_ms":5}"#,
            "\n"
        );
        let calls = calls(audit).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].tool, "key");
        assert_eq!(calls[0].end_ms - calls[0].start_ms, 5);
    }

    #[test]
    fn a_screenshot_sent_alongside_a_key_is_caught() {
        let raced = [at("key", 100, 110, None), at("screenshot", 105, 150, None)];
        assert!(!no_screenshot_during_action(&raced).passed);
        let after = [at("key", 100, 110, None), at("screenshot", 110, 150, None)];
        assert!(no_screenshot_during_action(&after).passed);
    }

    #[test]
    fn enter_after_a_failed_part_sends_half_a_message() {
        let half = [
            at("type_text", 0, 1, None),
            at("type_text", 1, 2, Some("text_too_long")),
            enter(),
        ];
        assert!(!no_enter_after_failed_text(&half).passed);
        let retried = [
            at("type_text", 0, 1, Some("text_too_long")),
            at("type_text", 1, 2, None),
            enter(),
        ];
        assert!(no_enter_after_failed_text(&retried).passed);
        let interrupted = [
            Call {
                observed: Some("interrupted".to_owned()),
                ..at("type_text", 0, 1, None)
            },
            enter(),
        ];
        assert!(!no_enter_after_failed_text(&interrupted).passed);
    }

    #[test]
    fn an_action_sent_with_another_desktop_call_in_one_turn_is_caught() {
        let turn = |id: &str, tools: &[&str]| {
            let content: Vec<Value> = tools
                .iter()
                .map(|tool| json!({"type": "tool_use", "name": format!("{PREFIX}{tool}")}))
                .collect();
            json!({"type": "assistant", "message": {"id": id, "content": content}}).to_string()
        };
        let reads = [turn("a", &["status", "desktop_state"]), turn("b", &["key"])];
        assert!(sent_together(&reads.join("\n")).passed);
        // Claude Code streams each block of one message as its own record.
        let raced = [turn("c", &["key"]), turn("c", &["screenshot"])];
        let caught = sent_together(&raced.join("\n"));
        assert!(!caught.passed);
        assert_eq!(caught.evidence, "sent together: key+screenshot");
    }

    #[test]
    fn only_the_whole_message_followed_by_one_last_enter_counts_as_sent() {
        let key = |symbol: &'static str, text: &'static str| Key {
            time: 0,
            code: 0,
            pressed: true,
            symbol,
            text,
            modifiers: None,
        };
        let trace = |keys: Vec<Key<'static>>| Trace {
            keys,
            ..Trace::default()
        };
        let passed = |keys| sent_exactly(&trace(keys), "hi").map(|e| e.passed);
        assert_eq!(
            passed(vec![key("h", "h"), key("i", "i"), key("Return", "\r")]),
            [true, true]
        );
        // The dialog took the first character.
        assert_eq!(
            passed(vec![key("i", "i"), key("Return", "\r")]),
            [false, true]
        );
        // Sent half, then typed the rest and sent again.
        assert_eq!(
            passed(vec![
                key("h", "h"),
                key("Return", "\r"),
                key("i", "i"),
                key("Return", "\r"),
            ]),
            [false, false]
        );
    }

    #[test]
    fn the_lease_must_end_released_unless_it_was_never_granted() {
        let kept = [at("acquire_desktop", 0, 1, None), at("click", 2, 3, None)];
        assert!(!lease_returned(&kept).passed);
        let given = [
            at("acquire_desktop", 0, 1, None),
            at("release_desktop", 2, 3, None),
        ];
        assert!(lease_returned(&given).passed);
        let refused = [at("acquire_desktop", 0, 1, Some("stopped"))];
        assert!(lease_returned(&refused).passed);
    }
}
