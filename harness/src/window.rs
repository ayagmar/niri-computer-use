//! `harness window`: a fixture app for M3's launch, focus and close checks. niri starts it
//! from a launch preset, so it runs with niri's environment, which it checks is NESTED
//! before it connects. It maps one or more plain toplevels and can start late, set its
//! `app_id` only after mapping, and ignore close requests the way an app asking about
//! unsaved changes does. It exits when its windows are closed, when niri goes away, or at
//! its own deadline.

use std::os::fd::AsFd as _;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use rustix::event::{PollFd, PollFlags, Timespec, poll};
use rustix::fs::{MemfdFlags, ftruncate, memfd_create};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_buffer::WlBuffer;
use wayland_client::protocol::wl_compositor::WlCompositor;
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::protocol::wl_shm::{self, WlShm};
use wayland_client::protocol::wl_shm_pool::WlShmPool;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle, delegate_noop};
use wayland_protocols::xdg::shell::client::xdg_surface::{self, XdgSurface};
use wayland_protocols::xdg::shell::client::xdg_toplevel::{self, XdgToplevel};
use wayland_protocols::xdg::shell::client::xdg_wm_base::{self, XdgWmBase};

use crate::failure::{Context as _, Failure, Result};
use crate::nested::Nested;
use crate::test_dir::TestDir;

/// Longer than any M3 run, so a window never outlives a run by much.
const DEADLINE: Duration = Duration::from_secs(90);
const WIDTH: i32 = 320;
const HEIGHT: i32 = 240;

pub(crate) const USAGE: &str = "usage: harness window <TEST_DIR> <app_id> [--count <n>] [--delay <ms>] [--late <ms>] [--keep-open] [--started <file>] [--deadline <ms>]";

/// What the fixture does.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Options {
    pub(crate) test_dir: PathBuf,
    pub(crate) app_id: String,
    /// How many toplevels to map.
    pub(crate) count: u32,
    /// How long to wait before connecting.
    pub(crate) delay: Duration,
    /// Set the `app_id` this long after the windows are mapped, instead of before.
    pub(crate) late: Option<Duration>,
    /// Ignore close requests.
    pub(crate) keep_open: bool,
    /// A file to create as soon as the fixture starts, before its delay.
    pub(crate) started: Option<PathBuf>,
    /// How long the fixture runs at most.
    pub(crate) deadline: Duration,
}

impl Options {
    pub(crate) fn parse(args: &[&str]) -> Result<Self> {
        let [test_dir, app_id, rest @ ..] = args else {
            return Err(Failure::new(USAGE));
        };
        let mut options = Self {
            test_dir: PathBuf::from(test_dir),
            app_id: (*app_id).to_owned(),
            count: 1,
            delay: Duration::ZERO,
            late: None,
            keep_open: false,
            started: None,
            deadline: DEADLINE,
        };
        let millis = |value: Option<&&str>| -> Result<Duration> {
            let value = value.ok_or_else(|| Failure::new(USAGE))?;
            Ok(Duration::from_millis(value.parse().context(USAGE)?))
        };
        let mut rest = rest.iter();
        while let Some(&flag) = rest.next() {
            match flag {
                "--count" => {
                    let value = rest.next().ok_or_else(|| Failure::new(USAGE))?;
                    options.count = value.parse().context(USAGE)?;
                }
                "--delay" => options.delay = millis(rest.next())?,
                "--late" => options.late = Some(millis(rest.next())?),
                "--keep-open" => options.keep_open = true,
                "--deadline" => options.deadline = millis(rest.next())?,
                "--started" => {
                    let value = rest.next().ok_or_else(|| Failure::new(USAGE))?;
                    options.started = Some(PathBuf::from(value));
                }
                _ => return Err(Failure::new(USAGE)),
            }
        }
        if options.count == 0 || options.app_id.is_empty() {
            return Err(Failure::new(USAGE));
        }
        Ok(options)
    }
}

/// The fixture's windows and what it has been asked to do.
#[derive(Debug)]
struct State {
    keep_open: bool,
    /// Toplevels the compositor has configured and that got a buffer.
    mapped: u32,
    closed: u32,
    buffer: WlBuffer,
}

pub(crate) fn run(options: &Options) -> Result<()> {
    let end = Instant::now() + options.deadline;
    if let Some(started) = &options.started {
        std::fs::write(started, "").context(format!("write {}", started.display()))?;
    }
    // Before connecting: niri gives what it spawns its own environment.
    Nested::from_env(&TestDir::open(options.test_dir.clone())?)?;
    pause(options.delay);
    let connection = Connection::connect_to_env().context("connect to the nested niri")?;
    let (globals, mut queue) =
        registry_queue_init::<State>(&connection).context("read the globals")?;
    let handle = queue.handle();
    let compositor: WlCompositor = globals
        .bind(&handle, 4..=6, ())
        .context("bind wl_compositor")?;
    let shm: WlShm = globals.bind(&handle, 1..=1, ()).context("bind wl_shm")?;
    let shell: XdgWmBase = globals
        .bind(&handle, 1..=6, ())
        .context("bind xdg_wm_base")?;
    let mut state = State {
        keep_open: options.keep_open,
        mapped: 0,
        closed: 0,
        buffer: buffer(&shm, &handle)?,
    };
    let toplevels: Vec<(WlSurface, XdgToplevel)> = (0..options.count)
        .map(|_| {
            let surface = compositor.create_surface(&handle, ());
            let xdg = shell.get_xdg_surface(&surface, &handle, surface.clone());
            let toplevel = xdg.get_toplevel(&handle, ());
            toplevel.set_title("harness window".to_owned());
            if options.late.is_none() {
                toplevel.set_app_id(options.app_id.clone());
            }
            surface.commit();
            (surface, toplevel)
        })
        .collect();
    if let Some(late) = options.late {
        dispatch_until(&mut queue, &mut state, end, |state| {
            state.mapped == options.count
        })?;
        pause(late);
        for (surface, toplevel) in &toplevels {
            toplevel.set_app_id(options.app_id.clone());
            surface.commit();
        }
    }
    dispatch_until(&mut queue, &mut state, end, |state| {
        state.closed == options.count
    })
}

