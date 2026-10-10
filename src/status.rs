//! The readiness report shared by the `status` tool and the `status` subcommand.

use std::collections::BTreeMap;

use niri_ipc::Output;
use serde::Serialize;

use crate::audit::{Audit, AuditStatus};
use crate::control::desk::LeaseStatus;
use crate::control::marker::{self, Found};
use crate::control::runtime::RuntimeDir;
use crate::control::{self, Lock};
use crate::error::ToolError;
use crate::niri::events::StreamState;
use crate::niri::{self, version::Compat};
use crate::noctalia::{self, Presence};
use crate::policy::{self, Facts, Loaded, PolicyStatus, Unrestricted, UnrestrictedStatus};
use crate::{Env, a11y, discover};

/// Programs the server runs, reported as found on `PATH` or not.
const BINARIES: [&str; 4] = ["grim", "wtype", "wl-paste", "loginctl"];

#[derive(Debug, Serialize)]
pub(crate) struct Status {
    /// The basename of `NIRI_SOCKET`, which names the compositor instance.
    instance: Option<String>,
    /// Where the runtime directory, niri's socket and the display came from: the
    /// environment, or discovery, or why neither.
    discovery: discover::Sources,
    /// The engine serving this session; null from the `status` subcommand.
    engine: Option<EngineStatus>,
    /// Why input, screenshots and clipboard reads would be refused now: the Wayland display
    /// isn't niri's, or couldn't be checked. Null when it is niri's. Checked afresh for each
    /// report.
    display_error: Option<ToolError>,
    niri: Niri,
    lease: LeaseStatus,
    /// Whether the stop flag is set for this niri instance.
    stop: bool,
    /// The input-dirty marker, if there is one.
    input_dirty: Option<Found>,
    lock: Lock,
    outputs: OutputSupport,
    noctalia: Presence,
    /// Why Noctalia counts as not running, when it's installed.
    noctalia_error: Option<ToolError>,
    /// Whether this session has an accessibility bus, for `elements`.
    accessibility: a11y::Presence,
    audit: AuditStatus,
    policy: PolicyStatus,
    /// Whether gated actions are allowed, and whether the policy file or the environment
    /// allowed them.
    unrestricted: UnrestrictedStatus,
    binaries: BTreeMap<&'static str, bool>,
}

/// What the caller knows that `status` reports: the event stream's state (none in the
/// subcommand, which opens no stream), whether Noctalia is installed (decided once at
/// startup for the server, whose tool list depends on it), the lease (from the server's
/// desk, or from the runtime directory in the subcommand), and the policy file as loaded.
#[derive(Debug)]
pub(crate) struct Sources<'a> {
    /// The engine serving the session; none for the `status` subcommand.
    pub(crate) engine: Option<EngineStatus>,
    pub(crate) event_stream: Option<StreamState>,
    pub(crate) audit: &'a Audit,
    pub(crate) noctalia_installed: bool,
    pub(crate) lease: LeaseStatus,
    pub(crate) policy: &'a Loaded,
    pub(crate) unrestricted: &'a Unrestricted,
    /// Decided once at startup for the server, whose tool list depends on it.
    pub(crate) accessibility: &'a a11y::Presence,
}

/// The engine that serves the session asking.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct EngineStatus {
    pub(crate) pid: u32,
    pub(crate) mode: Mode,
    /// The sessions it serves, this one included.
    pub(crate) sessions: usize,
    /// Why a client that asked for shared mode is served standalone.
    pub(crate) fallback: Option<String>,
}

/// How a server serves its clients.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Mode {
    /// One client, over stdin and stdout.
    Standalone,
    /// Every client of the niri instance, over the instance socket.
    Shared,
}

/// Whether the pointer tools may run on niri's outputs right now.
#[derive(Debug, Serialize)]
struct OutputSupport {
    pointer_supported: bool,
    /// Why not, when they may not.
    reason: Option<String>,
}

impl OutputSupport {
    fn of(outputs: Result<BTreeMap<String, Output>, ToolError>) -> Self {
        let checked = outputs.and_then(|outputs| policy::pointer_support(outputs.values()));
        Self {
            pointer_supported: checked.is_ok(),
            reason: checked.err().map(|error| error.detail),
        }
    }
}

#[derive(Debug, Serialize)]
struct Niri {
    version: Option<String>,
    ipc_crate: &'static str,
    compat: Option<Compat>,
    /// Null in the `status` subcommand, which keeps no stream open.
    event_stream: Option<StreamState>,
    error: Option<ToolError>,
}

