//! The steps of a desk's start, in order, each within its own deadline. Each starts what it
//! starts into `Processes`, so whatever step fails, the supervisor stops all of it.

use std::ffi::OsString;
use std::fs::File;
use std::os::unix::fs::{DirBuilderExt as _, FileTypeExt as _};
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt as _, BufReader};
use tokio::time::Instant;

use super::check::{self, RoundTrips};
use super::environment::{self, Host as HostVars, Vars};
use super::layout::Layout;
use super::programs::Programs;
use super::record::{self, Endpoints, Host, Size};
use super::{config, output};
use crate::niri::round_trips::{self, Burst};
use crate::runner::desk::{DeskChild, Output, desk_child};
use crate::runner::{self, Search};

/// How long one step may take.
pub(crate) const STEP: Duration = Duration::from_secs(15);
/// How often a step looks for the socket a process makes.
const POLL: Duration = Duration::from_millis(50);
/// How long `niri validate` may take.
const VALIDATE: Duration = Duration::from_secs(10);
/// The longest one round trip of the check may take.
const EACH: Duration = Duration::from_secs(1);

/// What a desk's start knows before it starts anything.
#[derive(Debug)]
pub(crate) struct Plan {
    pub(crate) size: Size,
    pub(crate) layout: Layout,
    /// Where each process's log goes.
    pub(crate) logs: PathBuf,
    pub(crate) host: Host,
    pub(crate) vars: HostVars,
    pub(crate) programs: Programs,
    /// This binary's file, for cage's client.
    pub(crate) exe: PathBuf,
}

/// What a desk runs, in start order.
#[derive(Debug, Default)]
pub(crate) struct Processes {
    pub(crate) cage: Option<DeskChild>,
    pub(crate) bus: Option<DeskChild>,
    pub(crate) niri: Option<DeskChild>,
    pub(crate) launcher: Option<DeskChild>,
    pub(crate) registry: Option<DeskChild>,
    /// Holds the listener that keeps the desk's accessibility on.
    pub(crate) listener: Option<zbus::Connection>,
}

impl Processes {
    /// The IDs of the processes still running.
    pub(crate) fn pids(&self) -> Vec<u32> {
        [
            &self.cage,
            &self.bus,
            &self.niri,
            &self.launcher,
            &self.registry,
        ]
        .into_iter()
        .filter_map(|child| child.as_ref()?.pid())
        .collect()
    }
}

impl Plan {
    fn log(&self, name: &str) -> Result<File, String> {
        let path = self.logs.join(format!("{name}.log"));
        File::create(&path).map_err(|error| format!("create {}: {error}", path.display()))
    }

    /// The desk's directory and configs, and `niri validate`.
    pub(crate) async fn config(&self) -> Result<(), String> {
        let root = self.layout.root();
        private_dir(&self.layout.tmp())?;
        write(&self.layout.niri_config(), &config::niri())?;
        write(&self.layout.bus_config(), &config::bus(root))?;
        record::write(&self.layout.host(), &self.host)?;
        let args = [
            "validate".to_owned(),
            "-c".to_owned(),
            self.layout.niri_config().display().to_string(),
        ];
        let done = runner::run(&self.programs.niri, Search::Own, &args, VALIDATE, 64 * 1024)
            .await
            .map_err(|error| error.detail)?;
        if !done.status.success() {
            return Err(format!(
                "niri validate refused the desk's config ({}): {}",
                done.status,
                done.stderr.trim()
            ));
        }
        Ok(())
    }

    /// The display the desk's niri draws on: cage's, sized, for a headless desk; the host's
    /// for a windowed one.
    pub(crate) async fn parent(&self, processes: &mut Processes) -> Result<PathBuf, String> {
        let Some(cage) = &self.programs.cage else {
            return self.host_display();
        };
        private_dir(&self.layout.cage_dir())?;
        let args: Vec<OsString> = vec![
            "-d".into(),
            "--".into(),
            self.exe.clone().into_os_string(),
            "desk".into(),
            "hold".into(),
        ];
        let env = environment::cage(&self.vars, &self.layout);
        let child = desk_child(
            cage.as_os_str(),
            &args,
            &env,
            Output::Log,
            self.log("cage")?,
        )
        .map_err(|error| error.detail)?;
        let display = self.layout.cage_display();
        let deadline = Instant::now() + STEP;
        let running = processes.cage.insert(child);
        wait_for(
            running,
            deadline,
            || is_socket(&display).then_some(()),
            "cage's display",
        )
        .await?;
        output::set(&display, self.size).await?;
        Ok(display)
    }

    fn host_display(&self) -> Result<PathBuf, String> {
        let display = self
            .host
            .wayland_display
            .as_deref()
            .ok_or("a windowed desk needs the host's WAYLAND_DISPLAY, which isn't known")?;
        let display = self.host.runtime_dir.join(display);
        if !is_socket(&display) {
            return Err(format!(
                "the host's display {} isn't a socket",
                display.display()
            ));
        }
        Ok(display)
    }

