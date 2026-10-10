//! The control plane: the per-instance runtime directory with its stop flag, the lease
//! and its watcher, the input-dirty marker and `recover`, and the lock state, for `status`
//! now and for the action tools' lock gate later. The lock state's sources are logind's
//! `LockedHint` for `XDG_SESSION_ID`, then Noctalia's `locked`, and locked wins.
//!
//! niri sets logind's locked hint on lock and unlock when it runs as the session instance
//! (`src/niri.rs` at v26.04), whichever `ext_session_lock` client locks the screen.

pub(crate) mod cleanup;
pub(crate) mod desk;
pub(crate) mod guard;
pub(crate) mod lease;
pub(crate) mod marker;
pub(crate) mod procs;
pub(crate) mod recover;
pub(crate) mod runtime;
pub(crate) mod stop;

use std::path::Path;
use std::time::Duration;

use serde::Serialize;
use serde_json::{Map, Value};

use crate::niri::Socket;
use crate::{niri, runner};

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
    /// The logind session asked: the one niri runs in.
    session: Option<String>,
    /// Why logind couldn't answer, when it couldn't.
    logind_error: Option<String>,
}

impl Lock {
    pub(crate) const fn state(&self) -> LockState {
        self.state
    }
}

/// Asks logind about niri's session and reads `noctalia`'s status reply; locked wins.
pub(crate) async fn lock(niri_socket: &Socket, noctalia: Option<&Map<String, Value>>) -> Lock {
    let session = niri_session(niri_socket).await;
    let logind = match &session {
        Ok(id) => locked_hint(id).await,
        Err(error) => Err(error.clone()),
    };
    Lock {
        session: session.ok(),
        ..decide(logind, noctalia)
    }
}

/// The logind session niri sets its locked hint on: the `XDG_SESSION_ID` niri started
/// with (`update_locked_hint` in niri v26.04 `src/niri.rs`). Asking about any other session,
/// such as this server's own when it runs from SSH or a TTY, would read a hint niri never
/// sets. niri sets it only when started with `--session`; a plain `niri` inherits the
/// variable but leaves the hint at `no` while locked.
async fn niri_session(niri_socket: &Socket) -> Result<String, String> {
    let pid = niri::pid(niri_socket)
        .await
        .map_err(|error| format!("find niri's process: {}", error.detail))?;
    let proc_root = Path::new("/proc");
    let session_instance = procs::has_arg(proc_root, pid, "--session")
        .map_err(|error| format!("read niri's command line (PID {pid}): {error}"))?;
    if !session_instance {
        return Err("niri runs without --session, so it sets no logind locked hint".to_owned());
    }
    procs::environ_var(proc_root, pid, "XDG_SESSION_ID")
        .map_err(|error| format!("read niri's environment (PID {pid}): {error}"))?
        .ok_or_else(|| "niri has no XDG_SESSION_ID, so it sets no logind locked hint".to_owned())
}

/// `loginctl show-session <id> -p LockedHint --value`, which prints `yes` or `no`.
async fn locked_hint(id: &str) -> Result<bool, String> {
    // logind session IDs are short alphanumeric names such as `3` or `c2`. Anything else
    // could be read as an option, and `self` and `auto` name the caller's session, not
    // niri's.
    if id.is_empty()
        || id == "self"
        || id == "auto"
        || !id.chars().all(|c| c.is_ascii_alphanumeric())
    {
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

/// Locked if either source says so. niri sets logind's hint only while it runs, and only
/// from a session; Noctalia's `locked` also covers its own lock screen.
fn decide(logind: Result<bool, String>, noctalia: Option<&Map<String, Value>>) -> Lock {
    let noctalia = noctalia
        .and_then(|status| status.get("locked"))
        .and_then(Value::as_bool);
    let lock = |state, source, logind_error| Lock {
        state,
        source,
        session: None,
        logind_error,
    };
    match (logind, noctalia) {
        (Ok(true), _) => lock(LockState::Locked, LockSource::Logind, None),
        (Ok(false), Some(true)) => lock(LockState::Locked, LockSource::Noctalia, None),
        (Ok(false), _) => lock(LockState::Unlocked, LockSource::Logind, None),
        (Err(error), Some(true)) => lock(LockState::Locked, LockSource::Noctalia, Some(error)),
        (Err(error), Some(false)) => lock(LockState::Unlocked, LockSource::Noctalia, Some(error)),
        (Err(error), None) => lock(LockState::Unknown, LockSource::None, Some(error)),
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
    fn either_source_saying_locked_wins() {
        let locked = noctalia(Value::Bool(true));
        let other_session = decide(Ok(false), Some(&locked));
        assert_eq!(
            (other_session.state, other_session.source),
            (LockState::Locked, LockSource::Noctalia)
        );
        let unlocked = noctalia(Value::Bool(false));
        let logind = decide(Ok(true), Some(&unlocked));
        assert_eq!(
            (logind.state, logind.source),
            (LockState::Locked, LockSource::Logind)
        );
    }

    #[test]
    fn logind_comes_first_then_noctalia_then_unknown() {
        let locked = noctalia(Value::Bool(true));
        let logind = decide(Ok(false), Some(&noctalia(Value::Bool(false))));
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
        let noctalia_unlocked = decide(
            Err("no session".to_owned()),
            Some(&noctalia(Value::Bool(false))),
        );
        assert_eq!(
            (noctalia_unlocked.state, noctalia_unlocked.source),
            (LockState::Unlocked, LockSource::Noctalia)
        );
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
        for id in ["", "--help", "3 4", "c2;x", "self", "auto"] {
            let error = locked_hint(id).await.unwrap_err();
            assert!(
                error.contains("isn't a logind session ID"),
                "{id:?}: {error}"
            );
        }
    }
}
