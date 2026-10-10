//! niri-computer-use: an MCP server that lets AI agents observe a niri desktop.

mod a11y;
mod act;
mod audit;
mod bridge;
mod cli;
mod clipboard;
mod control;
mod coords;
mod discover;
mod elements;
mod engine;
mod error;
mod image_header;
mod input;
mod niri;
mod noctalia;
mod observe;
mod policy;
mod refs;
mod runner;
mod save;
mod session;
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

use crate::control::runtime::RuntimeDir;

const USAGE: &str = "usage: niri-computer-use serve | status | stop | resume | recover | engine | guard <server-pid> | paste-keeper";

/// What the server reads from its environment, once at startup. An empty variable
/// counts as unset.
#[derive(Debug, Clone, Default)]
pub(crate) struct Env {
    /// niri's IPC socket, which also names the compositor instance.
    pub(crate) niri_socket: niri::Socket,
    pub(crate) path: Option<OsString>,
    pub(crate) runtime_dir: Option<PathBuf>,
    pub(crate) wayland_display: Option<OsString>,
    /// The Wayland display's socket, checked against niri at each use.
    pub(crate) display: niri::Display,
    /// `$XDG_STATE_HOME`, or `$HOME/.local/state`, for the audit log.
    pub(crate) state_dir: Option<PathBuf>,
    /// The session bus, where the accessibility bus is looked up:
    /// `DBUS_SESSION_BUS_ADDRESS`, or else the user bus in the runtime directory.
    pub(crate) session_bus: Option<OsString>,
    /// Where the runtime directory, niri's socket and the display came from.
    pub(crate) discovery: discover::Sources,
}

impl Env {
    async fn read() -> Self {
        Self::from_vars(|name| std::env::var_os(name), &discover::Roots::host()).await
    }

    /// The environment `var` reads, with what it lacks of the session discovered under
    /// `roots`. An empty variable counts as unset.
    async fn from_vars(
        var: impl Fn(&str) -> Option<OsString> + Sync,
        roots: &discover::Roots<'_>,
    ) -> Self {
        let var = |name| var(name).filter(|value| !value.is_empty());
        let given = discover::Given {
            runtime_dir: var("XDG_RUNTIME_DIR").map(PathBuf::from),
            niri_socket: var("NIRI_SOCKET").map(PathBuf::from),
            wayland_display: var("WAYLAND_DISPLAY"),
        };
        let session = discover::session(given, roots).await;
        let mut env = Self {
            niri_socket: session
                .niri_socket
                .map_or_else(niri::Socket::unknown, niri::Socket::at),
            path: var("PATH"),
            wayland_display: session.wayland_display,
            display: niri::Display::default(),
            // Without the variable, D-Bus clients (libdbus, sd-bus, zbus) use the user bus
            // systemd starts at `$XDG_RUNTIME_DIR/bus`.
            session_bus: var("DBUS_SESSION_BUS_ADDRESS").or_else(|| {
                session.runtime_dir.as_ref().map(|dir| {
                    let mut address = OsString::from("unix:path=");
                    address.push(dir.join("bus"));
                    address
                })
            }),
            runtime_dir: session.runtime_dir,
            state_dir: var("XDG_STATE_HOME")
                .map(PathBuf::from)
                .or_else(|| var("HOME").map(|home| PathBuf::from(home).join(".local/state"))),
            discovery: session.sources,
        };
        env.display = niri::Display::new(env.wayland_socket());
        env
    }

    /// The basename of `NIRI_SOCKET`, which names the compositor instance.
    pub(crate) fn instance(&self) -> Option<String> {
        self.niri_socket
            .path()
            .ok()
            .and_then(std::path::Path::file_name)
            .map(|name| name.to_string_lossy().into_owned())
    }

