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

use crate::desks::record;
use crate::niri::Socket;
use crate::runner::{self, Desktop, Search};
use crate::{Env, niri, noctalia};

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
    /// For a desk: why the host's recorded Noctalia couldn't answer, when it couldn't.
    #[serde(skip_serializing_if = "Option::is_none")]
    noctalia_error: Option<String>,
}

impl Lock {
    pub(crate) const fn state(&self) -> LockState {
        self.state
    }
}

/// Where the lock state is read: logind's hint for niri's session, and Noctalia's status
/// when Noctalia is installed.
#[derive(Debug, Clone, Copy)]
pub(crate) struct LockSources<'a> {
    pub(crate) env: &'a Env,
    pub(crate) noctalia: bool,
}

impl LockSources<'_> {
    /// The lock state from reads started now, side by side: Noctalia's status and logind's
    /// hint. A desk's is the host session's, from the sources its `host.json` records.
    pub(crate) async fn read(self) -> Lock {
        let status = async {
            if self.noctalia && self.env.desk().is_none() {
                noctalia::status(self.env).await.ok()
            } else {
                None
            }
        };
        let (status, reading) = tokio::join!(status, Reading::of(self.env));
        reading.lock(status.as_ref())
    }
}

/// What the lock state is made of, but the host's own shell status, which its callers
/// read anyway: logind's answer on the host; the whole state for a desk.
#[derive(Debug)]
pub(crate) enum Reading {
    Host(Logind),
    Desk(Lock),
}

impl Reading {
    pub(crate) async fn of(env: &Env) -> Self {
        match env.desk() {
            Some(desk) => Self::Desk(desk_lock(&desk.host.lock, &env.desktop).await),
            None => Self::Host(logind(&env.host_niri, &env.desktop).await),
        }
    }

    /// The lock state, with the host's Noctalia `status` on the host; a desk never reads
    /// its own.
    pub(crate) fn lock(self, status: Option<&Map<String, Value>>) -> Lock {
        match self {
            Self::Host(logind) => lock(logind, status),
            Self::Desk(lock) => lock,
        }
    }
}

/// Where a desk's `host.json` says the host's lock state is read: the logind session the
/// host's niri sets its hint on, and the host's Noctalia's socket when the host has
/// Noctalia. Refused when neither exists, since a desk's server could never act.
pub(crate) async fn host_lock_sources(env: &Env) -> Result<record::LockSources, String> {
    let session = niri_session(&env.host_niri).await;
    let noctalia = env
        .desktop
        .noctalia
        .is_some()
        .then(|| noctalia::socket(env))
        .flatten();
    match (session, noctalia) {
        (Err(error), None) => Err(format!(
            "a desk's server follows the host's lock state, and the host has no source of it: logind: {error}; Noctalia: not installed"
        )),
        (session, noctalia) => Ok(record::LockSources {
            logind_session: session.ok(),
            noctalia,
        }),
    }
}

/// A desk's lock state, from exactly the host's recorded `sources`. A recorded source that
/// can't be read leaves the state unknown, unless the other says locked.
async fn desk_lock(sources: &record::LockSources, desktop: &Desktop) -> Lock {
    let logind = async {
        match &sources.logind_session {
            Some(id) => Some(locked_hint(desktop, id).await),
            None => None,
        }
    };
    let noctalia = async {
        match &sources.noctalia {
            Some(socket) => Some(noctalia_locked(socket).await),
            None => None,
        }
    };
    let (logind, noctalia) = tokio::join!(logind, noctalia);
    Lock {
        session: sources.logind_session.clone(),
        ..decide_desk(logind.as_ref(), noctalia.as_ref())
    }
}

/// Noctalia's `locked` at `socket`.
async fn noctalia_locked(socket: &Path) -> Result<bool, String> {
    let status = noctalia::status_at(socket)
        .await
        .map_err(|error| error.detail)?;
    status
        .get("locked")
        .and_then(Value::as_bool)
        .ok_or_else(|| {
            format!(
                "Noctalia's status at {} says nothing of locked",
                socket.display()
            )
        })
}

/// A desk's lock state from its recorded sources' answers, `None` for one not recorded:
/// locked if either says so; else unknown if either couldn't answer, or none was
/// recorded; else unlocked.
fn decide_desk(
    logind: Option<&Result<bool, String>>,
    noctalia: Option<&Result<bool, String>>,
) -> Lock {
    let error = |read: Option<&Result<bool, String>>| read.and_then(|read| read.clone().err());
    let lock = |state, source| Lock {
        state,
        source,
        session: None,
        logind_error: error(logind),
        noctalia_error: error(noctalia),
    };
    match (logind, noctalia) {
        (Some(Ok(true)), _) => lock(LockState::Locked, LockSource::Logind),
        (_, Some(Ok(true))) => lock(LockState::Locked, LockSource::Noctalia),
        (Some(Err(_)), _) | (_, Some(Err(_))) => lock(LockState::Unknown, LockSource::None),
        (Some(Ok(false)), _) => lock(LockState::Unlocked, LockSource::Logind),
        (None, Some(Ok(false))) => lock(LockState::Unlocked, LockSource::Noctalia),
        (None, None) => Lock {
            logind_error: Some("the desk's host.json records no lock source".to_owned()),
            ..lock(LockState::Unknown, LockSource::None)
        },
    }
}

