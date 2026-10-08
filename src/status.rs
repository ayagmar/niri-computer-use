//! The readiness report shared by the `status` tool and the `status` subcommand.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::Env;
use crate::control::{self, Lock};
use crate::error::ToolError;
use crate::niri::events::StreamState;
use crate::niri::{self, version::Compat};
use crate::noctalia::{self, Presence};

/// Programs the server runs or will run, reported as found on `PATH` or not.
const BINARIES: [&str; 5] = ["grim", "wtype", "wl-copy", "wl-paste", "loginctl"];

#[derive(Debug, Serialize)]
pub(crate) struct Status {
    /// The basename of `NIRI_SOCKET`, which names the compositor instance.
    instance: Option<String>,
    niri: Niri,
    lock: Lock,
    noctalia: Presence,
    /// Why Noctalia counts as not running, when it's installed.
    noctalia_error: Option<ToolError>,
    binaries: BTreeMap<&'static str, bool>,
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

pub(crate) async fn collect(env: &Env, event_stream: Option<StreamState>) -> Status {
    let socket = env.niri_socket.as_deref();
    let installed = env.finds("noctalia");
    let (version, noctalia) = tokio::join!(niri::version(socket), async {
        if installed {
            Some(noctalia::status(env).await)
        } else {
            None
        }
    });
    let noctalia_status = noctalia.as_ref().and_then(|reply| reply.as_ref().ok());
    let lock = control::lock(env.session_id.as_deref(), noctalia_status).await;
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
        niri: Niri {
            compat: version.as_deref().map(niri::version::compat),
            version,
            ipc_crate: niri::version::IPC_CRATE,
            event_stream,
            error,
        },
        lock,
        noctalia: presence,
        noctalia_error,
        binaries: BINARIES
            .into_iter()
            .map(|name| (name, env.finds(name)))
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn reports_what_it_can_without_failing() {
        let status = serde_json::to_value(collect(&Env::default(), None).await).unwrap();
        assert_eq!(
            status,
            serde_json::json!({
                "instance": null,
                "niri": {
                    "version": null,
                    "ipc_crate": "26.4.0",
                    "compat": null,
                    "event_stream": null,
                    "error": {"error": "niri_unavailable", "detail": "NIRI_SOCKET is not set"}
                },
                "lock": {
                    "state": "unknown",
                    "source": "none",
                    "logind_error": "XDG_SESSION_ID is not set"
                },
                "noctalia": "not_installed",
                "noctalia_error": null,
                "binaries": {
                    "grim": false, "loginctl": false, "wl-copy": false, "wl-paste": false, "wtype": false
                }
            })
        );
    }
}
