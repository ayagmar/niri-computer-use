//! niri-computer-use: an MCP server that lets AI agents observe a niri desktop.

mod a11y;
mod act;
mod apps;
mod audit;
mod bridge;
mod changes;
mod cli;
mod clipboard;
mod confirm;
mod control;
mod coords;
mod desks;
mod discover;
mod elements;
mod engine;
mod error;
mod image_header;
mod input;
mod live;
mod niri;
mod noctalia;
mod notice;
mod observe;
mod overlay_area;
mod policy;
mod ppm;
mod refs;
mod runner;
mod save;
mod session;
mod settle;
mod status;
mod stop_key;
#[cfg(test)]
mod test_support;
mod tools;
mod wait;
mod watch;

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

use crate::control::runtime::RuntimeDir;

const USAGE: &str = "usage: niri-computer-use serve | status [--check] | stop | resume | recover | watch, each but stop and resume with an optional --desk <name> | desk start [--windowed] [--size WIDTHxHEIGHT] <name> | desk stop <name> | desk list | engine | guard <server-pid> | paste-keeper | --version";

/// What the server reads from its environment, once at startup. An empty variable
/// counts as unset.
#[derive(Debug, Clone, Default)]
pub(crate) struct Env {
    /// niri's IPC socket: resolved, when `instance` is.
    pub(crate) niri_socket: niri::Socket,
    /// The compositor instance, resolved once at startup, which names the runtime
    /// directory.
    pub(crate) instance: control::runtime::Instance,
    /// `loginctl` and `noctalia`, found by `read` and by no other constructor, so a test's
    /// environment never searches the host's directories.
    pub(crate) desktop: runner::Desktop,
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
    /// The host session, or the desk this server works on.
    pub(crate) place: desks::join::Place,
    /// The niri of the owner's session, whose lock state and stop key count: `niri_socket`
    /// on the host, the host's niri for a desk.
    pub(crate) host_niri: niri::Socket,
}

impl Env {
    async fn read(probe: discover::Probe) -> Self {
        let env = Self::from_vars(
            |name| std::env::var_os(name),
            &discover::Roots::host(),
            probe,
        )
        .await;
        let search = std::env::var_os("NIRI_COMPUTER_USE_DESKTOP_PATH")
            .filter(|search| !search.is_empty())
            .unwrap_or_else(|| runner::DESKTOP_PATH.into());
        Self {
            desktop: runner::Desktop::find(&search),
            ..env
        }
    }

    /// The environment `var` reads, with what it lacks of the session discovered under
    /// `roots` the way `probe` says. An empty variable counts as unset.
    async fn from_vars(
        var: impl Fn(&str) -> Option<OsString> + Sync,
        roots: &discover::Roots<'_>,
        probe: discover::Probe,
    ) -> Self {
        let var = |name| var(name).filter(|value| !value.is_empty());
        let given = discover::Given {
            runtime_dir: var("XDG_RUNTIME_DIR").map(PathBuf::from),
            niri_socket: var("NIRI_SOCKET").map(PathBuf::from),
            wayland_display: var("WAYLAND_DISPLAY"),
        };
        let session = discover::session(given, roots, probe).await;
        // Connections go to the resolved socket, so a symlink retargeted later doesn't move
        // them to another niri.
        let niri_socket = match (session.instance.socket(), session.niri_socket) {
            (Ok(resolved), _) => niri::Socket::at(resolved.to_path_buf()),
            (Err(_), found) => found.map_or_else(niri::Socket::unknown, niri::Socket::at),
        };
        let mut env = Self {
            host_niri: niri_socket.clone(),
            niri_socket,
            instance: session.instance,
            desktop: runner::Desktop::default(),
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
            place: desks::join::Place::Host,
        };
        env.display = niri::Display::new(env.wayland_socket());
        env
    }

    /// The environment of the desk `name` of the host session `self` describes, once it is
    /// ready: the desk's runtime directory, niri, display and session bus in place of the
    /// host's, everything else the host's. Fails, having started nothing, when the desk
    /// isn't ready; never falls back to the host.
    async fn join(
        self,
        name: &desks::name::DeskName,
        probe: discover::Probe,
    ) -> Result<Self, String> {
        self.join_with(
            name,
            probe,
            |var| std::env::var_os(var),
            &discover::Roots::host(),
        )
        .await
    }

