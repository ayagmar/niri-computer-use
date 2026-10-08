//! Lock state, for `status` now and for the action tools' lock gate later. The sources, in
//! order: logind's `LockedHint` for `XDG_SESSION_ID`, then Noctalia's `locked`.
//!
//! niri sets logind's locked hint on lock and unlock when it runs as the session instance
//! (`src/niri.rs` at v26.04), whichever `ext_session_lock` client locks the screen.

use std::time::Duration;

use serde::Serialize;
use serde_json::{Map, Value};

use crate::runner;

const LOGINCTL_DEADLINE: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LockState {
    Locked,
    Unlocked,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LockSource {
    Logind,
    Noctalia,
    None,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Lock {
    state: LockState,
    source: LockSource,
    /// Why logind couldn't answer, when it couldn't.
    logind_error: Option<String>,
}

/// Asks logind, and falls back to `noctalia`'s status reply when logind can't answer.
pub(crate) async fn lock(session_id: Option<&str>, noctalia: Option<&Map<String, Value>>) -> Lock {
    let logind = match session_id {
        Some(id) => locked_hint(id).await,
        None => Err("XDG_SESSION_ID is not set".to_owned()),
    };
    decide(logind, noctalia)
}

/// `loginctl show-session <id> -p LockedHint --value`, which prints `yes` or `no`.
async fn locked_hint(id: &str) -> Result<bool, String> {
    // logind session IDs are short alphanumeric names such as `3` or `c2`. Anything else
    // could be read as an option.
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err(format!("XDG_SESSION_ID {id:?} isn't a logind session ID"));
    }
    let args = ["show-session", id, "-p", "LockedHint", "--value"].map(str::to_owned);
    let done = runner::run("loginctl", &args, LOGINCTL_DEADLINE, 1024)
        .await
        .map_err(|error| error.detail)?;
    if !done.status.success() {
        return Err(done.failure("loginctl").detail);
    }
    match String::from_utf8_lossy(&done.stdout).trim() {
        "yes" => Ok(true),
        "no" => Ok(false),
        other => Err(format!("loginctl printed {other:?} for LockedHint")),
    }
}

fn decide(logind: Result<bool, String>, noctalia: Option<&Map<String, Value>>) -> Lock {
    let state = |locked: bool| {
        if locked {
            LockState::Locked
        } else {
            LockState::Unlocked
        }
    };
    let logind_error = match logind {
        Ok(locked) => {
            return Lock {
                state: state(locked),
                source: LockSource::Logind,
                logind_error: None,
            };
        }
        Err(error) => Some(error),
    };
    match noctalia
        .and_then(|status| status.get("locked"))
        .and_then(Value::as_bool)
    {
        Some(locked) => Lock {
            state: state(locked),
            source: LockSource::Noctalia,
            logind_error,
        },
        None => Lock {
            state: LockState::Unknown,
            source: LockSource::None,
            logind_error,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noctalia(locked: Value) -> Map<String, Value> {
        let mut status = Map::new();
        status.insert("locked".to_owned(), locked);
        status
    }

    #[test]
    fn logind_comes_first_then_noctalia_then_unknown() {
        let locked = noctalia(Value::Bool(true));
        let logind = decide(Ok(false), Some(&locked));
        assert_eq!(
            (logind.state, logind.source),
            (LockState::Unlocked, LockSource::Logind)
        );
        let fallback = decide(Err("no session".to_owned()), Some(&locked));
        assert_eq!(
            (fallback.state, fallback.source),
            (LockState::Locked, LockSource::Noctalia)
        );
        assert_eq!(fallback.logind_error.as_deref(), Some("no session"));
        for status in [None, Some(&noctalia(Value::Null))] {
            let unknown = decide(Err("no session".to_owned()), status);
            assert_eq!(
                (unknown.state, unknown.source),
                (LockState::Unknown, LockSource::None)
            );
        }
    }

    #[tokio::test]
    async fn a_session_id_that_could_be_an_option_is_refused() {
        for id in ["", "--help", "3 4", "c2;x"] {
            let error = locked_hint(id).await.unwrap_err();
            assert!(
                error.contains("isn't a logind session ID"),
                "{id:?}: {error}"
            );
        }
    }
}
