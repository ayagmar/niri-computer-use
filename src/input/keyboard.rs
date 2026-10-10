//! The keyboard tools' work (plan §6, §11): one `wtype` call per stroke, each behind the
//! stdin gate. A stroke is one of `key`'s combinations, or a part of at most 100 characters
//! of `type_text`'s text, or the `Return` that submits it. Focus is checked after every
//! stroke, and the rest isn't typed once it moved. `wtype -` waits at its open stdin before
//! it sends anything, so the input-dirty marker names its PID before any key goes out.
//!
//! The call runs in a task of its own: a stop or a cancelled request drops the tool's
//! work, but a `wtype` already typing finishes within its deadline and then removes the
//! marker. Past the deadline its group is killed and the marker stays (plan §11).

use std::path::Path;
use std::time::Duration;

use serde::Serialize;

use crate::act::{Observed, Outcome};
use crate::control::cleanup;
use crate::control::marker::{Child, Marker, Phase, Written};
use crate::control::procs;
use crate::control::runtime::RuntimeDir;
use crate::error::{CallError, ErrorName, ToolError};
use crate::niri::waiter::View;
use crate::niri::{self, waiter::Waited};
use crate::policy;
use crate::runner::{self, Finished, Gated};

use super::Input;
use super::paste::Aftercare;

/// Plan §6: about 0.4 s of wtype's sleeps at the 100-character cap, and C10's slowest
/// run of the corpus at well under half of this.
const WTYPE_DEADLINE: Duration = Duration::from_secs(3);
/// One `wtype` call's text, counted in Unicode scalar values, as wtype batches them.
const MAX_PART: usize = 100;
/// `key`'s combinations per call.
pub(crate) const MAX_KEYS: usize = 16;
/// Ten parts, each with its own deadline: under fifteen seconds at C10's slowest rate.
const MAX_TEXT: usize = 1000;
/// wtype prints nothing on success.
const MAX_STDOUT: u64 = 64 * 1024;
/// wtype decodes stdin with the locale's `mbstowcs`; outside a UTF-8 locale it stops at
/// the first character beyond ASCII and still exits 0. glibc always has `C.UTF-8`.
const UTF8: [(&str, &str); 1] = [("LC_ALL", "C.UTF-8")];

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

/// What one `wtype` call types.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Stroke {
    Key(String),
    Text(String),
}

/// wtype's names for the modifiers a combination may hold, by the names it accepts.
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
    /// text over `MAX_TEXT`, more than `MAX_KEYS` keys, or a combination wtype can't press.
    fn check(&self) -> Result<(), CallError> {
        match self {
            Self::Keys(keys) => check_keys(keys),
            Self::Text { text, .. } => check_text(text),
        }
    }

    /// The `wtype` calls, in order: each key, or the text in parts of at most `MAX_PART`
    /// characters and then `Return` to submit it.
    fn strokes(&self) -> Vec<Stroke> {
        match self {
            Self::Keys(keys) => keys.iter().cloned().map(Stroke::Key).collect(),
            Self::Text { text, submit } => {
                let chars: Vec<char> = text.chars().collect();
                let mut strokes: Vec<Stroke> = chars
                    .chunks(MAX_PART)
                    .map(|part| Stroke::Text(part.iter().collect()))
                    .collect();
                if *submit {
                    strokes.push(Stroke::Key("Return".to_owned()));
                }
                strokes
            }
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
    if length > MAX_TEXT {
        return Err(ToolError::new(
            ErrorName::TextTooLong,
            format!("{length} characters; at most {MAX_TEXT} per call, so split the text; nothing was typed"),
        )
        .into());
    }
    Ok(())
}

impl Stroke {
    /// wtype's arguments: `-` first, so it waits for stdin before anything else, then
    /// for a key the modifiers pressed, the key, and the modifiers released in reverse.
    fn args(&self) -> Result<Vec<String>, CallError> {
        let mut args = vec!["-".to_owned()];
        let Self::Key(combo) = self else {
            return Ok(args);
        };
        let (modifiers, key) = parse_combo(combo).map_err(CallError::InvalidArguments)?;
        for modifier in &modifiers {
            args.extend(["-M".to_owned(), (*modifier).to_owned()]);
        }
        args.extend(["-k".to_owned(), key.to_owned()]);
        for modifier in modifiers.iter().rev() {
            args.extend(["-m".to_owned(), (*modifier).to_owned()]);
        }
        Ok(args)
    }

    /// What wtype reads from stdin: the text, or nothing for a key.
    fn stdin(&self) -> Vec<u8> {
        match self {
            Self::Key(_) => Vec::new(),
            Self::Text(text) => text.as_bytes().to_vec(),
        }
    }
}

/// How much of a call went out.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct Sent {
    pub(super) chars: usize,
    pub(super) keys: usize,
}