pub(crate) async fn collect(env: &Env, sources: Sources<'_>) -> Status {
    let Sources {
        engine,
        event_stream,
        audit,
        noctalia_installed,
        lease,
        policy,
        unrestricted,
        accessibility,
    } = sources;
    let socket = &env.niri_socket;
    let (version, outputs, noctalia) =
        tokio::join!(niri::version(socket), niri::outputs(socket), async {
            if noctalia_installed {
                Some(noctalia::status(env).await)
            } else {
                None
            }
        });
    let noctalia_status = noctalia.as_ref().and_then(|reply| reply.as_ref().ok());
    let lock = control::lock(&env.niri_socket, noctalia_status).await;
    let display_error = env.display.checked(socket).await.err();
    let (version, error) = match version {
        Ok(version) => (Some(version), None),
        Err(error) => (None, Some(error)),
    };
    let (presence, noctalia_error) = match noctalia {
        None => (Presence::NotInstalled, None),
        Some(Ok(_)) => (Presence::Running, None),
        Some(Err(failure)) => (Presence::NotRunning, Some(failure)),
    };
    Status {
        instance: env.instance(),
        discovery: env.discovery.clone(),
        engine,
        display_error,
        niri: Niri {
            compat: version.as_deref().map(niri::version::compat),
            version,
            ipc_crate: niri::version::IPC_CRATE,
            event_stream,
            error,
        },
        lease,
        input_dirty: RuntimeDir::of(env)
            .ok()
            .and_then(|runtime| marker::read(&runtime)),
        stop: RuntimeDir::of(env).is_ok_and(|runtime| runtime.stopped().unwrap_or(true)),
        lock,
        outputs: OutputSupport::of(outputs),
        noctalia: presence,
        noctalia_error,
        accessibility: accessibility.clone(),
        audit: audit.status(),
        policy: policy.status(),
        unrestricted: unrestricted.status(),
        binaries: BINARIES
            .into_iter()
            .map(|name| (name, env.finds(name)))
            .collect(),
    }
}

impl Status {
    /// What the lease decision needs from this report.
    pub(crate) const fn facts<'a>(&'a self, policy: &'a Loaded) -> Facts<'a> {
        Facts {
            compat: self.niri.compat,
            niri_error: self.niri.error.as_ref(),
            event_stream: self.niri.event_stream,
            policy,
            lock: self.lock.state(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::desk::status_without_desk;

    #[tokio::test]
    async fn reports_what_it_can_without_failing() {
        let audit = Audit::new(None);
        let sources = Sources {
            engine: None,
            event_stream: None,
            audit: &audit,
            noctalia_installed: false,
            lease: status_without_desk(&Env::default()),
            policy: &Loaded::Missing,
            unrestricted: &Unrestricted {
                policy: false,
                env: Ok(false),
            },
            accessibility: &a11y::detect(None).await,
        };
        let status = serde_json::to_value(collect(&Env::default(), sources).await).unwrap();
        assert_eq!(
            status,
            serde_json::json!({
                "instance": null,
                "discovery": {
                    "runtime_dir": {"source": "missing", "detail": "XDG_RUNTIME_DIR is not set"},
                    "niri_socket": {"source": "missing", "detail": "NIRI_SOCKET is not set"},
                    "wayland_display": {"source": "missing", "detail": "WAYLAND_DISPLAY is not set"},
                    "warning": null
                },
                "engine": null,
                "display_error": {
                    "error": "upstream_error",
                    "detail": "WAYLAND_DISPLAY is not set"
                },
                "niri": {
                    "version": null,
                    "ipc_crate": "26.4.0",
                    "compat": null,
                    "event_stream": null,
                    "error": {"error": "niri_unavailable", "detail": "NIRI_SOCKET is not set"}
                },
                "lease": {"held_by_me": false, "holder": null, "error": "NIRI_SOCKET is not set"},
                "stop": false,
                "input_dirty": null,
                "lock": {
                    "state": "unknown",
                    "source": "none",
                    "session": null,
                    "logind_error": "find niri's process: NIRI_SOCKET is not set"
                },
                "outputs": {"pointer_supported": false, "reason": "NIRI_SOCKET is not set"},
                "noctalia": "not_installed",
                "noctalia_error": null,
                "accessibility": {
                    "available": false,
                    "address": null,
                    "reason": "neither DBUS_SESSION_BUS_ADDRESS nor XDG_RUNTIME_DIR is set"
                },
                "audit": {"path": null, "last_error": "neither XDG_STATE_HOME nor HOME is set"},
                "policy": {
                    "state": "missing", "presets": 0, "preset_names": [], "denied_app_ids": 0,
                    "capture_dir": null,
                    "error": null
                },
                "unrestricted": {"enabled": false, "source": null, "error": null},
                "binaries": {
                    "grim": false, "loginctl": false, "wl-paste": false, "wtype": false
                }
            })
        );
    }
}
