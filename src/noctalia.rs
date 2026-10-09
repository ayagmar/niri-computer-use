//! The only module that talks to Noctalia, through its IPC socket.
//!
//! Noctalia 5.2.1's client writes `<cwd>\x1e<command>`, shuts down its write half and
//! reads the reply until EOF (`src/ipc/ipc_client.cpp`). The service erases everything up
//! to the first `\x1e` before parsing (`src/ipc/ipc_service.cpp`), so the server only
//! ever sends fixed payloads, never text from a tool's arguments: `status`, and
//! `panel-open` or `panel-close` with an allowlisted panel.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Map, Value};
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};
use tokio::net::UnixStream;

use crate::Env;
use crate::error::{ErrorName, ToolError, Unanswered};
use crate::policy::Panel;

const DEADLINE: Duration = Duration::from_secs(2);
/// `/` as the caller's directory and the separator, before the command.
const CWD_PREFIX: &[u8] = b"/\x1e";
/// A `status` reply is a few hundred bytes.
const MAX_REPLY: u64 = 64 * 1024;

/// Every command the server sends. Each is built from fixed words, so none can carry a
/// separator or a newline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Command {
    Status,
    PanelOpen(Panel),
    PanelClose(Panel),
}

impl Command {
    fn payload(self) -> Vec<u8> {
        let command = match self {
            Self::Status => "status".to_owned(),
            Self::PanelOpen(panel) => format!("panel-open {}", panel.id()),
            Self::PanelClose(panel) => format!("panel-close {}", panel.id()),
        };
        [CWD_PREFIX, command.as_bytes()].concat()
    }
}

/// Whether Noctalia is installed and answering, for `status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Presence {
    Running,
    NotRunning,
    NotInstalled,
}

/// `$XDG_RUNTIME_DIR/noctalia-$WAYLAND_DISPLAY.sock`, where Noctalia listens.
pub(crate) fn socket(env: &Env) -> Option<PathBuf> {
    let display = env.wayland_display.as_deref()?.to_str()?;
    if display.contains('/') {
        return None;
    }
    Some(
        env.runtime_dir
            .as_deref()?
            .join(format!("noctalia-{display}.sock")),
    )
}

/// Noctalia's `status` reply, a JSON object with `barVisible`, `panelOpen`,
/// `activePanelId` and `locked`. Anything else, including no answer, means Noctalia
/// isn't usable: `noctalia_unavailable`, except an explicit `error:` reply, which keeps
/// Noctalia's text as an `upstream_error`.
pub(crate) async fn status(env: &Env) -> Result<Map<String, Value>, ToolError> {
    let socket = socket(env).ok_or_else(|| {
        unavailable("WAYLAND_DISPLAY or XDG_RUNTIME_DIR doesn't name a Noctalia socket")
    })?;
    let reply = tokio::time::timeout(DEADLINE, request(&socket, Command::Status))
        .await
        .unwrap_or_else(|_| Err(format!("no reply within {DEADLINE:?}")))
        .map_err(|error| unavailable(format!("{}: {error}", socket.display())))?;
    interpret(&reply)
}

/// `activePanelId` from a `status` reply: the open panel's id, or `None`. A reply without
/// it isn't the status this server knows.
pub(crate) fn active_panel(status: &Map<String, Value>) -> Result<Option<String>, ToolError> {
    match status.get("activePanelId") {
        Some(Value::Null) => Ok(None),
        Some(Value::String(panel)) => Ok(Some(panel.clone())),
        _ => Err(unavailable(format!(
            "Noctalia's status has no activePanelId string or null: {}",
            Value::Object(status.clone())
        ))),
    }
}

