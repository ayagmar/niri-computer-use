//! The readiness report shared by the `status` tool and the `status` subcommand.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;

use serde::Serialize;

use crate::Env;
use crate::error::ToolError;
use crate::niri::events::StreamState;
use crate::niri::{self, version::Compat};

/// Programs the server runs or will run, reported as found on `PATH` or not.
const BINARIES: [&str; 5] = ["grim", "wtype", "wl-copy", "wl-paste", "loginctl"];

#[derive(Debug, Serialize)]
pub(crate) struct Status {
    /// The basename of `NIRI_SOCKET`, which names the compositor instance.
    instance: Option<String>,
    niri: Niri,
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
    let (version, error) = match niri::version(socket).await {
        Ok(version) => (Some(version), None),
        Err(error) => (None, Some(error)),
    };
    Status {
        instance: socket
            .and_then(Path::file_name)
            .map(|name| name.to_string_lossy().into_owned()),
        niri: Niri {
            compat: version.as_deref().map(niri::version::compat),
            version,
            ipc_crate: niri::version::IPC_CRATE,
            event_stream,
            error,
        },
        binaries: BINARIES
            .into_iter()
            .map(|name| (name, on_path(env.path.as_deref(), name)))
            .collect(),
    }
}

/// Whether `PATH` has an executable file called `name`.
fn on_path(path: Option<&OsStr>, name: &str) -> bool {
    path.into_iter().flat_map(std::env::split_paths).any(|dir| {
        dir.join(name)
            .metadata()
            .is_ok_and(|file| file.is_file() && file.permissions().mode() & 0o111 != 0)
    })
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::fs;

    use super::*;

    #[test]
    fn finds_only_executable_files_on_path() {
        let dir =
            std::env::temp_dir().join(format!("niri-desktop-mcp-path-{}", std::process::id()));
        fs::create_dir_all(dir.join("dir-named-grim/grim")).unwrap();
        fs::write(dir.join("wtype"), "").unwrap();
        fs::set_permissions(dir.join("wtype"), fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(dir.join("loginctl"), "").unwrap();
        fs::set_permissions(dir.join("loginctl"), fs::Permissions::from_mode(0o644)).unwrap();
        let path = std::env::join_paths(["/nonexistent".into(), dir.clone()]).unwrap();

        assert!(on_path(Some(&path), "wtype"));
        assert!(!on_path(Some(&path), "loginctl"));
        assert!(!on_path(Some(&path), "grim"));
        assert!(!on_path(Some(&path), "wl-copy"));
        assert!(!on_path(None, "wtype"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn reports_an_unknown_niri_instead_of_failing() {
        let env = Env {
            niri_socket: None,
            path: Some(OsString::new()),
        };
        let status = serde_json::to_value(collect(&env, None).await).unwrap();
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
                "binaries": {
                    "grim": false, "loginctl": false, "wl-copy": false, "wl-paste": false, "wtype": false
                }
            })
        );
    }
}