impl Sent {
    fn add(&mut self, stroke: &Stroke) {
        match stroke {
            Stroke::Key(_) => self.keys += 1,
            Stroke::Text(text) => self.chars += text.chars().count(),
        }
    }

    const fn nothing(self) -> bool {
        self.chars == 0 && self.keys == 0
    }

    /// What went out, as the detail of a failure partway says it.
    fn describe(self, typing: &Typing) -> String {
        match typing {
            Typing::Keys(keys) => format!("pressed {} of {} keys", self.keys, keys.len()),
            Typing::Text { text, .. } => {
                format!(
                    "typed {} of {} characters",
                    self.chars,
                    text.chars().count()
                )
            }
        }
    }
}

/// `ctrl+shift+t` → wtype's modifier names and the key's keysym name.
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
        let wtype = MODIFIERS
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(part))
            .map(|(_, wtype)| *wtype)
            .ok_or_else(|| {
                format!("unknown modifier {part:?}; use shift, ctrl, alt, altgr or super")
            })?;
        if modifiers.contains(&wtype) {
            return Err(format!("modifier {part:?} appears twice in {combo:?}"));
        }
        modifiers.push(wtype);
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
        || "no window has keyboard focus".to_owned(),
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

/// Types `typing`, one `wtype` call per stroke. `observed` is `sent` once every stroke
/// went out, or `interrupted` when focus moved off the window that had it; then the
/// strokes after that one aren't typed, and the outcome says how much was.
pub(crate) async fn type_input(
    input: Input<'_>,
    typing: Typing,
    expect: Expect,
) -> Result<Outcome, CallError> {
    type_input_then(input, typing, expect, &mut None).await
}

/// `type_input`, where whatever commits the first stroke takes `aftercare` right before
/// the stroke can go out, and keeps the input-dirty marker until it has said the stroke
/// went out. `aftercare` is still there when nothing went out.
pub(crate) async fn type_input_then(
    input: Input<'_>,
    typing: Typing,
    expect: Expect,
    aftercare: &mut Option<Aftercare>,
) -> Result<Outcome, CallError> {
    typing.check()?;
    match input.keyboard.and_then(std::ffi::OsStr::to_str) {
        Some("native") => {
            return super::native::type_input(input, typing, expect, aftercare).await;
        }
        None if input.keyboard.is_none() => {}
        Some("wtype") => {}
        _ => {
            return Err(ToolError::new(
                ErrorName::Refused,
                "NIRI_COMPUTER_USE_KEYBOARD must be wtype or native",
            )
            .into());
        }
    }
    let mut waiter = niri::waiter(input.niri.events).await?;
    let focus = check_expect(&expect, waiter.view())?;
    if let Some(refused) = policy::refuse_input(input.policy, super::focused_app_id(waiter.view()))
    {
        return Err(refused.into());
    }
    let before = waiter.view().focused_window();
    let mut sent = Sent::default();
    for stroke in typing.strokes() {
        // wtype finds the display by name alone, so who serves it is checked right before.
        input
            .display
            .checked(input.niri.socket)
            .await
            .map_err(|error| partly(error, &typing, sent))?;
        run_wtype(
            input.runtime,
            typing.tool(),
            &stroke.args()?,
            stroke.stdin(),
            aftercare,
        )
        .await
        .map_err(|error| partly(error, &typing, sent))?;
        sent.add(&stroke);
        let moved = waiter
            .until(Duration::ZERO, |view| {
                (view.focused_window() != before).then_some(())
            })
            .await;
        let outcome = match moved {
            Waited::Timeout => continue,
            Waited::Done(()) => Outcome::seen(Observed::Interrupted, waiter.view(), Vec::new()),
            // wtype typed, but where focus went meanwhile is unknown.
            Waited::Lost(reason) => Outcome::uncertain(Some(true), Some(waiter.view()), reason),
        };
        return Ok(ended(outcome, focus, &typing, sent));
    }
    let done = Outcome::seen(Observed::Sent, waiter.view(), Vec::new());
    Ok(ended(done, focus, &typing, sent))
}