/// Sends `panel-open` or `panel-close` and requires Noctalia's `ok`. Noctalia carries the
/// command out before it replies (`PanelManager::registerIpc`), so a lost reply leaves the
/// panel's state unknown.
pub(crate) async fn change_panel(env: &Env, command: Command) -> Result<(), Unanswered> {
    let socket = socket(env).ok_or_else(|| {
        Unanswered::Refused(unavailable(
            "WAYLAND_DISPLAY or XDG_RUNTIME_DIR doesn't name a Noctalia socket",
        ))
    })?;
    let lost = |error: String| ToolError::new(ErrorName::UpstreamError, error);
    let reply = tokio::time::timeout(DEADLINE, async {
        let stream = UnixStream::connect(&socket).await.map_err(|error| {
            Unanswered::Refused(unavailable(format!(
                "{}: connect: {error}",
                socket.display()
            )))
        })?;
        exchange(stream, command)
            .await
            .map_err(|error| Unanswered::Lost(lost(format!("{}: {error}", socket.display()))))
    })
    .await
    .map_err(|_| Unanswered::Lost(lost(format!("no reply from Noctalia within {DEADLINE:?}"))))??;
    acknowledged(&reply)
}

fn acknowledged(reply: &[u8]) -> Result<(), Unanswered> {
    let text = String::from_utf8_lossy(reply);
    if text == "ok\n" {
        return Ok(());
    }
    let error = ToolError::new(
        ErrorName::UpstreamError,
        format!("Noctalia replied: {}", text.trim_end()),
    );
    if text.starts_with("error:") {
        return Err(Unanswered::Refused(error));
    }
    Err(Unanswered::Lost(error))
}

async fn request(socket: &Path, command: Command) -> Result<Vec<u8>, String> {
    let stream = UnixStream::connect(socket)
        .await
        .map_err(|error| format!("connect: {error}"))?;
    exchange(stream, command).await
}

/// Writes the whole payload, shuts down the write half, and reads until EOF.
async fn exchange(
    mut stream: impl AsyncRead + AsyncWrite + Unpin,
    command: Command,
) -> Result<Vec<u8>, String> {
    stream
        .write_all(&command.payload())
        .await
        .map_err(|error| format!("write: {error}"))?;
    stream
        .shutdown()
        .await
        .map_err(|error| format!("shut down the write half: {error}"))?;
    let mut reply = Vec::new();
    (&mut stream)
        .take(MAX_REPLY + 1)
        .read_to_end(&mut reply)
        .await
        .map_err(|error| format!("read: {error}"))?;
    if u64::try_from(reply.len()).unwrap_or(u64::MAX) > MAX_REPLY {
        return Err(format!("the reply is longer than {MAX_REPLY} bytes"));
    }
    Ok(reply)
}

fn interpret(reply: &[u8]) -> Result<Map<String, Value>, ToolError> {
    let text = String::from_utf8_lossy(reply);
    if let Some(message) = text.strip_prefix("error:") {
        return Err(ToolError::new(
            ErrorName::UpstreamError,
            format!("Noctalia replied: error:{}", message.trim_end()),
        ));
    }
    if text.trim().is_empty() {
        return Err(unavailable(
            "Noctalia closed the connection without a reply",
        ));
    }
    if let Ok(Value::Object(status)) = serde_json::from_str(&text) {
        return Ok(status);
    }
    Err(unavailable(format!(
        "Noctalia's status reply isn't a JSON object: {}",
        text.trim_end()
    )))
}