    /// `join`, with the server's own variables from `var` and the session discovered under
    /// `roots`.
    async fn join_with(
        self,
        name: &desks::name::DeskName,
        probe: discover::Probe,
        var: impl Fn(&str) -> Option<OsString> + Sync,
        roots: &discover::Roots<'_>,
    ) -> Result<Self, String> {
        let runtime = self
            .runtime_dir
            .as_deref()
            .ok_or_else(|| format!("XDG_RUNTIME_DIR isn't known, so desk {name} can't be found"))?;
        let joined = desks::join::join(runtime, name, desks::cli::alive)?;
        let overlay = joined.overlay();
        let env = Self::from_vars(
            |key| overlay.get(key).cloned().or_else(|| var(key)),
            roots,
            probe,
        )
        .await;
        let host_niri = joined.host.niri_socket.clone().map_or_else(
            || niri::Socket::unknown("the desk's record of its host names no niri socket"),
            niri::Socket::at,
        );
        Ok(Self {
            // A desk has no shell of its own; the host's Noctalia is read for the lock only.
            desktop: runner::Desktop {
                noctalia: None,
                ..self.desktop
            },
            place: desks::join::Place::Desk(Box::new(joined)),
            host_niri,
            ..env
        })
    }

    /// The desk this server works on, if any.
    pub(crate) fn desk(&self) -> Option<&desks::join::Joined> {
        match &self.place {
            desks::join::Place::Host => None,
            desks::join::Place::Desk(joined) => Some(joined),
        }
    }

    /// The file name of niri's socket, resolved when it could be, which names the compositor
    /// instance.
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
        if let Some(desk) = self.desk() {
            return desk.children();
        }
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
}

#[derive(Debug, PartialEq, Eq)]
enum Command {
    /// Serve MCP over stdin and stdout.
    Serve,
    /// Print the readiness report, as JSON or, with `--check`, as one line per
    /// prerequisite.
    Status { check: bool },
    /// Set the stop flag for this niri instance.
    Stop,
    /// Clear the stop flag.
    Resume,
    /// Clear the input-dirty marker after ending the input child and asking the human.
    Recover,
    /// The shared engine a bridge starts: serve every client of this niri instance on its
    /// socket.
    Engine(engine::host::Start),
    /// The crash guardian `serve` starts: after the server's end, release what its marker
    /// names.
    Guard(u32),
    /// The clipboard keeper `paste` starts: hold the pasted text, then restore the
    /// clipboard.
    PasteKeeper,
    /// Print what agents are doing on this niri instance as JSON lines, for a shell.
    Watch,
    /// A background desk's lifecycle: `desk start`, `stop` and `list`, and the
    /// supervisor's own subcommands.
    Desk(desks::cli::Command),
    /// Print the version.
    Version,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let (command, desk) = match command(&args) {
        Ok(command) => command,
        Err(message) => {
            cli::print_error(&message);
            return ExitCode::FAILURE;
        }
    };
    if command == Some(Command::Version) {
        cli::say(concat!("niri-computer-use ", env!("CARGO_PKG_VERSION")));
        return ExitCode::SUCCESS;
    }
    if let Some(Command::Desk(subcommand)) = &command
        && let Some(code) = desk_process(subcommand).await
    {
        return code;
    }
    // The flag commands must work while niri hangs, and pick only where a flag goes.
    let probe = match command {
        Some(Command::Stop | Command::Resume | Command::Watch) => discover::Probe::Offline,
        _ => discover::Probe::Connect,
    };
    let mut env = match desk {
        None => Env::read(probe).await,
        // The host is read without connecting to anything: only its runtime directory and
        // what the desk's server keeps of it are used.
        Some(name) => match Env::read(discover::Probe::Offline)
            .await
            .join(&name, probe)
            .await
        {
            Ok(env) => env,
            Err(message) => {
                cli::print_error(&message);
                return ExitCode::FAILURE;
            }
        },
    };
    if let Some(Command::Engine(start)) = &command
        && start.runtime_dir_discovered
    {
        env.discovery.runtime_dir = discover::Source::Discovered;
    }
    let given = session::Given::read(|name| std::env::var_os(name));
    runner::pass_on(env.session_vars());
    if let Some(Command::Status { check }) = command {
        return status(&env, given, check).await;
    }
    let result = match command {
        Some(Command::Serve) => serve(env, given).await,
        Some(Command::Stop) => stop(&env),
        Some(Command::Resume) => resume(&env),
        Some(Command::Recover) => control::recover::run(&env).await,
        Some(Command::Watch) => watch::run(&env).await,
        Some(Command::Engine(start)) => engine::host::run(env, start.unscoped).await,
        Some(Command::Guard(server)) => control::guard::run(&env, server).await,
        Some(Command::PasteKeeper) => input::keeper::run(&env).await,
        Some(Command::Desk(subcommand)) => desks::cli::run(&env, subcommand).await,
        Some(Command::Version | Command::Status { .. }) | None => Err(USAGE.to_owned()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            cli::print_error(&message);
            ExitCode::FAILURE
        }
    }
}