/// The outcome with what went out: for a call that stopped early, how many characters or
/// keys; for `submit`, whether `Return` was pressed.
pub(super) fn ended(outcome: Outcome, focus: Focus, typing: &Typing, sent: Sent) -> Outcome {
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
        ..outcome
    }
}

/// A stroke's failure, saying how much went out before it.
fn partly(error: ToolError, typing: &Typing, sent: Sent) -> ToolError {
    if sent.nothing() {
        return error;
    }
    ToolError::new(
        error.name,
        format!("{} before this: {}", sent.describe(typing), error.detail),
    )
}

/// Writes the marker, starts wtype behind the gate, records its PID, then feeds it in a
/// task that outlives this call if the call is dropped. That task takes `aftercare`.
async fn run_wtype(
    runtime: &RuntimeDir,
    tool: &str,
    args: &[String],
    stdin: Vec<u8>,
    aftercare: &mut Option<Aftercare>,
) -> Result<(), ToolError> {
    let mut marker = Written::write(runtime, Marker::pending(tool, Vec::new()))
        .await
        .map_err(|error| marker_error("write", &error))?;
    let gated = match runner::gated("wtype", args, &UTF8, WTYPE_DEADLINE) {
        Ok(gated) => gated,
        // Nothing started, so nothing was typed.
        Err(error) => return Err(cleared(marker, error).await),
    };
    let child = gated.pid().and_then(|pid| {
        let stat = procs::stat(Path::new("/proc"), pid)?;
        Some(Child {
            pid,
            start_time: stat.start_time,
        })
    });
    let recorded = match child {
        Some(child) => marker
            .update(|marker| {
                marker.phase = Phase::Running;
                marker.child = Some(child);
            })
            .await
            .map_err(|error| format!("record wtype in the input-dirty marker: {error}")),
        None => Err("wtype's PID or start time is unknown".to_owned()),
    };
    if let Err(detail) = recorded {
        // Still waiting at the gate: killing it now types nothing.
        drop(gated);
        let error = ToolError::new(ErrorName::UpstreamError, detail);
        return Err(cleared(marker, error).await);
    }
    // A task of its own, so wtype finishes and the marker comes off even if the call is
    // dropped.
    let aftercare = aftercare.take();
    let typed = cleanup::spawn(cleanup::deadline(), async move {
        feed(gated, &stdin, marker, aftercare).await
    });
    typed.await.map_err(|error| {
        ToolError::new(
            ErrorName::UpstreamError,
            format!("the wtype task ended: {error}"),
        )
    })?
}