    /// The Wayland display's socket: `WAYLAND_DISPLAY`, under `XDG_RUNTIME_DIR` unless it
    /// is an absolute path.
    fn wayland_socket(&self) -> Result<PathBuf, String> {
        let display = std::path::Path::new(self.wayland_display.as_ref().ok_or_else(|| {
            match &self.discovery.wayland_display {
                discover::Source::Missing(detail) => detail.clone(),
                discover::Source::Environment | discover::Source::Discovered => {
                    "WAYLAND_DISPLAY is not set".to_owned()
                }
            }
        })?);
        if display.is_absolute() {
            return Ok(display.to_path_buf());
        }
        let runtime = self.runtime_dir.as_ref().ok_or_else(|| {
            format!(
                "XDG_RUNTIME_DIR is not set, so WAYLAND_DISPLAY {} names no socket",
                display.display()
            )
        })?;
        Ok(runtime.join(display))
    }

    /// The session variables as found, for the server's children.
    fn session_vars(&self) -> Vec<(&'static str, OsString)> {
        let socket = self
            .niri_socket
            .path()
            .ok()
            .map(|path| path.as_os_str().to_owned());
        [
            (
                "XDG_RUNTIME_DIR",
                self.runtime_dir.clone().map(PathBuf::into_os_string),
            ),
            ("NIRI_SOCKET", socket),
            ("WAYLAND_DISPLAY", self.wayland_display.clone()),
        ]
        .into_iter()
        .filter_map(|(name, value)| Some((name, value?)))
        .collect()
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
    /// The shared engine a bridge starts: serve every client of this niri instance on its
    /// socket.
    Engine,
    /// The crash guardian `serve` starts: after the server's end, release what its marker
    /// names.
    Guard(u32),
    /// The clipboard keeper `paste` starts: hold the pasted text, then restore the
    /// clipboard.
    PasteKeeper,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let env = Env::read().await;
    let given = session::Given::read(|name| std::env::var_os(name));
    runner::pass_on(env.session_vars());
    let result = match command(&args) {
        Some(Command::Serve) => serve(env, given).await,
        Some(Command::Status) => {
            let audit = audit::Audit::new(env.state_dir.clone());
            let settings = session::Settings::new(given);
            let accessibility = a11y::detect(env.session_bus.as_deref()).await;
            let sources = status::Sources {
                engine: None,
                event_stream: None,
                audit: &audit,
                noctalia_installed: env.finds("noctalia"),
                lease: control::desk::status_without_desk(&env),
                policy: &settings.policy,
                unrestricted: &settings.unrestricted,
                accessibility: &accessibility,
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
        Some(Command::Engine) => engine::host::run(env).await,
        Some(Command::Guard(server)) => control::guard::run(&env, server).await,
        Some(Command::PasteKeeper) => input::keeper::run(&env).await,
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
        [only] if only == "paste-keeper" => Some(Command::PasteKeeper),
        [only] if only == "engine" => Some(Command::Engine),
        [guard, server] if guard == "guard" => server.to_str()?.parse().ok().map(Command::Guard),
        _ => None,
    }
}

/// Serves the client on stdin and stdout: through the shared engine in shared mode, unless
/// it can't be reached or refuses this client, and standalone otherwise. Decided before
/// the client's first byte is read.
async fn serve(env: Env, given: session::Given) -> Result<(), String> {
    let settings = session::Settings::new(given.clone());
    let shared = policy::shared(
        &settings.policy,
        std::env::var_os("NIRI_COMPUTER_USE_SHARED").as_deref(),
    );
    let reached = match shared {
        Ok(true) => reach_engine(&env, given.clone()).await.map(Some),
        Ok(false) => Ok(None),
        Err(note) => Err(note),
    };
    let fallback = match reached {
        Ok(Some((target, link))) => return bridge::run(target, link).await,
        Ok(None) => None,
        Err(reason) => {
            cli::print_error(&format!("serving this client standalone: {reason}"));
            Some(reason)
        }
    };
    standalone(env, settings, fallback).await
}

async fn reach_engine(
    env: &Env,
    given: session::Given,
) -> Result<(bridge::Target, bridge::Link), String> {
    let target = bridge::Target::new(env, given)?;
    let link = bridge::connect(&target).await?;
    Ok((target, link))
}

/// Serves the one client on stdin and stdout. `fallback` says why it isn't served by the
/// shared engine it asked for.
async fn standalone(
    env: Env,
    settings: session::Settings,
    fallback: Option<String>,
) -> Result<(), String> {
    let engine = engine::Engine::start(env, status::Mode::Standalone, fallback).await?;
    let engine = std::sync::Arc::new(engine);
    let session = engine.open_session(std::process::id(), settings);
    let served =
        engine::host::serve_session(&engine, session, tokio::io::stdin(), tokio::io::stdout())
            .await;
    // A dropped action's input cleanup may still run; cut short, it leaves the marker.
    control::cleanup::settled().await;
    served
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
            Ok(PathBuf::from("/run/user/1000/wayland-1"))
        );
        assert_eq!(
            env("/tmp/w/wayland-9", None).wayland_socket(),
            Ok(PathBuf::from("/tmp/w/wayland-9"))
        );
        assert_eq!(
            env("wayland-1", None).wayland_socket(),
            Err("XDG_RUNTIME_DIR is not set, so WAYLAND_DISPLAY wayland-1 names no socket".into())
        );
        assert_eq!(
            Env::default().wayland_socket(),
            Err("WAYLAND_DISPLAY is not set".into())
        );
    }

    #[tokio::test]
    async fn the_session_bus_is_in_the_discovered_runtime_directory() {
        let root = test_support::fresh_dir("env-bus");
        let euid = rustix::process::geteuid().as_raw();
        let runtime = root.join(euid.to_string());
        std::fs::create_dir(&runtime).unwrap();
        std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o700)).unwrap();
        let roots = discover::Roots {
            run_user: &root,
            proc: &root.join("proc"),
            euid,
        };
        let env = Env::from_vars(|_| None, &roots).await;
        let mut bus = OsString::from("unix:path=");
        bus.push(runtime.join("bus"));
        assert_eq!(env.session_bus, Some(bus));
        assert_eq!(env.runtime_dir, Some(runtime));
        let given = Env::from_vars(
            |name| (name == "DBUS_SESSION_BUS_ADDRESS").then(|| "unix:path=/b".into()),
            &roots,
        )
        .await;
        assert_eq!(given.session_bus, Some("unix:path=/b".into()));
        std::fs::remove_dir_all(root).unwrap();
    }

    /// A client that forwards niri's socket but not the runtime directory, for a niri
    /// whose runtime directory isn't the default one, such as a nested niri.
    #[tokio::test]
    async fn servers_for_one_niri_share_its_lease_and_stop_flag_without_the_runtime_variable() {
        use crate::control::lease::Lease;
        use crate::control::runtime::RuntimeDir;

        let root = test_support::fresh_dir("env-nested");
        let euid = rustix::process::geteuid().as_raw();
        let default = root.join("run").join(euid.to_string());
        let nested = root.join("nested");
        for dir in [&default, &nested] {
            std::fs::create_dir_all(dir).unwrap();
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let roots = discover::Roots {
            run_user: &root.join("run"),
            proc: &root.join("proc"),
            euid,
        };
        let socket = nested.join("niri.wayland-2.42.sock");
        let full = Env::from_vars(
            |name| match name {
                "XDG_RUNTIME_DIR" => Some(nested.clone().into()),
                "NIRI_SOCKET" => Some(socket.clone().into()),
                _ => None,
            },
            &roots,
        )
        .await;
        let partial = Env::from_vars(
            |name| (name == "NIRI_SOCKET").then(|| socket.clone().into()),
            &roots,
        )
        .await;
        let (full, partial) = (
            RuntimeDir::of(&full).unwrap(),
            RuntimeDir::of(&partial).unwrap(),
        );
        let _held = Lease::acquire(&full, "full").unwrap();
        assert!(Lease::acquire(&partial, "partial").is_err());
        full.stop().unwrap();
        assert!(partial.stopped().unwrap());
        std::fs::remove_dir_all(root).unwrap();
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
        assert_eq!(
            command(&args(&["paste-keeper"])),
            Some(Command::PasteKeeper)
        );
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