/// `stop`: sets the stop flag of this niri instance and the host-wide one every desk's
/// server checks, each even when the other can't be set.
fn stop(env: &Env) -> Result<(), String> {
    let desks = desks::cli::desks(env).and_then(|desks| {
        desks.stop().map_err(|error| {
            format!(
                "set the desks' stop flag {}: {error}",
                desks.stop_flag().display()
            )
        })
    });
    let host = RuntimeDir::of(env).and_then(|runtime| {
        runtime
            .stop()
            .map_err(|error| format!("set the stop flag in {}: {error}", runtime.path().display()))
    });
    match (host, desks) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Err(host), Err(desks)) => Err(format!("{host}; {desks}")),
    }
}

/// `resume`: clears this niri instance's stop flag, which it refuses while its input-dirty
/// marker is set, and only then the host-wide one. No desk's input-dirty marker is touched.
fn resume(env: &Env) -> Result<(), String> {
    RuntimeDir::of(env)?.resume()?;
    desks::cli::desks(env)?.resume()
}

/// The `status` subcommand: the report as JSON, or with `check` as text that fails while
/// anything blocks the server.
async fn status(env: &Env, given: session::Given, check: bool) -> ExitCode {
    let audit = audit::Audit::new(env.state_dir.clone());
    let settings = session::Settings::new(given);
    let accessibility = a11y::detect(env.session_bus.as_deref()).await;
    let sources = status::Sources {
        engine: None,
        event_stream: None,
        config_failed: niri::config_failed(&env.host_niri).await.ok(),
        stop_key: Some(&stop_key::Reader::default()),
        lease_notice_error: None,
        audit: &audit,
        lease: control::desk::status_without_desk(env),
        policy: &settings.policy,
        unrestricted: &settings.unrestricted,
        confirm_lease: &settings.confirm_lease,
        tools: &settings.tools,
        shared: &settings.shared,
        place: settings.place(),
        apps: &settings.apps,
        accessibility: &accessibility,
        accessibility_switch: None,
    };
    let report = status::collect(env, sources).await;
    if check {
        let verdict = status::check::verdict(&report, &stop_key::own_program());
        cli::say(&verdict.text);
        return if verdict.blockers == 0 {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        };
    }
    match cli::print_json(&report) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            cli::print_error(&format!("print status: {error}"));
            ExitCode::FAILURE
        }
    }
}