/// Lets wtype past its gate and feeds it, between arming `aftercare` and saying the
/// stroke went out, which it says even after a failure, since wtype may have typed some.
async fn feed(
    gated: Gated,
    stdin: &[u8],
    marker: Written,
    aftercare: Option<Aftercare>,
) -> Result<(), ToolError> {
    let Some(aftercare) = aftercare else {
        return finish(gated.feed(stdin, MAX_STDOUT).await, marker).await;
    };
    let aftercare = match aftercare.arm().await {
        Ok(armed) => armed,
        Err(error) => {
            // Still waiting at the gate: killing it now types nothing.
            drop(gated);
            return Err(cleared(marker, error).await);
        }
    };
    let fed = gated.feed(stdin, MAX_STDOUT).await;
    aftercare.sent().await;
    finish(fed, marker).await
}

/// Removes the marker once wtype has exited by itself. A wtype killed by a signal, or
/// one past its deadline, which the runner kills, leaves it.
async fn finish(fed: Result<Finished, ToolError>, marker: Written) -> Result<(), ToolError> {
    let finished = match fed {
        Ok(finished) => finished,
        Err(error) => {
            return Err(ToolError::new(
                error.name,
                format!(
                    "{}; keys may be held, so the input-dirty marker stays until the user runs `niri-computer-use recover`",
                    error.detail
                ),
            ));
        }
    };
    if finished.status.code().is_none() {
        return Err(ToolError::new(
            ErrorName::UpstreamError,
            format!(
                "wtype ended with {}; keys may be held, so the input-dirty marker stays",
                finished.status
            ),
        ));
    }
    marker
        .clear()
        .await
        .map_err(|error| marker_error("remove", &error))?;
    if finished.status.success() {
        Ok(())
    } else {
        Err(finished.failure("wtype"))
    }
}

/// `error`, after removing the marker of a call that typed nothing; a marker that can't
/// be removed is added to the detail, since it blocks the next action.
async fn cleared(marker: Written, error: ToolError) -> ToolError {
    match marker.clear().await {
        Ok(()) => error,
        Err(clear) => ToolError::new(
            error.name,
            format!(
                "{}; the input-dirty marker couldn't be removed ({clear}), so actions refuse until the user runs `niri-computer-use recover`",
                error.detail
            ),
        ),
    }
}

fn marker_error(what: &str, error: &std::io::Error) -> ToolError {
    ToolError::new(
        ErrorName::UpstreamError,
        format!("{what} the input-dirty marker: {error}"),
    )
}

#[cfg(test)]
mod tests {
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
    fn a_combo_presses_modifiers_around_its_key() {
        let args = |combo: &str| Stroke::Key(combo.to_owned()).args().unwrap();
        assert_eq!(
            args("ctrl+shift+t"),
            [
                "-", "-M", "ctrl", "-M", "shift", "-k", "t", "-m", "shift", "-m", "ctrl"
            ]
        );
        assert_eq!(
            args("Super+Return"),
            ["-", "-M", "logo", "-k", "Return", "-m", "logo"]
        );
        assert_eq!(args("F12"), ["-", "-k", "F12"]);
        assert_eq!(Stroke::Text("hi".to_owned()).args().unwrap(), ["-"]);
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
        assert_eq!(
            keys(&["ctrl+l", "Return"]).strokes(),
            [
                Stroke::Key("ctrl+l".to_owned()),
                Stroke::Key("Return".to_owned())
            ]
        );
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
        assert!(matches!(
            text("", true).check(),
            Err(CallError::InvalidArguments(_))
        ));
        assert_eq!(Stroke::Text("é".to_owned()).stdin(), "é".as_bytes());
        assert_eq!(Stroke::Key("a".to_owned()).stdin(), b"");
    }

    #[test]
    fn long_text_goes_out_in_parts_of_a_hundred_characters_then_return_to_submit() {
        let parts = text(&"é".repeat(250), false).strokes();
        let lengths: Vec<usize> = parts
            .iter()
            .map(|stroke| match stroke {
                Stroke::Text(part) => part.chars().count(),
                Stroke::Key(key) => panic!("{key}"),
            })
            .collect();
        assert_eq!(lengths, [100, 100, 50]);
        let joined: String = parts
            .iter()
            .map(|part| String::from_utf8(part.stdin()).unwrap())
            .collect();
        assert_eq!(joined, "é".repeat(250));
        assert_eq!(
            text("hi", true).strokes(),
            [
                Stroke::Text("hi".to_owned()),
                Stroke::Key("Return".to_owned())
            ]
        );
    }

