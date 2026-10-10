//! One client of the engine, and what its own environment decides for it alone.

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;

use crate::policy::{self, Loaded, Unrestricted};

/// What a client's own environment gives its session, and no other: its two variables,
/// its home, and the policy file as read from its config directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Given {
    /// `NIRI_COMPUTER_USE_UNRESTRICTED`, as given: `1` turns `unrestricted` on.
    pub(crate) unrestricted: Option<OsString>,
    /// `NIRI_COMPUTER_USE_KEYBOARD`: the experimental backend; absent means wtype.
    pub(crate) keyboard: Option<OsString>,
    /// `$HOME`, for a `capture_dir` under `~/`.
    pub(crate) home: Option<PathBuf>,
    pub(crate) policy: policy::Source,
}

impl Given {
    /// What `var` reads, with the policy file in `$XDG_CONFIG_HOME`, or `$HOME/.config`,
    /// read now. An empty variable counts as unset.
    pub(crate) fn read(var: impl Fn(&str) -> Option<OsString>) -> Self {
        let var = |name| var(name).filter(|value| !value.is_empty());
        let home = var("HOME").map(PathBuf::from);
        let config_dir = var("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| home.as_ref().map(|home| home.join(".config")));
        Self {
            unrestricted: var("NIRI_COMPUTER_USE_UNRESTRICTED"),
            keyboard: var("NIRI_COMPUTER_USE_KEYBOARD"),
            home,
            policy: policy::Source::read(config_dir.as_deref()),
        }
    }
}

/// A session's settings, fixed when it starts.
#[derive(Debug)]
pub(crate) struct Settings {
    /// The policy file, as the session read it when it started.
    pub(crate) policy: Loaded,
    /// Whether `unrestricted` is on, from the policy file and the session's variable.
    pub(crate) unrestricted: Unrestricted,
    /// The keyboard backend the session asked for.
    pub(crate) keyboard: Option<OsString>,
    /// The session's home, for a `capture_dir` under `~/`.
    pub(crate) home: Option<PathBuf>,
}

impl Settings {
    /// Checks what the session's environment gave.
    pub(crate) fn new(given: Given) -> Self {
        let env_on = Unrestricted::parse_env(given.unrestricted.as_deref());
        let policy = Loaded::from_source(&given.policy, env_on == Ok(true));
        Self {
            unrestricted: Unrestricted {
                policy: policy.unrestricted(),
                env: env_on,
            },
            policy,
            keyboard: given.keyboard,
            home: given.home,
        }
    }
}

/// Which session of the engine: what the lease, its refs and the window to give focus
/// back to belong to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct SessionId(pub(crate) u64);

/// A client: who it is, for the audit log and the lease record, and its settings.
#[derive(Debug, Clone)]
pub(crate) struct Session {
    id: SessionId,
    /// The process the client started: this server.
    pid: u32,
    settings: Arc<Settings>,
}

impl Session {
    /// The one client of a server that serves its own stdio.
    pub(crate) fn local(settings: Settings) -> Self {
        Self {
            id: SessionId(1),
            pid: std::process::id(),
            settings: Arc::new(settings),
        }
    }

    pub(crate) const fn id(&self) -> SessionId {
        self.id
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_policy_comes_from_the_sessions_own_config_directory() {
        let dir = crate::test_support::fresh_dir("session-given");
        for config in ["config", "home/.config"] {
            let file = dir.join(config).join("niri-computer-use");
            std::fs::create_dir_all(&file).unwrap();
            std::fs::write(file.join("policy.toml"), format!("# {config}")).unwrap();
        }
        let text = |given: Given| {
            let policy::Source::Text { text, .. } = given.policy else {
                panic!("{:?}", given.policy);
            };
            text
        };
        let home = dir.join("home").into_os_string();
        let config = dir.join("config").into_os_string();
        let both = Given::read(|name| match name {
            "HOME" => Some(home.clone()),
            "XDG_CONFIG_HOME" => Some(config.clone()),
            _ => None,
        });
        assert_eq!(both.home, Some(dir.join("home")));
        assert_eq!(text(both), "# config");
        // An empty variable counts as unset, so the file under `$HOME` is read.
        let home_only = Given::read(|name| match name {
            "HOME" => Some(home.clone()),
            "XDG_CONFIG_HOME" | "NIRI_COMPUTER_USE_UNRESTRICTED" => Some(OsString::new()),
            _ => None,
        });
        assert_eq!(home_only.unrestricted, None);
        assert_eq!(text(home_only), "# home/.config");
        assert_eq!(Given::read(|_| None).policy, policy::Source::NoConfigDir);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