/// A desk process's own first steps, which must not discover the session or connect to
/// niri: `desk exec` becomes the desk's program, and `desk hold` waits for cage to end.
async fn desk_process(command: &desks::cli::Command) -> Option<ExitCode> {
    match command {
        desks::cli::Command::Exec {
            parent,
            death,
            program,
            args,
        } => {
            cli::print_error(&runner::desk::exec_dying_with(
                *parent, *death, program, args,
            ));
            Some(ExitCode::FAILURE)
        }
        desks::cli::Command::Hold => match desks::hold::run(|name| std::env::var_os(name)).await {
            Ok(()) => Some(ExitCode::SUCCESS),
            Err(error) => {
                cli::print_error(&error);
                Some(ExitCode::FAILURE)
            }
        },
        desks::cli::Command::Guard {
            supervisor,
            name,
            generation,
        } => match desks::guard::run(*supervisor, name, generation).await {
            Ok(()) => Some(ExitCode::SUCCESS),
            Err(error) => {
                cli::print_error(&error);
                Some(ExitCode::FAILURE)
            }
        },
        desks::cli::Command::Start { .. }
        | desks::cli::Command::Stop(_)
        | desks::cli::Command::List
        | desks::cli::Command::Supervise { .. } => None,
    }
}

/// The subcommand `args` name, `None` when they name none, and the desk a trailing
/// `--desk <name>` names; an error says what is wrong with a desk's name or a `desk`
/// subcommand's arguments.
fn command(args: &[OsString]) -> Result<(Option<Command>, Option<desks::name::DeskName>), String> {
    if let [first, rest @ ..] = args
        && first == "desk"
    {
        return desks::cli::parse(rest).map(|subcommand| (Some(Command::Desk(subcommand)), None));
    }
    let [rest @ .., flag, name] = args else {
        return Ok((known(args), None));
    };
    if flag != DESK {
        return Ok((known(args), None));
    }
    let name = desks::name::DeskName::parse(&name.to_string_lossy())?;
    match known(rest) {
        Some(
            command @ (Command::Serve
            | Command::Status { .. }
            | Command::Watch
            | Command::Recover
            | Command::Engine(_)),
        ) => Ok((Some(command), Some(name))),
        _ => Err(format!(
            "{DESK} goes after serve, status, watch or recover; `stop` and `resume` act on the host and every desk at once"
        )),
    }
}

/// The flag that names the desk a subcommand works on.
pub(crate) const DESK: &str = "--desk";

fn known(args: &[OsString]) -> Option<Command> {
    match args {
        [only] => single(only.to_str()?),
        [status, flag] if status == "status" && flag == "--check" => {
            Some(Command::Status { check: true })
        }
        [engine, flag] if engine == "engine" && flag == engine::host::RUNTIME_DIR_DISCOVERED => {
            Some(Command::Engine(engine::host::Start {
                runtime_dir_discovered: true,
                unscoped: None,
            }))
        }
        [engine, flag, reason] if engine == "engine" && flag == engine::host::UNSCOPED => {
            Some(Command::Engine(engine::host::Start {
                unscoped: Some(reason.to_str()?.to_owned()),
                runtime_dir_discovered: false,
            }))
        }
        [guard, server] if guard == "guard" => server.to_str()?.parse().ok().map(Command::Guard),
        _ => None,
    }
}

/// A subcommand given alone.
fn single(arg: &str) -> Option<Command> {
    Some(match arg {
        "serve" => Command::Serve,
        "status" => Command::Status { check: false },
        "stop" => Command::Stop,
        "resume" => Command::Resume,
        "recover" => Command::Recover,
        "watch" => Command::Watch,
        "paste-keeper" => Command::PasteKeeper,
        "engine" => Command::Engine(engine::host::Start::default()),
        "--version" => Command::Version,
        _ => return None,
    })
}

