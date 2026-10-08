//! niri-desktop-mcp: an MCP server that lets AI agents observe a niri desktop.

mod cli;
mod error;
mod niri;
mod status;
mod tools;

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

use rmcp::ServiceExt as _;

const USAGE: &str = "usage: niri-desktop-mcp serve | status";

/// What the server reads from its environment, once at startup.
#[derive(Debug, Clone)]
pub(crate) struct Env {
    /// niri's IPC socket, which also names the compositor instance.
    pub(crate) niri_socket: Option<PathBuf>,
    pub(crate) path: Option<OsString>,
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
    let env = Env {
        niri_socket: niri_socket(std::env::var_os("NIRI_SOCKET")),
        path: std::env::var_os("PATH"),
    };
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

/// An empty `NIRI_SOCKET` means no niri, the same as an unset one.
fn niri_socket(value: Option<OsString>) -> Option<PathBuf> {
    value.filter(|value| !value.is_empty()).map(PathBuf::from)
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
    fn an_empty_niri_socket_is_no_socket() {
        assert_eq!(niri_socket(None), None);
        assert_eq!(niri_socket(Some(OsString::new())), None);
        assert_eq!(
            niri_socket(Some("/run/user/1000/niri.sock".into())),
            Some(PathBuf::from("/run/user/1000/niri.sock"))
        );
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