fn unavailable(detail: impl Into<String>) -> ToolError {
    ToolError::new(ErrorName::NoctaliaUnavailable, detail)
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use tokio::io::duplex;

    use super::*;

    #[test]
    fn the_socket_follows_noctalias_naming() {
        let env = |display: &str| Env {
            runtime_dir: Some(PathBuf::from("/run/user/1000")),
            wayland_display: Some(OsString::from(display)),
            ..Env::default()
        };
        assert_eq!(
            socket(&env("wayland-1")),
            Some(PathBuf::from("/run/user/1000/noctalia-wayland-1.sock"))
        );
        assert_eq!(socket(&env("/run/user/1000/wayland-1")), None);
        assert_eq!(socket(&Env::default()), None);
    }

    #[tokio::test]
    async fn sends_the_fixed_status_payload_then_reads_to_eof() {
        let (client, mut noctalia) = duplex(4096);
        let fake = tokio::spawn(async move {
            let mut request = Vec::new();
            // Reading to EOF proves the client shut down its write half.
            noctalia.read_to_end(&mut request).await.unwrap();
            noctalia.write_all(b"{\"locked\":false}").await.unwrap();
            request
        });
        assert_eq!(
            exchange(client, Command::Status).await.unwrap(),
            b"{\"locked\":false}"
        );
        assert_eq!(fake.await.unwrap(), b"/\x1estatus");
    }

    #[test]
    fn every_payload_is_one_fixed_command_after_the_separator() {
        assert_eq!(
            Command::PanelOpen(Panel::ControlCenter).payload(),
            b"/\x1epanel-open control-center"
        );
        assert_eq!(
            Command::PanelClose(Panel::TrayDrawer).payload(),
            b"/\x1epanel-close tray-drawer"
        );
        let panels = [Panel::ControlCenter, Panel::Wallpaper, Panel::TrayDrawer];
        let commands = panels
            .into_iter()
            .flat_map(|panel| [Command::PanelOpen(panel), Command::PanelClose(panel)])
            .chain([Command::Status]);
        for command in commands {
            let payload = command.payload();
            let rest = payload.strip_prefix(b"/\x1e").unwrap();
            assert!(
                !rest
                    .iter()
                    .any(|byte| matches!(byte, b'\x1e' | b'\n' | b'\r')),
                "{command:?}"
            );
        }
    }

    #[test]
    fn a_panel_change_needs_noctalias_ok() {
        assert_eq!(acknowledged(b"ok\n"), Ok(()));
        // Noctalia names the panels it has when it doesn't know one.
        let Err(Unanswered::Refused(error)) =
            acknowledged(b"error: unknown panel \"x\" (available: a, b)\n")
        else {
            panic!("an error reply is a refusal");
        };
        assert_eq!(
            error.detail,
            "Noctalia replied: error: unknown panel \"x\" (available: a, b)"
        );
        // Something that is neither may have followed the change.
        for odd in [&b""[..], b"ok", b"{}"] {
            assert!(
                matches!(acknowledged(odd), Err(Unanswered::Lost(_))),
                "{odd:?}"
            );
        }
    }

    #[test]
    fn the_active_panel_is_an_id_or_null() {
        let status = |active: Value| {
            let mut status = Map::new();
            status.insert("activePanelId".to_owned(), active);
            status
        };
        assert_eq!(
            active_panel(&status(Value::from("wallpaper"))),
            Ok(Some("wallpaper".to_owned()))
        );
        assert_eq!(active_panel(&status(Value::Null)), Ok(None));
        for bad in [status(Value::Bool(false)), Map::new()] {
            assert_eq!(
                active_panel(&bad).unwrap_err().name,
                ErrorName::NoctaliaUnavailable
            );
        }
    }

    #[test]
    fn replies_are_a_status_object_an_error_or_unavailable() {
        let status = interpret(b"{\"barVisible\":true,\"locked\":false}").unwrap();
        assert_eq!(status.get("locked"), Some(&Value::Bool(false)));
        assert_eq!(
            interpret(b"error: unknown command\n").unwrap_err(),
            ToolError::new(
                ErrorName::UpstreamError,
                "Noctalia replied: error: unknown command"
            )
        );
        for bad in [&b""[..], b"[]", b"ok\n", b"{"] {
            assert_eq!(
                interpret(bad).unwrap_err().name,
                ErrorName::NoctaliaUnavailable
            );
        }
    }

    #[tokio::test]
    async fn a_missing_socket_is_unavailable() {
        let env = Env {
            runtime_dir: Some(std::env::temp_dir().join(format!(
                "niri-computer-use-no-noctalia-{}",
                std::process::id()
            ))),
            wayland_display: Some(OsString::from("wayland-1")),
            ..Env::default()
        };
        let error = status(&env).await.unwrap_err();
        assert_eq!(error.name, ErrorName::NoctaliaUnavailable);
        assert!(error.detail.contains("connect:"), "{error:?}");
    }
}