/// Serves the client on stdin and stdout: through the shared engine in shared mode, unless
/// it can't be reached or refuses this client, and standalone otherwise. A client that
/// requires the shared engine is never served standalone: without it, every call fails with
/// `engine_unavailable` and tries the engine again, so the client is served once the cause
/// is gone. Decided before the client's first byte is read.
async fn serve(env: Env, given: session::Given) -> Result<(), String> {
    let settings = session::Settings::new(given.clone());
    let sharing = settings.shared.setting();
    let reached = match (&settings.shared.env, sharing) {
        (Err(note), _) => Err((None, note.clone())),
        (Ok(_), policy::Sharing::Off) => return standalone(env, settings, None).await,
        (Ok(_), policy::Sharing::On | policy::Sharing::Required) => {
            // The engine reports the setting decided here, from the whole policy file, which
            // the hello may not carry.
            let given = session::Given {
                shared: Some(sharing.variable().into()),
                ..given
            };
            reach_engine(&env, given).await
        }
    };
    let tools = tools::Server::listed(&settings);
    let log = bridge::log::Log::new(&env, &tools);
    let (target, reason) = match reached {
        Ok((target, link)) => return bridge::run(target, Some(link), None, log).await,
        Err(unreached) => unreached,
    };
    if sharing == policy::Sharing::Required {
        let Some(target) = target else {
            cli::print_error(&format!(
                "the shared engine is required, so every tool fails with engine_unavailable: {reason}"
            ));
            return bridge::unavailable::Unavailable::new(tools, &reason, log)
                .run()
                .await;
        };
        cli::print_error(&format!(
            "the shared engine is required, so every call fails with engine_unavailable until one is reached: {reason}"
        ));
        let before = bridge::before::Before::new(tools);
        return bridge::run(*target, None, Some(before), log).await;
    }
    cli::print_error(&format!("serving this client standalone: {reason}"));
    standalone(env, settings, Some(reason)).await
}

