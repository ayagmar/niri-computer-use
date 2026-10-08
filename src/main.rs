//! niri-desktop-mcp: an MCP server that lets AI agents observe a niri desktop.

mod cli;
mod clipboard;
mod control;
mod error;
mod image_header;
mod niri;
mod noctalia;
mod observe;
mod runner;
mod status;
#[cfg(test)]
mod test_support;
mod tools;

use std::ffi::OsString;
use std::os::unix::fs::PermissionsExt as _;
use std::path::PathBuf;
use std::process::ExitCode;

use rmcp::ServiceExt as _;

const USAGE: &str = "usage: niri-desktop-mcp serve | status";

/// What the server reads from its environment, once at startup. An empty variable
/// counts as unset.
#[derive(Debug, Clone, Default)]
pub(crate) struct Env {
    /// niri's IPC socket, which also names the compositor instance.
    pub(crate) niri_socket: Option<PathBuf>,
    pub(crate) path: Option<OsString>,
    pub(crate) runtime_dir: Option<PathBuf>,
    pub(crate) wayland_display: Option<OsString>,
    /// The logind session, for the lock state.
    pub(crate) session_id: Option<String>,
}

impl Env {
    fn read() -> Self {
        let var = |name| std::env::var_os(name).filter(|value| !value.is_empty());
        Self {
            niri_socket: var("NIRI_SOCKET").map(PathBuf::from),
            path: var("PATH"),
            runtime_dir: var("XDG_RUNTIME_DIR").map(PathBuf::from),
            wayland_display: var("WAYLAND_DISPLAY"),
            session_id: var("XDG_SESSION_ID").map(|id| id.to_string_lossy().into_owned()),
        }
    }

    /// The basename of `NIRI_SOCKET`, which names the compositor instance.
    pub(crate) fn instance(&self) -> Option<String> {
        self.niri_socket
            .as_deref()
            .and_then(std::path::Path::file_name)
            .map(|name| name.to_string_lossy().into_owned())
    }

    /// Whether `PATH` has an executable file called `program`.
    pub(crate) fn finds(&self, program: &str) -> bool {
        let dirs = self.path.iter().flat_map(std::env::split_paths);
        dirs.map(|dir| dir.join(program)).any(|file| {
            file.metadata()
                .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        })
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Command {
    /// Serve MCP over stdin and stdout.
    Serve,
    /// Print the readiness report.
    Status,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let env = Env::read();
    let result = match command(&args) {
        Some(Command::Serve) => serve(env).await,
        Some(Command::Status) => cli::print_json(&status::collect(&env, None).await)
            .map_err(|error| format!("print status: {error}")),
        None => Err(USAGE.to_owned()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            cli::print_error(&message);
            ExitCode::FAILURE
        }
    }
}

fn command(args: &[OsString]) -> Option<Command> {
    match args {
        [only] if only == "serve" => Some(Command::Serve),
        [only] if only == "status" => Some(Command::Status),
        _ => None,
    }
}

async fn serve(env: Env) -> Result<(), String> {
    let events = env
        .niri_socket
        .clone()
        .map(niri::events::EventStream::spawn);
    let service = tools::Server::new(env, events)
        .serve(rmcp::transport::stdio())
        .await
        .map_err(|error| format!("start MCP session: {error}"))?;
    service
        .waiting()
        .await
        .map(drop)
        .map_err(|error| format!("MCP session: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_only_executable_files_on_path() {
        use std::fs;
        let dir = test_support::fresh_dir("path");
        fs::create_dir_all(dir.join("dir-named-grim/grim")).unwrap();
        fs::write(dir.join("wtype"), "").unwrap();
        fs::set_permissions(dir.join("wtype"), fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(dir.join("loginctl"), "").unwrap();
        fs::set_permissions(dir.join("loginctl"), fs::Permissions::from_mode(0o644)).unwrap();
        let env = Env {
            path: Some(std::env::join_paths([dir.join("missing"), dir.clone()]).unwrap()),
            ..Env::default()
        };
        assert!(env.finds("wtype"));
        assert!(!env.finds("loginctl"));
        assert!(!env.finds("grim"));
        assert!(!env.finds("wl-copy"));
        assert!(!Env::default().finds("wtype"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn takes_exactly_one_known_subcommand() {
        let args = |list: &[&str]| list.iter().map(OsString::from).collect::<Vec<_>>();
        assert_eq!(command(&args(&["serve"])), Some(Command::Serve));
        assert_eq!(command(&args(&["status"])), Some(Command::Status));
        for bad in [&[][..], &["stop"], &["serve", "status"], &["--help"]] {
            assert_eq!(command(&args(bad)), None, "{bad:?}");
        }
    }
}