    /// The desk's session bus, and its address.
    pub(crate) async fn bus(&self, processes: &mut Processes) -> Result<String, String> {
        let args: Vec<OsString> = vec![
            format!("--config-file={}", self.layout.bus_config().display()).into(),
            "--nofork".into(),
            "--print-address=1".into(),
        ];
        let env = environment::bus(&self.vars, &self.layout);
        let mut child = desk_child(
            self.programs.dbus_daemon.as_os_str(),
            &args,
            &env,
            Output::Piped,
            self.log("dbus")?,
        )
        .map_err(|error| error.detail)?;
        let stdout = child.take_stdout().ok_or("dbus-daemon has no stdout")?;
        let bus = processes.bus.insert(child);
        let mut line = String::new();
        let mut reader = BufReader::new(stdout);
        tokio::select! {
            read = tokio::time::timeout(STEP, reader.read_line(&mut line)) => {
                read.map_err(|_| "dbus-daemon printed no address by the step's deadline".to_owned())?
                    .map_err(|error| format!("read dbus-daemon's address: {error}"))?;
            }
            exited = bus.exited() => return Err(exited),
        }
        let address = line.trim().to_owned();
        if !address.starts_with("unix:path=") {
            return Err(format!(
                "dbus-daemon printed {address:?}, not a unix:path= address"
            ));
        }
        Ok(address)
    }

    /// The desk's niri, once its IPC socket and display exist: the socket's path and the
    /// display's name.
    pub(crate) async fn niri(
        &self,
        processes: &mut Processes,
        parent: &Path,
        bus: &str,
    ) -> Result<(PathBuf, String), String> {
        let args: Vec<OsString> = vec!["-c".into(), self.layout.niri_config().into_os_string()];
        let env = environment::niri(&self.vars, &self.layout, bus, parent);
        let child = desk_child(
            self.programs.niri.as_os_str(),
            &args,
            &env,
            Output::Log,
            self.log("niri")?,
        )
        .map_err(|error| error.detail)?;
        let pid = child.pid().ok_or("niri exited at once")?;
        let niri = processes.niri.insert(child);
        let root = self.layout.root();
        let found = wait_for(
            niri,
            Instant::now() + STEP,
            || niri_socket(root, pid),
            "niri's sockets",
        )
        .await?;
        let engine = root
            .join("niri-computer-use")
            .join(format!("niri.{}.{pid}", found.1))
            .join(crate::engine::host::SOCKET);
        if engine.as_os_str().len() > super::layout::SOCKET_PATH_MAX {
            return Err(format!(
                "the desk's server socket {} would be too long",
                engine.display()
            ));
        }
        Ok(found)
    }

    /// The accessibility bus, the registry, and the desk's accessibility on: the bus's
    /// address.
    pub(crate) async fn accessibility(
        &self,
        processes: &mut Processes,
        bus: &str,
    ) -> Result<String, String> {
        let deadline = Instant::now() + STEP;
        let mut env: Vars = environment::apps(&self.vars, &self.layout, bus);
        env.insert("ATSPI_DBUS_IMPLEMENTATION", "dbus-daemon".into());
        let launcher = desk_child(
            self.programs.launcher.as_os_str(),
            &["--launch-immediately".into()],
            &env,
            Output::Log,
            self.log("at-spi-bus")?,
        )
        .map_err(|error| error.detail)?;
        processes.launcher = Some(launcher);
        let session = super::accessibility::connect(bus, deadline).await?;
        let address = super::accessibility::address(&session, deadline).await?;
        let registry = desk_child(
            self.programs.registry.as_os_str(),
            &[],
            &environment::apps(&self.vars, &self.layout, bus),
            Output::Log,
            self.log("at-spi-registry")?,
        )
        .map_err(|error| error.detail)?;
        processes.registry = Some(registry);
        let listener = super::accessibility::turn_on(&session, &address, deadline).await?;
        processes.listener = Some(listener);
        Ok(address)
    }

    /// The round-trip check, while niri redraws.
    pub(crate) async fn round_trips(
        &self,
        endpoints: &Endpoints,
        niri_pid: u32,
    ) -> Result<RoundTrips, String> {
        let display = self.layout.root().join(&endpoints.wayland_display);
        let burst = Burst {
            count: check::BURST,
            slow: check::LIMIT,
            enough_slow: 3,
            each: EACH,
        };
        let measured = tokio::time::timeout(STEP, round_trips::measure(&display, niri_pid, burst))
            .await
            .map_err(|_| "it didn't end by the step's deadline".to_owned())
            .and_then(|measured| measured.map_err(|error| error.detail));
        check::judge(measured)
    }
}

/// `niri.<display>.<pid>.sock` in `root`, once both it and the display exist.
fn niri_socket(root: &Path, pid: u32) -> Option<(PathBuf, String)> {
    let suffix = format!(".{pid}.sock");
    std::fs::read_dir(root)
        .ok()?
        .filter_map(Result::ok)
        .find_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let display = name
                .strip_prefix("niri.")?
                .strip_suffix(&suffix)?
                .to_owned();
            let socket = entry.path();
            (is_socket(&socket) && is_socket(&root.join(&display))).then_some((socket, display))
        })
}

/// Polls `found` until it gives a value, `child` exits, or `deadline` passes.
async fn wait_for<T>(
    child: &mut DeskChild,
    deadline: Instant,
    found: impl Fn() -> Option<T>,
    what: &str,
) -> Result<T, String> {
    loop {
        if let Some(value) = found() {
            return Ok(value);
        }
        if Instant::now() >= deadline {
            return Err(format!("no {what} by the step's deadline"));
        }
        tokio::select! {
            () = tokio::time::sleep(POLL) => {}
            exited = child.exited() => return Err(format!("{exited} before {what} appeared")),
        }
    }
}

fn is_socket(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_socket())
}

fn private_dir(dir: &Path) -> Result<(), String> {
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(dir)
        .map_err(|error| format!("create {}: {error}", dir.display()))
}

fn write(path: &Path, text: &str) -> Result<(), String> {
    std::fs::write(path, text).map_err(|error| format!("write {}: {error}", path.display()))
}