/// The engine's target and a link to it, or why there is none, with the target when trying
/// again later may reach one.
async fn reach_engine(
    env: &Env,
    given: session::Given,
) -> Result<(bridge::Target, bridge::Link), (Option<Box<bridge::Target>>, String)> {
    let target = bridge::Target::new(env, given).map_err(|reason| (None, reason))?;
    match bridge::connect(&target).await {
        Ok(link) => Ok((target, link)),
        Err(bridge::Unreached::Never(reason)) => Err((None, reason)),
        Err(unreached) => Err((Some(Box::new(target)), unreached.detail())),
    }
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
    engine::host::serve_alone(&engine, session, tokio::io::stdin(), tokio::io::stdout()).await
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;

    use super::*;

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
        let env = Env::from_vars(|_| None, &roots, discover::Probe::Connect).await;
        let mut bus = OsString::from("unix:path=");
        bus.push(runtime.join("bus"));
        assert_eq!(env.session_bus, Some(bus));
        assert_eq!(env.runtime_dir, Some(runtime));
        let given = Env::from_vars(
            |name| (name == "DBUS_SESSION_BUS_ADDRESS").then(|| "unix:path=/b".into()),
            &roots,
            discover::Probe::Connect,
        )
        .await;
        assert_eq!(given.session_bus, Some("unix:path=/b".into()));
        std::fs::remove_dir_all(root).unwrap();
    }

    /// A client that forwards niri's socket but not the runtime directory, for a niri
    /// whose runtime directory isn't the default one, such as a nested niri, and one that
    /// reaches both through another root that links to them.
    #[tokio::test]
    async fn servers_for_one_niri_share_its_lease_and_stop_flag_however_they_reach_it() {
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
        let _niri = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        let alias = root.join("alias");
        std::os::unix::fs::symlink(&nested, &alias).unwrap();
        let full = Env::from_vars(
            |name| match name {
                "XDG_RUNTIME_DIR" => Some(nested.clone().into()),
                "NIRI_SOCKET" => Some(socket.clone().into()),
                _ => None,
            },
            &roots,
            discover::Probe::Connect,
        )
        .await;
        let partial = Env::from_vars(
            |name| (name == "NIRI_SOCKET").then(|| socket.clone().into()),
            &roots,
            discover::Probe::Connect,
        )
        .await;
        let aliased = Env::from_vars(
            |name| match name {
                "XDG_RUNTIME_DIR" => Some(alias.clone().into()),
                "NIRI_SOCKET" => Some(alias.join("niri.wayland-2.42.sock").into()),
                _ => None,
            },
            &roots,
            discover::Probe::Connect,
        )
        .await;
        let (full, partial, aliased) = (
            RuntimeDir::of(&full).unwrap(),
            RuntimeDir::of(&partial).unwrap(),
            RuntimeDir::of(&aliased).unwrap(),
        );
        let _held = Lease::acquire(&full, "full").unwrap();
        assert!(Lease::acquire(&partial, "partial").is_err());
        assert!(Lease::acquire(&aliased, "aliased").is_err());
        full.stop().unwrap();
        assert!(partial.stopped().unwrap());
        assert!(aliased.stopped().unwrap());
        std::fs::remove_dir_all(root).unwrap();
    }

    /// Catches a `resume` that clears a desk's input-dirty marker, which only `recover
    /// --desk` may once the human has checked that desk's input, one that clears the
    /// host-wide flag while the host's own input may be stuck, and a `stop` that leaves the
    /// desks running.
    #[test]
    fn stop_reaches_every_desk_and_resume_leaves_each_desks_marker() {
        use crate::control::runtime::RuntimeDir;
        let dir = test_support::short_dir("stop-all");
        let host = test_support::niri_env(&dir);
        let desk = test_support::desk_env(
            &dir,
            dir.join("niri.test.sock"),
            desks::record::LockSources::default(),
        );
        let (host_runtime, desk_runtime) = (
            RuntimeDir::of(&host).unwrap(),
            RuntimeDir::of(&desk).unwrap(),
        );
        desk_runtime.create().unwrap();
        std::fs::write(desk_runtime.path().join("input-dirty"), "").unwrap();
        stop(&host).unwrap();
        assert!(host_runtime.stopped().unwrap());
        assert!(desk_runtime.stopped().unwrap());
        resume(&host).unwrap();
        assert!(!host_runtime.stopped().unwrap());
        assert!(!desk_runtime.stopped().unwrap());
        assert!(desk_runtime.input_dirty().unwrap());
        stop(&host).unwrap();
        std::fs::write(host_runtime.path().join("input-dirty"), "").unwrap();
        assert!(resume(&host).is_err());
        assert!(desk_runtime.stopped().unwrap());
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A desk `web` of the host session in `dir`, recorded ready, its supervisor this test,
    /// with its sockets, and the host's niri at `niri.wayland-0.3.sock`.
    fn ready_desk(dir: &std::path::Path) -> Vec<std::os::unix::net::UnixListener> {
        use desks::record::{self, Endpoints, Host, Phase, Record, Supervisor};
        let name = desks::name::DeskName::parse("web").unwrap();
        let layout = desks::layout::Desks::of(dir).desk(&name).unwrap();
        std::fs::create_dir_all(layout.root()).unwrap();
        let niri = layout.root().join("niri.wayland-1.7.sock");
        let sockets = [niri.clone(), layout.root().join("wayland-1")]
            .map(|socket| std::os::unix::net::UnixListener::bind(socket).unwrap());
        let me = std::process::id();
        let start_time = control::procs::stat(std::path::Path::new("/proc"), me)
            .unwrap()
            .start_time;
        let phase = Phase::Ready {
            endpoints: Endpoints {
                niri_socket: niri,
                wayland_display: "wayland-1".to_owned(),
                session_bus: "unix:path=/desk/bus".to_owned(),
                accessibility_bus: "unix:path=/desk/at-spi".to_owned(),
            },
            round_trips: desks::check::RoundTrips::of(&[std::time::Duration::from_micros(500)])
                .unwrap(),
        };
        let desk = Record {
            name,
            mode: record::Mode::Headless,
            size: None,
            supervisor: Supervisor {
                pid: me,
                start_time,
            },
            generation: desks::scope::Generation::parse("1").unwrap(),
            cgroup: "/app.slice/ncu-desk-web-1.scope".to_owned(),
            started: String::new(),
            phase,
        };
        record::write(&layout.record(), &desk).unwrap();
        let host = Host {
            runtime_dir: dir.to_path_buf(),
            niri_socket: Some(dir.join("niri.wayland-0.3.sock")),
            wayland_display: Some("wayland-0".to_owned()),
            session_bus: Some("unix:path=/host/bus".to_owned()),
            lock: record::LockSources {
                logind_session: Some("4".to_owned()),
                noctalia: Some(dir.join("noctalia-wayland-0.sock")),
            },
        };
        record::write(&layout.host(), &host).unwrap();
        sockets.into()
    }

    /// Catches a desk's server that reads the desk's niri for the lock state and the stop
    /// key, which no lock screen ever locks, or the host's niri, display or session bus
    /// for what it drives; one that drops the host's recorded Noctalia because its own
    /// search finds no Noctalia; and children that lose the host's bus for notifications.
    #[tokio::test]
    async fn a_joined_desk_drives_the_desk_and_keeps_the_hosts_lock_sources() {
        let dir = test_support::short_dir("env-desk");
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let _sockets = ready_desk(&dir);
        let root = dir.join("ncu-d/web");
        // This server's own search finds no Noctalia.
        let host = Env {
            runtime_dir: Some(dir.clone()),
            ..Env::default()
        };
        let roots = discover::Roots {
            run_user: &dir.join("run"),
            proc: &dir.join("proc"),
            euid: rustix::process::geteuid().as_raw(),
        };
        let name = desks::name::DeskName::parse("web").unwrap();
        let env = host
            .join_with(&name, discover::Probe::Offline, |_| None, &roots)
            .await
            .unwrap();
        let niri = root.join("niri.wayland-1.7.sock");
        assert_eq!(env.niri_socket.path().unwrap(), niri.as_path());
        assert_eq!(env.runtime_dir, Some(root));
        assert_eq!(env.session_bus, Some("unix:path=/desk/bus".into()));
        let host_niri = dir.join("niri.wayland-0.3.sock");
        assert_eq!(env.host_niri.path().unwrap(), host_niri.as_path());
        assert_eq!(env.desktop.noctalia, None);
        let lock = env.desk().unwrap().host.lock.clone();
        assert_eq!(lock.noctalia, Some(dir.join("noctalia-wayland-0.sock")));
        assert_eq!(lock.logind_session.as_deref(), Some("4"));
        let children: std::collections::BTreeMap<_, _> = env.session_vars().into_iter().collect();
        assert_eq!(children["NIRI_SOCKET"], niri.as_os_str());
        assert_eq!(children["DBUS_SESSION_BUS_ADDRESS"], "unix:path=/host/bus");
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Catches a `--desk` taken by `stop` or `resume`, which act on the host and every
    /// desk at once, and a desk name that could be an option or a path.
    #[test]
    fn a_desk_is_named_last_and_only_where_it_means_something() {
        let args = |list: &[&str]| list.iter().map(OsString::from).collect::<Vec<_>>();
        let web = Some(desks::name::DeskName::parse("web").unwrap());
        assert_eq!(
            command(&args(&["serve", "--desk", "web"])),
            Ok((Some(Command::Serve), web.clone()))
        );
        assert_eq!(
            command(&args(&["status", "--check", "--desk", "web"])),
            Ok((Some(Command::Status { check: true }), web))
        );
        for bad in [
            &["stop", "--desk", "web"][..],
            &["resume", "--desk", "web"],
            &["serve", "--desk", "../web"],
            &["serve", "--desk", "-x"],
        ] {
            assert!(command(&args(bad)).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn takes_exactly_one_known_subcommand() {
        let args = |list: &[&str]| list.iter().map(OsString::from).collect::<Vec<_>>();
        assert_eq!(known(&args(&["serve"])), Some(Command::Serve));
        assert_eq!(
            known(&args(&["status"])),
            Some(Command::Status { check: false })
        );
        assert_eq!(
            known(&args(&["status", "--check"])),
            Some(Command::Status { check: true })
        );
        assert_eq!(known(&args(&["stop"])), Some(Command::Stop));
        assert_eq!(known(&args(&["resume"])), Some(Command::Resume));
        assert_eq!(known(&args(&["watch"])), Some(Command::Watch));
        assert_eq!(known(&args(&["recover"])), Some(Command::Recover));
        assert_eq!(known(&args(&["guard", "42"])), Some(Command::Guard(42)));
        assert_eq!(known(&args(&["paste-keeper"])), Some(Command::PasteKeeper));
        for bad in [
            &[][..],
            &["stop", "now"],
            &["serve", "status"],
            &["--help"],
            &["guard"],
            &["guard", "-1"],
            &["engine", "--unscoped"],
            &["engine", "--runtime-dir-discovered", "x"],
        ] {
            assert_eq!(known(&args(bad)), None, "{bad:?}");
        }
    }
}
