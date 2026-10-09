//! The keyboard tools' work (plan §6, §11): `key` and `type_text`, each one `wtype` call
//! behind the stdin gate. `wtype -` waits at its open stdin before it sends anything, so
//! the input-dirty marker names its PID before any key goes out.
//!
//! The call runs in a task of its own: a stop or a cancelled request drops the tool's
//! work, but a `wtype` already typing finishes within its deadline and then removes the
//! marker. Past the deadline its group is killed and the marker stays (plan §11).

use std::path::Path;
use std::time::Duration;

use serde::Serialize;

use crate::act::{Observed, Outcome};
use crate::control::marker::{Child, Marker, Phase, Written};
use crate::control::procs;
use crate::control::runtime::RuntimeDir;
use crate::error::{CallError, ErrorName, ToolError};
use crate::niri::waiter::View;
use crate::niri::{self, waiter::Waited};
use crate::policy;
use crate::runner::{self, Finished};

use super::Input;

/// Plan §6: about 0.4 s of wtype's sleeps at the 100-character cap, and C10's slowest
/// run of the corpus at well under half of this.
const WTYPE_DEADLINE: Duration = Duration::from_secs(3);
/// Counted in Unicode scalar values, as wtype batches them.
const MAX_TEXT: usize = 100;
/// wtype prints nothing on success.
const MAX_STDOUT: u64 = 64 * 1024;

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
    /// A combination such as `ctrl+shift+t`: modifiers, then one key's keysym name.
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
            Self::Key(_) => "key",
            Self::Text(_) => "type_text",
        }
    }

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

    fn check_length(&self) -> Result<(), CallError> {
        let Self::Text(text) = self else {
            return Ok(());
        };
        let length = text.chars().count();
        if length == 0 {
            return Err(CallError::InvalidArguments(
                "`text` must not be empty".to_owned(),
            ));
        }
        if length > MAX_TEXT {
            return Err(ToolError::new(
                ErrorName::TextTooLong,
                format!("{length} characters; at most {MAX_TEXT} per call, so split the text"),
            )
            .into());
        }
        Ok(())
    }
}

/// `ctrl+shift+t` → wtype's modifier names and the key's keysym name.
fn parse_combo(combo: &str) -> Result<(Vec<&'static str>, &str), String> {
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
fn check_expect(expect: &Expect, view: &View) -> Result<Focus, ToolError> {
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
    Err(ToolError::new(
        ErrorName::FocusMismatch,
        format!("expected {expect:?}; {actual}"),
    ))
}

/// Types `typing` with one `wtype` call. `observed` is `sent` once wtype has exited, or
/// `interrupted` when focus moved off the window that had it at any point meanwhile.
pub(crate) async fn type_input(
    input: Input<'_>,
    typing: Typing,
    expect: Expect,
) -> Result<Outcome, CallError> {
    typing.check_length()?;
    let args = typing.args()?;
    let mut waiter = niri::waiter(input.niri.events).await?;
    let focus = check_expect(&expect, waiter.view())?;
    if let Some(refused) = policy::refuse_input(input.policy, super::focused_app_id(waiter.view()))
    {
        return Err(refused.into());
    }
    let before = waiter.view().focused_window();
    run_wtype(input.runtime, &typing, args).await?;
    let moved = waiter
        .until(Duration::ZERO, |view| {
            (view.focused_window() != before).then_some(())
        })
        .await;
    let observed = match moved {
        Waited::Done(()) => Observed::Interrupted,
        Waited::Timeout | Waited::Lost(_) => Observed::Sent,
    };
    Ok(Outcome {
        focus: Some(focus),
        ..Outcome::seen(observed, waiter.view(), Vec::new())
    })
}

/// Writes the marker, starts wtype behind the gate, records its PID, then feeds it in a
/// task that outlives this call if the call is dropped.
async fn run_wtype(
    runtime: &RuntimeDir,
    typing: &Typing,
    args: Vec<String>,
) -> Result<(), ToolError> {
    let mut marker = Written::write(runtime, Marker::pending(typing.tool(), Vec::new()))
        .map_err(|error| marker_error("write", &error))?;
    let gated = match runner::gated("wtype", &args, WTYPE_DEADLINE) {
        Ok(gated) => gated,
        Err(error) => {
            // Nothing started, so nothing was typed.
            marker.clear().ok();
            return Err(error);
        }
    };
    let child = gated.pid().and_then(|pid| {
        let stat = procs::stat(Path::new("/proc"), pid)?;
        Some(Child {
            pid,
            start_time: stat.start_time,
        })
    });
    let recorded = child
        .ok_or_else(|| "wtype's PID or start time is unknown".to_owned())
        .and_then(|child| {
            marker
                .update(|marker| {
                    marker.phase = Phase::Running;
                    marker.child = Some(child);
                })
                .map_err(|error| format!("record wtype in the input-dirty marker: {error}"))
        });
    if let Err(detail) = recorded {
        // Still waiting at the gate: killing it now types nothing.
        drop(gated);
        marker.clear().ok();
        return Err(ToolError::new(ErrorName::UpstreamError, detail));
    }
    let stdin = typing.stdin();
    let typed = tokio::spawn(async move { finish(gated.feed(&stdin, MAX_STDOUT).await, marker) });
    typed.await.map_err(|error| {
        ToolError::new(
            ErrorName::UpstreamError,
            format!("the wtype task ended: {error}"),
        )
    })?
}

/// Removes the marker once wtype has exited by itself. A wtype killed by a signal, or
/// one past its deadline, which the runner kills, leaves it.
fn finish(fed: Result<Finished, ToolError>, marker: Written) -> Result<(), ToolError> {
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
        .map_err(|error| marker_error("remove", &error))?;
    if finished.status.success() {
        Ok(())
    } else {
        Err(finished.failure("wtype"))
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

    #[test]
    fn a_combo_presses_modifiers_around_its_key() {
        assert_eq!(
            Typing::Key("ctrl+shift+t".to_owned()).args().unwrap(),
            [
                "-", "-M", "ctrl", "-M", "shift", "-k", "t", "-m", "shift", "-m", "ctrl"
            ]
        );
        assert_eq!(
            Typing::Key("Super+Return".to_owned()).args().unwrap(),
            ["-", "-M", "logo", "-k", "Return", "-m", "logo"]
        );
        assert_eq!(
            Typing::Key("F12".to_owned()).args().unwrap(),
            ["-", "-k", "F12"]
        );
        assert_eq!(Typing::Text("hi".to_owned()).args().unwrap(), ["-"]);
        for bad in [
            "",
            "ctrl+",
            "ctrl+ctrl+a",
            "hyper+a",
            "ctrl+-",
            "a b",
            "ctrl+control+a",
        ] {
            assert!(Typing::Key(bad.to_owned()).args().is_err(), "{bad}");
        }
    }

    #[test]
    fn text_is_capped_in_scalar_values() {
        let text = |text: &str| Typing::Text(text.to_owned()).check_length();
        assert!(text(&"é".repeat(100)).is_ok());
        let long = text(&"→".repeat(101)).unwrap_err();
        let CallError::Tool(error) = long else {
            panic!("{long:?}")
        };
        assert_eq!(error.name, ErrorName::TextTooLong);
        assert!(
            error.detail.starts_with("101 characters"),
            "{}",
            error.detail
        );
        assert!(matches!(text(""), Err(CallError::InvalidArguments(_))));
        assert_eq!(Typing::Text("é".to_owned()).stdin(), "é".as_bytes());
        assert_eq!(Typing::Key("a".to_owned()).stdin(), b"");
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
}
