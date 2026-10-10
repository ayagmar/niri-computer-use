//! One client of the engine, and what its own environment decides for it alone.

use std::ffi::OsString;
use std::sync::Arc;

use crate::Env;
use crate::policy::{Loaded, Unrestricted};

/// The variables a client's environment sets for its own session, never for another's.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Vars {
    /// `NIRI_COMPUTER_USE_UNRESTRICTED`, as given: `1` turns `unrestricted` on.
    pub(crate) unrestricted: Option<OsString>,
    /// `NIRI_COMPUTER_USE_KEYBOARD`: the experimental backend; absent means wtype.
    pub(crate) keyboard: Option<OsString>,
}

impl Vars {
    /// The variables `var` reads. An empty variable counts as unset.
    pub(crate) fn read(var: impl Fn(&str) -> Option<OsString>) -> Self {
        let var = |name| var(name).filter(|value| !value.is_empty());
        Self {
            unrestricted: var("NIRI_COMPUTER_USE_UNRESTRICTED"),
            keyboard: var("NIRI_COMPUTER_USE_KEYBOARD"),
        }
    }
}

/// A session's settings, fixed when it starts.
#[derive(Debug)]
pub(crate) struct Settings {
    /// The policy file, read when the session starts.
    pub(crate) policy: Loaded,
    /// Whether `unrestricted` is on, from the policy file and the session's variable.
    pub(crate) unrestricted: Unrestricted,
    /// The keyboard backend the session asked for.
    pub(crate) keyboard: Option<OsString>,
}

impl Settings {
    /// Reads the policy file in `env`'s config directory now, with the session's `vars`.
    pub(crate) fn read(env: &Env, vars: Vars) -> Self {
        let env_on = Unrestricted::parse_env(vars.unrestricted.as_deref());
        let policy = env.policy(env_on == Ok(true));
        Self {
            unrestricted: Unrestricted {
                policy: policy.unrestricted(),
                env: env_on,
            },
            policy,
            keyboard: vars.keyboard,
        }
    }
}

/// A client: who it is, for the audit log and the lease record, and its settings.
#[derive(Debug, Clone)]
pub(crate) struct Session {
    /// The process the client started: this server.
    pid: u32,
    settings: Arc<Settings>,
}

impl Session {
    /// The one client of a server that serves its own stdio.
    pub(crate) fn local(settings: Settings) -> Self {
        Self {
            pid: std::process::id(),
            settings: Arc::new(settings),
        }
    }

    pub(crate) fn settings(&self) -> &Settings {
        &self.settings
    }

    /// The label for the audit log and the lease: the MCP client's name and the process it
    /// started, such as `claude-code/4711`.
    pub(crate) fn label(&self, client: &str) -> String {
        format!("{client}/{}", self.pid)
    }
}