    #[test]
    fn the_outcome_says_how_much_went_out_and_whether_it_was_submitted() {
        let current = view(vec![window(1, Some("a"), 1, true)]);
        let report = |typing: &Typing, chars, keys| {
            let outcome = Outcome::seen(Observed::Interrupted, &current, Vec::new());
            let ended = ended(outcome, Focus::Matched, typing, Sent { chars, keys });
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
        let failed = partly(
            ToolError::new(ErrorName::UpstreamError, "wtype exited 1"),
            &message,
            Sent {
                chars: 200,
                keys: 0,
            },
        );
        assert_eq!(
            failed.detail,
            "typed 200 of 250 characters before this: wtype exited 1"
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

    #[tokio::test]
    async fn a_paste_key_its_keeper_doesnt_admit_is_never_let_past_the_gate() {
        let dir = crate::test_support::fresh_dir("feed-unadmitted");
        let runtime = RuntimeDir::of(&crate::test_support::niri_env(&dir)).unwrap();
        runtime.create().unwrap();
        let marker = Written::write(&runtime, Marker::pending("paste", Vec::new()))
            .await
            .unwrap();
        let typed = dir.join("typed");
        let script = format!("cat > '{}'", typed.display());
        let gated = runner::gated("sh", &["-c".to_owned(), script], &[], WTYPE_DEADLINE).unwrap();
        let (aftercare, _done) =
            crate::input::paste::fake_aftercare(crate::input::paste::STOPPED_WAITING).await;
        let error = feed(gated, b"ctrl+v", marker, Some(aftercare))
            .await
            .unwrap_err();
        assert!(
            error.detail.contains("nothing was pasted"),
            "{}",
            error.detail
        );
        // Killed at its gate, the stand-in for wtype read nothing.
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(std::fs::read(&typed).unwrap_or_default(), b"");
        // Nothing went out, so the marker is off.
        assert!(!runtime.input_dirty().unwrap());
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// The keeper hears `p` only once the key is out: a `p` before it would let the keeper
    /// restore the user's clipboard for the late key to paste.
    #[tokio::test]
    async fn a_paste_keys_keeper_hears_p_only_once_the_key_is_out() {
        let dir = crate::test_support::fresh_dir("feed-ordered");
        let runtime = RuntimeDir::of(&crate::test_support::niri_env(&dir)).unwrap();
        runtime.create().unwrap();
        let marker = Written::write(&runtime, Marker::pending("paste", Vec::new()))
            .await
            .unwrap();
        let out = dir.join("out");
        // The stand-in for wtype takes a moment for the key, then notes it is out.
        let wtype = format!("cat >/dev/null; sleep 0.2; : > '{}'", out.display());
        let gated = runner::gated("sh", &["-c".to_owned(), wtype], &[], WTYPE_DEADLINE).unwrap();
        let keeper = format!(
            r#"dd bs=1 count=1 status=none >/dev/null; echo '{{"report":"armed"}}'; c=$(dd bs=1 count=1 status=none); if [ -e '{}' ]; then when=after; else when=before; fi; echo "{{\"report\":\"done\",\"read\":true,\"clipboard\":\"restored\",\"detail\":\"$c $when the key\"}}""#,
            out.display()
        );
        let (aftercare, done) = crate::input::paste::fake_aftercare(&keeper).await;
        feed(gated, b"", marker, Some(aftercare)).await.unwrap();
        assert_eq!(
            done.await.unwrap().unwrap(),
            crate::input::paste::Report::Done {
                read: true,
                clipboard: crate::input::paste::Clipboard::Restored,
                detail: Some("p after the key".to_owned())
            }
        );
        assert!(!runtime.input_dirty().unwrap());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
