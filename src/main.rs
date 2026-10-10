//! niri-computer-use: an MCP server that lets AI agents observe a niri desktop.

mod act;
mod audit;
mod cli;
mod clipboard;
mod control;
mod coords;
mod error;
mod image_header;
mod input;
mod niri;
mod noctalia;
mod observe;
mod policy;
mod refs;
mod runner;
mod settle;
mod status;
#[cfg(test)]
mod test_support;
mod tools;
mod wait;

use std::ffi::OsString;
use std::os::unix::fs::PermissionsExt as _;
use std::path::PathBuf;
use std::process::ExitCode;

use rmcp::ServiceExt as _;

use crate::control::runtime::RuntimeDir;

const USAGE: &str =
    "usage: niri-computer-use serve | status | stop | resume | recover | guard <server-pid>";

/// What the server reads from its environment, once at startup. An empty variable
/// counts as unset.
#[derive(Debug, Clone, Default)]
pub(crate) struct Env {
    /// niri's IPC socket, which also names the compositor instance.
    pub(crate) niri_socket: Option<PathBuf>,
    pub(crate) path: Option<OsString>,
    pub(crate) runtime_dir: Option<PathBuf>,
    pub(crate) wayland_display: Option<OsString>,
    /// `$XDG_STATE_HOME`, or `$HOME/.local/state`, for the audit log.
    pub(crate) state_dir: Option<PathBuf>,
    /// `$XDG_CONFIG_HOME`, or `$HOME/.config`, for the policy file.
    pub(crate) config_dir: Option<PathBuf>,
    /// Explicit experimental backend selection; absent means wtype.
    pub(crate) keyboard: Option<OsString>,
}

impl Env {
    fn read() -> Self {
        let var = |name| std::env::var_os(name).filter(|value| !value.is_empty());
        Self {
            niri_socket: var("NIRI_SOCKET").map(PathBuf::from),
            path: var("PATH"),
            runtime_dir: var("XDG_RUNTIME_DIR").map(PathBuf::from),
            wayland_display: var("WAYLAND_DISPLAY"),
            keyboard: var("NIRI_COMPUTER_USE_KEYBOARD"),
            state_dir: var("XDG_STATE_HOME")
                .map(PathBuf::from)
                .or_else(|| var("HOME").map(|home| PathBuf::from(home).join(".local/state"))),
            config_dir: var("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .or_else(|| var("HOME").map(|home| PathBuf::from(home).join(".config"))),
        }
    }

    /// The policy file, `<config dir>/niri-computer-use/policy.toml`, read and checked.
    pub(crate) fn policy(&self) -> policy::Loaded {
        let path = self
            .config_dir
            .as_ref()
            .map(|dir| dir.join("niri-computer-use").join("policy.toml"));
        let read = path
            .as_deref()
            .map(|path| (path, std::fs::read_to_string(path)));
        policy::Loaded::from_read(read)
    }

    /// The basename of `NIRI_SOCKET`, which names the compositor instance.
    pub(crate) fn instance(&self) -> Option<String> {
        self.niri_socket
            .as_deref()
            .and_then(std::path::Path::file_name)
            .map(|name| name.to_string_lossy().into_owned())
    }

    /// The Wayland display's socket: `WAYLAND_DISPLAY`, under `XDG_RUNTIME_DIR` unless it
    /// is an absolute path.
    pub(crate) fn wayland_socket(&self) -> Option<PathBuf> {
        let display = std::path::Path::new(self.wayland_display.as_ref()?);
        if display.is_absolute() {
            return Some(display.to_path_buf());
        }
        Some(self.runtime_dir.as_ref()?.join(display))
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
    /// Set the stop flag for this niri instance.
    Stop,
    /// Clear the stop flag.
    Resume,
    /// Clear the input-dirty marker after ending the input child and asking the human.
    Recover,
    /// The crash guardian `serve` starts: after the server's end, release what its marker
    /// names.
    Guard(u32),
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let env = Env::read();
    let result = match command(&args) {
        Some(Command::Serve) => serve(env).await,
        Some(Command::Status) => {
            let audit = audit::Audit::new(env.state_dir.clone());
            let policy = env.policy();
            let sources = status::Sources {
                event_stream: None,
                audit: &audit,
                noctalia_installed: env.finds("noctalia"),
                lease: control::desk::status_without_desk(&env),
                policy: &policy,
            };
            cli::print_json(&status::collect(&env, sources).await)
                .map_err(|error| format!("print status: {error}"))
        }
        Some(Command::Stop) => RuntimeDir::of(&env).and_then(|runtime| {
            runtime.stop().map_err(|error| {
                format!("set the stop flag in {}: {error}", runtime.path().display())
            })
        }),
        Some(Command::Resume) => RuntimeDir::of(&env).and_then(|runtime| runtime.resume()),
        Some(Command::Recover) => control::recover::run(&env).await,
        Some(Command::Guard(server)) => control::guard::run(&env, server).await,
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
        [only] if only == "stop" => Some(Command::Stop),
        [only] if only == "resume" => Some(Command::Resume),
        [only] if only == "recover" => Some(Command::Recover),
        [guard, server] if guard == "guard" => server.to_str()?.parse().ok().map(Command::Guard),
        _ => None,
    }
}

async fn serve(env: Env) -> Result<(), String> {
    // Lives until the server exits; see `control::guard`. `/proc/self/exe` still runs this
    // binary when its file has been replaced since.
    let _guardian = runner::watcher(
        "/proc/self/exe",
        &["guard".to_owned(), std::process::id().to_string()],
    )
    .map_err(|error| format!("start the crash guardian: {}", error.detail))?;
    let events = env
        .niri_socket
        .clone()
        .map(niri::events::EventStream::spawn);
    let audit = audit::Audit::new(env.state_dir.clone());
    let service = tools::Server::new(env, events, audit)
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
    fn the_wayland_socket_is_under_the_runtime_directory_unless_absolute() {
        let env = |display: &str, runtime: Option<&str>| Env {
            wayland_display: Some(display.into()),
            runtime_dir: runtime.map(PathBuf::from),
            ..Env::default()
        };
        assert_eq!(
            env("wayland-1", Some("/run/user/1000")).wayland_socket(),
            Some(PathBuf::from("/run/user/1000/wayland-1"))
        );
        assert_eq!(
            env("/tmp/w/wayland-9", None).wayland_socket(),
            Some(PathBuf::from("/tmp/w/wayland-9"))
        );
        assert_eq!(env("wayland-1", None).wayland_socket(), None);
        assert_eq!(Env::default().wayland_socket(), None);
    }

    #[test]
    fn takes_exactly_one_known_subcommand() {
        let args = |list: &[&str]| list.iter().map(OsString::from).collect::<Vec<_>>();
        assert_eq!(command(&args(&["serve"])), Some(Command::Serve));
        assert_eq!(command(&args(&["status"])), Some(Command::Status));
        assert_eq!(command(&args(&["stop"])), Some(Command::Stop));
        assert_eq!(command(&args(&["resume"])), Some(Command::Resume));
        assert_eq!(command(&args(&["recover"])), Some(Command::Recover));
        assert_eq!(command(&args(&["guard", "42"])), Some(Command::Guard(42)));
        for bad in [
            &[][..],
            &["stop", "now"],
            &["serve", "status"],
            &["--help"],
            &["guard"],
            &["guard", "-1"],
        ] {
            assert_eq!(command(&args(bad)), None, "{bad:?}");
        }
    }
}