/// What logind says about niri's session.
#[derive(Debug)]
pub(crate) struct Logind {
    session: Result<String, String>,
    hint: Result<bool, String>,
}

/// Asks logind for the locked hint of niri's session, through the desktop's `loginctl`.
pub(crate) async fn logind(niri_socket: &Socket, desktop: &Desktop) -> Logind {
    let session = niri_session(niri_socket).await;
    let hint = match &session {
        Ok(id) => locked_hint(desktop, id).await,
        Err(error) => Err(error.clone()),
    };
    Logind { session, hint }
}

/// The lock state from `logind`'s answer and `noctalia`'s status reply; locked wins.
pub(crate) fn lock(logind: Logind, noctalia: Option<&Map<String, Value>>) -> Lock {
    Lock {
        session: logind.session.ok(),
        ..decide(logind.hint, noctalia)
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
async fn locked_hint(desktop: &Desktop, id: &str) -> Result<bool, String> {
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
    let Some(loginctl) = &desktop.loginctl else {
        return Err(format!("no loginctl in {}", desktop.search));
    };
    let args = ["show-session", id, "-p", "LockedHint", "--value"].map(str::to_owned);
    let done = runner::run(loginctl, Search::Own, &args, LOGINCTL_DEADLINE, 1024)
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
        noctalia_error: None,
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
            let error = locked_hint(&Desktop::default(), id).await.unwrap_err();
            assert!(
                error.contains("isn't a logind session ID"),
                "{id:?}: {error}"
            );
        }
    }

    /// Serves Noctalia's `status` at `socket`, saying `locked`.
    fn noctalia_at(socket: &Path, locked: bool) {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        let listener = tokio::net::UnixListener::bind(socket).unwrap();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let mut request = Vec::new();
                stream.read_to_end(&mut request).await.ok();
                let status = format!(r#"{{"locked": {locked}}}"#);
                stream.write_all(status.as_bytes()).await.ok();
            }
        });
    }

    /// Catches a desk whose server reads its own session's lock state, which no lock screen
    /// ever locks, so an agent would act on the desk while the owner's screen is locked; one
    /// that drops the host's recorded Noctalia because the joining server's own search
    /// doesn't find Noctalia; and one that acts when a recorded source can't be read.
    #[tokio::test]
    async fn a_desks_lock_state_is_the_recorded_hosts_and_unknown_refuses() {
        let dir = crate::test_support::short_dir("lock");
        let host_noctalia = dir.join("noctalia-wayland-0.sock");
        let recorded = record::LockSources {
            logind_session: None,
            noctalia: Some(host_noctalia.clone()),
        };
        let env = crate::test_support::desk_env(&dir, dir.join("host-niri.sock"), recorded);
        // The joining server finds no Noctalia of its own.
        assert_eq!(env.desktop.noctalia, None);
        // The desk's own display names a Noctalia that says unlocked; it must not count.
        noctalia_at(&noctalia::socket(&env).unwrap(), false);
        let sources = LockSources {
            env: &env,
            noctalia: false,
        };
        let unread = sources.read().await;
        assert_eq!(unread.state(), LockState::Unknown, "{unread:?}");
        assert!(unread.noctalia_error.is_some(), "{unread:?}");
        assert!(crate::policy::refuse_lock(unread.state()).is_some());

        noctalia_at(&host_noctalia, true);
        let locked = sources.read().await;
        assert_eq!(
            (locked.state(), locked.source),
            (LockState::Locked, LockSource::Noctalia)
        );
        assert_eq!(
            crate::policy::refuse_lock(locked.state()).unwrap().name,
            crate::error::ErrorName::ScreenLocked
        );

        // A recorded logind session this server can't ask about, with Noctalia unlocked.
        let unlocked = dir.join("noctalia-wayland-1.sock");
        noctalia_at(&unlocked, false);
        let half_recorded = record::LockSources {
            logind_session: Some("4".to_owned()),
            noctalia: Some(unlocked),
        };
        let half_env =
            crate::test_support::desk_env(&dir, dir.join("host-niri.sock"), half_recorded);
        let half = LockSources {
            env: &half_env,
            noctalia: false,
        }
        .read()
        .await;
        assert_eq!(half.state(), LockState::Unknown, "{half:?}");
        assert!(half.logind_error.is_some(), "{half:?}");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_desk_is_unlocked_only_when_every_recorded_source_says_so() {
        let state = |logind: Option<Result<bool, String>>,
                     noctalia: Option<Result<bool, String>>| {
            decide_desk(logind.as_ref(), noctalia.as_ref()).state
        };
        let failed = || Some(Err("unreadable".to_owned()));
        assert_eq!(state(Some(Ok(true)), failed()), LockState::Locked);
        assert_eq!(state(failed(), Some(Ok(true))), LockState::Locked);
        assert_eq!(state(Some(Ok(false)), failed()), LockState::Unknown);
        assert_eq!(state(failed(), None), LockState::Unknown);
        assert_eq!(state(None, None), LockState::Unknown);
        assert_eq!(state(Some(Ok(false)), Some(Ok(false))), LockState::Unlocked);
        assert_eq!(state(None, Some(Ok(false))), LockState::Unlocked);
        assert_eq!(state(Some(Ok(false)), None), LockState::Unlocked);
    }
}