/// Dispatches events until `done` holds, niri goes away, or `end`. Only a protocol or
/// connection failure before `end` other than niri going away is an error.
fn dispatch_until(
    queue: &mut EventQueue<State>,
    state: &mut State,
    end: Instant,
    done: impl Fn(&State) -> bool,
) -> Result<()> {
    loop {
        queue.dispatch_pending(state).context("dispatch")?;
        if done(state) {
            return Ok(());
        }
        if queue.flush().is_err() {
            // niri is gone, as at the end of a run.
            return Ok(());
        }
        let Some(guard) = queue.prepare_read() else {
            continue;
        };
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Ok(());
        }
        let timeout = Timespec::try_from(left).context("timeout")?;
        let fd = guard.connection_fd();
        let mut fds = [PollFd::new(&fd, PollFlags::IN)];
        poll(&mut fds, Some(&timeout)).context("poll the connection")?;
        if fds.iter().any(|polled| !polled.revents().is_empty()) && guard.read().is_err() {
            return Ok(());
        }
    }
}

/// One black buffer, shared by every window. Its memory is never written, so it stays
/// zero: black in `XRGB8888`.
fn buffer(shm: &WlShm, handle: &QueueHandle<State>) -> Result<WlBuffer> {
    let stride = WIDTH * 4;
    let size = stride * HEIGHT;
    let fd = memfd_create("harness-window", MemfdFlags::CLOEXEC).context("memfd_create")?;
    ftruncate(&fd, u64::try_from(size).context("buffer size")?).context("size the buffer")?;
    let pool = shm.create_pool(fd.as_fd(), size, handle, ());
    let buffer = pool.create_buffer(
        0,
        WIDTH,
        HEIGHT,
        stride,
        wl_shm::Format::Xrgb8888,
        handle,
        (),
    );
    pool.destroy();
    Ok(buffer)
}

#[expect(
    clippy::disallowed_methods,
    reason = "the fixture is a plain blocking program; this is its configured delay, bounded \
              by the caller"
)]
fn pause(duration: Duration) {
    std::thread::sleep(duration);
}

impl Dispatch<WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: <WlRegistry as wayland_client::Proxy>::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<XdgWmBase, ()> for State {
    fn event(
        _: &mut Self,
        shell: &XdgWmBase,
        event: xdg_wm_base::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_wm_base::Event::Ping { serial } = event {
            shell.pong(serial);
        }
    }
}

/// The first configure maps the window: ack it, attach the buffer, commit.
impl Dispatch<XdgSurface, WlSurface> for State {
    fn event(
        state: &mut Self,
        xdg: &XdgSurface,
        event: xdg_surface::Event,
        surface: &WlSurface,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            xdg.ack_configure(serial);
            surface.attach(Some(&state.buffer), 0, 0);
            surface.damage_buffer(0, 0, WIDTH, HEIGHT);
            surface.commit();
            state.mapped += 1;
        }
    }
}

impl Dispatch<XdgToplevel, ()> for State {
    fn event(
        state: &mut Self,
        toplevel: &XdgToplevel,
        event: xdg_toplevel::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if matches!(event, xdg_toplevel::Event::Close) && !state.keep_open {
            toplevel.destroy();
            state.closed += 1;
        }
    }
}

delegate_noop!(State: ignore WlCompositor);
delegate_noop!(State: ignore WlSurface);
delegate_noop!(State: ignore WlShm);
delegate_noop!(State: ignore WlShmPool);
delegate_noop!(State: ignore WlBuffer);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_fixture_options() {
        let plain = Options::parse(&["/t", "one"]).unwrap();
        assert_eq!(
            plain,
            Options {
                test_dir: PathBuf::from("/t"),
                app_id: "one".to_owned(),
                count: 1,
                delay: Duration::ZERO,
                late: None,
                keep_open: false,
                started: None,
                deadline: DEADLINE,
            }
        );
        let all = Options::parse(&[
            "/t",
            "x",
            "--count",
            "2",
            "--delay",
            "1500",
            "--late",
            "300",
            "--keep-open",
            "--started",
            "/t/s",
            "--deadline",
            "600000",
        ])
        .unwrap();
        assert_eq!(all.count, 2);
        assert_eq!(all.delay, Duration::from_millis(1500));
        assert_eq!(all.late, Some(Duration::from_millis(300)));
        assert!(all.keep_open);
        assert_eq!(all.started, Some(PathBuf::from("/t/s")));
        assert_eq!(all.deadline, Duration::from_mins(10));
        for bad in [
            &["/t"][..],
            &["/t", ""],
            &["/t", "x", "--count", "0"],
            &["/t", "x", "--late"],
            &["/t", "x", "--wat"],
        ] {
            assert!(Options::parse(bad).is_err(), "{bad:?}");
        }
    }
}
