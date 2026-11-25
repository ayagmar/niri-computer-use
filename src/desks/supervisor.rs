//! `desk supervise`: the process that owns one desk, in the desk's systemd scope. It makes
//! the desk's directory, runs the start's steps, and reports each, then the outcome, as a
//! JSON line on its stdout for `desk start`. A ready desk runs until `desk stop` asks over
//! the control socket or one of its processes exits. Whatever ends it, from the moment the
//! directory exists, one path stops it: every process in reverse order, then what they
//! left, which the kernel gave the supervisor as their subreaper, all within one deadline,
//! and then a check that nothing but the supervisor and its guard is left in the scope.
//! The record says how it ended. The directory stays: `desk stop`, or the next `desk
//! start`, removes it once the scope is stopped and empty. The desk's lock, which `desk
//! start` handed it, is let go once the desk is published ready, or when the supervisor
//! exits; its guard stops the scope once it has exited, however it exited.

use std::path::Path;
use std::time::Duration;

use rustix::process::Signal;
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::net::UnixListener;
use tokio::net::unix::pipe;
use tokio::sync::watch;
use tokio::time::Instant;

use super::layout::Layout;
use super::record::{self, Endpoints, Phase, Record, Step};
use super::scope;
use super::serial::Handed;
use super::steps::{Plan, Processes};
use crate::control::procs;
use crate::runner::desk::{DeskChild, Guard};

/// What `desk stop` writes to the control socket.
pub(crate) const STOP: &str = "stop";
/// How long niri gets to exit after `SIGTERM`, and each other process.
const NIRI_GRACE: Duration = Duration::from_secs(5);
const GRACE: Duration = Duration::from_secs(2);
/// How long what the desk's apps left gets after `SIGTERM`.
const SWEEP_GRACE: Duration = Duration::from_secs(3);
const SWEEP_POLL: Duration = Duration::from_millis(50);
/// The whole shutdown: every process's stop, the sweeps and the check that the scope holds
/// nothing else.
const SHUTDOWN: Duration = Duration::from_secs(15);
/// The longest one progress line may take to write. A reader that stalls loses the rest.
const PROGRESS: Duration = Duration::from_secs(1);

/// The record the supervisor keeps, the pipe it reports on, and the desk's lock until the
/// desk is ready.
struct Desk {
    layout: Layout,
    record: Record,
    progress: Option<pipe::Sender>,
    lock: Option<Handed>,
}

impl Desk {
    /// Writes the record as a line to the progress pipe, if there still is one, within
    /// `PROGRESS`. `desk start` may have gone or stalled: the desk runs on without it.
    async fn report(&mut self) {
        let Some(progress) = &mut self.progress else {
            return;
        };
        let mut text = serde_json::to_string(&self.record).unwrap_or_default();
        text.push('\n');
        let wrote = tokio::time::timeout(PROGRESS, async {
            progress.write_all(text.as_bytes()).await?;
            progress.flush().await
        })
        .await;
        if !matches!(wrote, Ok(Ok(()))) {
            self.progress = None;
        }
    }

    /// Enters `phase`: writes the record, then reports it. A record that can't be written
    /// is an error, and nothing is reported.
    async fn publish(&mut self, phase: Phase) -> Result<(), String> {
        self.record.phase = phase;
        record::write(&self.layout.record(), &self.record)?;
        self.report().await;
        Ok(())
    }

    /// Enters `phase` once the desk is already stopping or failed: a record that can't be
    /// written is logged, and the phase is reported either way.
    async fn note(&mut self, phase: Phase) {
        self.record.phase = phase;
        if let Err(error) = record::write(&self.layout.record(), &self.record) {
            crate::cli::print_error(&error);
        }
        self.report().await;
    }

    const fn step(&self) -> Step {
        match &self.record.phase {
            Phase::Starting { step } | Phase::Failed { step, .. } => *step,
            Phase::Ready { .. } | Phase::Stopping | Phase::Stopped { .. } => Step::RoundTrips,
        }
    }

    /// Runs `start` until it gives the ready phase, unless `desk stop` asks first, and
    /// publishes it. Then the progress pipe is closed, `desk start` has its answer, and the
    /// lock is let go.
    async fn become_ready(
        &mut self,
        asked: &watch::Receiver<bool>,
        start: impl AsyncFnOnce(&mut Self) -> Result<Phase, String>,
    ) -> Result<(), String> {
        let mut asked = asked.clone();
        let ready = tokio::select! {
            biased;
            _ = asked.wait_for(|asked| *asked) => Err("desk stop asked for it during the start".to_owned()),
            ready = start(self) => ready,
        }?;
        self.publish(ready).await?;
        self.progress = None;
        if let Some(lock) = self.lock.take() {
            lock.release();
        }
        Ok(())
    }

    /// Records how the desk ended: `outcome` is why it stopped once it was ready, or why
    /// its start failed; `cleaned` what its shutdown left, if anything. Fails unless the
    /// desk was ready and nothing was left.
    async fn finish(
        mut self,
        outcome: Result<String, String>,
        cleaned: Result<(), String>,
    ) -> Result<(), String> {
        let left = cleaned.err();
        let with_left = |reason: String| match &left {
            Some(left) => format!("{reason}; {left}"),
            None => reason,
        };
        match outcome {
            Ok(why) => {
                self.note(Phase::Stopped {
                    reason: with_left(why),
                })
                .await;
                left.map_or(Ok(()), Err)
            }
            Err(reason) => {
                let step = self.step();
                let reason = with_left(reason);
                self.note(Phase::Failed {
                    step,
                    reason: reason.clone(),
                })
                .await;
                Err(reason)
            }
        }
    }
}

/// Runs the desk as `plan` says, with `record` as its first record, until it stops, with
/// `guard` left running and `lock` held until the desk is ready. Returns why it stopped
/// early, if it never became ready, or what its stop left.
pub(crate) async fn run(
    plan: Plan,
    record: Record,
    guard: Guard,
    lock: Handed,
) -> Result<(), String> {
    let guard = guard.pid().ok_or("the desk's guard has exited")?;
    let layout = plan.layout.clone();
    make_root(&layout)?;
    let mut desk = Desk {
        layout,
        record,
        progress: progress(),
        lock: Some(lock),
    };
    let mut processes = Processes::default();
    let outcome = live(&plan, &mut desk, &mut processes).await;
    match &outcome {
        Ok(why) => {
            crate::cli::print_error(&format!("desk {} stops: {why}", desk.record.name));
            desk.note(Phase::Stopping).await;
        }
        Err(reason) => {
            crate::cli::print_error(&format!("desk {} failed: {reason}", desk.record.name));
        }
    }
    let cleaned = shutdown(&mut processes, &desk.record.cgroup, guard).await;
    desk.finish(outcome, cleaned).await
}

/// The supervisor's stdout as a non-blocking pipe, so a stalled reader can't hold a write
/// past its deadline; `None` when it isn't a pipe.
fn progress() -> Option<pipe::Sender> {
    use std::os::fd::AsFd as _;
    let opened = tokio::io::stdout()
        .as_fd()
        .try_clone_to_owned()
        .and_then(pipe::Sender::from_owned_fd);
    match opened {
        Ok(sender) => Some(sender),
        Err(error) => {
            crate::cli::print_error(&format!("report the desk's progress on stdout: {error}"));
            None
        }
    }
}

/// Everything from the first record until the desk is to stop: why it stops, once it was
/// ready, or why it failed.
async fn live(plan: &Plan, desk: &mut Desk, processes: &mut Processes) -> Result<String, String> {
    desk.publish(Phase::Starting { step: Step::Config }).await?;
    let asked = listen(&desk.layout)?;
    rustix::process::set_child_subreaper(Some(rustix::process::getpid()))
        .map_err(|error| format!("become the desk's subreaper: {error}"))?;
    desk.become_ready(&asked, async |starting: &mut Desk| {
        start(plan, processes, starting).await
    })
    .await?;
    Ok(serve(processes, asked).await)
}

/// Runs the steps in order, entering each in the record first, and gives the ready phase.
async fn start(plan: &Plan, processes: &mut Processes, desk: &mut Desk) -> Result<Phase, String> {
    plan.config().await?;
    desk.publish(Phase::Starting { step: Step::Parent }).await?;
    let parent = plan.parent(processes).await?;
    desk.publish(Phase::Starting { step: Step::Bus }).await?;
    let bus = plan.bus(processes).await?;
    desk.publish(Phase::Starting { step: Step::Niri }).await?;
    let (niri_socket, wayland_display) = plan.niri(processes, &parent, &bus).await?;
    desk.publish(Phase::Starting {
        step: Step::Accessibility,
    })
    .await?;
    let accessibility_bus = plan.accessibility(processes, &bus).await?;
    desk.publish(Phase::Starting {
        step: Step::RoundTrips,
    })
    .await?;
    let endpoints = Endpoints {
        niri_socket,
        wayland_display,
        session_bus: bus,
        accessibility_bus,
    };
    let niri_pid = processes
        .niri
        .as_ref()
        .and_then(DeskChild::pid)
        .ok_or("niri exited")?;
    let round_trips = plan.round_trips(&endpoints, niri_pid).await?;
    Ok(Phase::Ready {
        endpoints,
        round_trips,
    })
}

/// The desk's directory, made by its supervisor alone, under the desk's lock.
fn make_root(layout: &Layout) -> Result<(), String> {
    use std::os::unix::fs::DirBuilderExt as _;
    let parent = layout.root().parent().unwrap_or_else(|| Path::new("/"));
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(parent)
        .map_err(|error| format!("create {}: {error}", parent.display()))?;
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(layout.root())
        .map_err(|error| format!("create {}: {error}", layout.root().display()))
}

/// Listens on the control socket; the receiver turns true once `desk stop` asks.
fn listen(layout: &Layout) -> Result<watch::Receiver<bool>, String> {
    let path = layout.control();
    let listener = UnixListener::bind(&path)
        .map_err(|error| format!("listen on {}: {error}", path.display()))?;
    let (asked, receiver) = watch::channel(false);
    tokio::spawn(async move {
        #[expect(
            clippy::collection_is_never_read,
            reason = "the connections are held, unread, so each closes as the supervisor exits, which `desk stop` waits for"
        )]
        let mut held = Vec::new();
        while let Ok((stream, _)) = listener.accept().await {
            let mut lines = BufReader::new(stream);
            let mut line = String::new();
            let read = tokio::time::timeout(GRACE, lines.read_line(&mut line)).await;
            if matches!(read, Ok(Ok(_))) && line.trim() == STOP {
                asked.send_replace(true);
            }
            held.push(lines);
        }
    });
    Ok(receiver)
}

/// Waits until `desk stop` asks or one of the desk's processes exits, and says which.
async fn serve(processes: &mut Processes, mut asked: watch::Receiver<bool>) -> String {
    tokio::select! {
        _ = asked.wait_for(|asked| *asked) => "desk stop asked for it".to_owned(),
        exited = exited(&mut processes.niri) => exited,
        exited = exited(&mut processes.cage) => exited,
        exited = exited(&mut processes.bus) => exited,
        exited = exited(&mut processes.launcher) => exited,
        exited = exited(&mut processes.registry) => exited,
    }
}

async fn exited(child: &mut Option<DeskChild>) -> String {
    match child {
        Some(child) => child.exited().await,
        None => std::future::pending().await,
    }
}

/// Stops every process within `SHUTDOWN`, in reverse order: niri first, then what its
/// apps left, then the daemons, then anything left but the `guard`; then waits until
/// nothing but the supervisor and the guard is in the scope's `cgroup`. `Ok` only once
/// that is checked. The guard stays: once the supervisor exits, it stops the scope, which
/// then holds nothing else.
async fn shutdown(processes: &mut Processes, cgroup: &str, guard: u32) -> Result<(), String> {
    let until = Instant::now() + SHUTDOWN;
    processes.listener = None;
    let mut errors = Vec::new();
    stop_one(&mut processes.niri, NIRI_GRACE, until, &mut errors).await;
    let mut kept = processes.pids();
    kept.push(guard);
    sweep(&kept, until).await;
    stop_one(&mut processes.registry, GRACE, until, &mut errors).await;
    stop_one(&mut processes.launcher, GRACE, until, &mut errors).await;
    stop_one(&mut processes.bus, GRACE, until, &mut errors).await;
    stop_one(&mut processes.cage, GRACE, until, &mut errors).await;
    sweep(&[guard], until).await;
    let emptied = emptied(cgroup, guard, until).await;
    match (emptied, errors.is_empty()) {
        (Ok(()), _) => Ok(()),
        (Err(left), true) => Err(left),
        (Err(left), false) => Err(format!("{left} ({})", errors.join("; "))),
    }
}

async fn stop_one(
    child: &mut Option<DeskChild>,
    grace: Duration,
    until: Instant,
    errors: &mut Vec<String>,
) {
    let grace = grace.min(until.saturating_duration_since(Instant::now()));
    if let Some(running) = child.as_mut()
        && let Err(error) = running.stop(grace).await
    {
        errors.push(format!("stop {}: {error}", running.program()));
    }
    // Gone, or left to the sweep and the scope either way.
    child.take();
}

/// The supervisor's living children but `kept`: the processes it started, and what the
/// kernel gave it as their subreaper. It never reaps those, so their PIDs stay theirs.
fn living_children(kept: &[u32]) -> Vec<u32> {
    let proc_root = Path::new("/proc");
    procs::children(proc_root, std::process::id())
        .into_iter()
        .filter(|pid| !kept.contains(pid))
        .filter(|&pid| procs::stat(proc_root, pid).is_some_and(|stat| !stat.exited()))
        .collect()
}

fn signal(pids: &[u32], signal: Signal) {
    for &pid in pids {
        if let Some(pid) = i32::try_from(pid)
            .ok()
            .and_then(rustix::process::Pid::from_raw)
        {
            rustix::process::kill_process(pid, signal).ok();
        }
    }
}

/// Stops every process left under the supervisor but `kept`: `SIGTERM`, then `SIGKILL`
/// for what is left after `SWEEP_GRACE`, or at `until`.
async fn sweep(kept: &[u32], until: Instant) {
    let found = living_children(kept);
    if found.is_empty() {
        return;
    }
    signal(&found, Signal::TERM);
    let deadline = until.min(Instant::now() + SWEEP_GRACE);
    while Instant::now() < deadline && !living_children(kept).is_empty() {
        tokio::time::sleep(SWEEP_POLL).await;
    }
    signal(&living_children(kept), Signal::KILL);
}

/// Waits until the scope's `cgroup` holds no living process but the supervisor and its
/// `guard`, killing what comes to it meanwhile, until `until`; then what is left.
async fn emptied(cgroup: &str, guard: u32, until: Instant) -> Result<(), String> {
    let root = Path::new(scope::CGROUP_ROOT);
    let proc_root = Path::new("/proc");
    let me = std::process::id();
    loop {
        let others: Vec<u32> = scope::members(root, cgroup)?
            .into_iter()
            .filter(|&pid| pid != me && pid != guard)
            .filter(|&pid| procs::stat(proc_root, pid).is_some_and(|stat| !stat.exited()))
            .collect();
        if others.is_empty() {
            return Ok(());
        }
        if Instant::now() >= until {
            let named: Vec<String> = others
                .iter()
                .map(|&pid| {
                    format!(
                        "{pid} ({})",
                        procs::comm(proc_root, pid).unwrap_or_default()
                    )
                })
                .collect();
            return Err(format!(
                "left in the desk's scope after {SHUTDOWN:?}: {}",
                named.join(", ")
            ));
        }
        signal(&living_children(&[guard]), Signal::KILL);
        tokio::time::sleep(SWEEP_POLL).await;
    }
}

#[cfg(test)]
mod tests {
    use tokio::io::AsyncReadExt as _;

    use super::*;
    use crate::desks::check::RoundTrips;
    use crate::desks::layout::Desks;
    use crate::desks::name::DeskName;
    use crate::desks::record::{Mode, Supervisor};
    use crate::desks::scope::Generation;

    fn desk(dir: &Path, progress: Option<pipe::Sender>) -> Desk {
        let layout = Desks::of(dir)
            .desk(&DeskName::parse("web").unwrap())
            .unwrap();
        std::fs::create_dir_all(layout.root()).unwrap();
        Desk {
            record: Record {
                name: DeskName::parse("web").unwrap(),
                mode: Mode::Headless,
                size: None,
                supervisor: Supervisor {
                    pid: 1,
                    start_time: 1,
                },
                generation: Generation::parse("1").unwrap(),
                cgroup: "/x.scope".to_owned(),
                started: String::new(),
                phase: Phase::Starting { step: Step::Config },
            },
            layout,
            progress,
            lock: None,
        }
    }

    fn ready(root: &Path) -> Phase {
        Phase::Ready {
            endpoints: Endpoints {
                niri_socket: root.join("niri.sock"),
                wayland_display: "wayland-1".to_owned(),
                session_bus: "unix:path=/nowhere".to_owned(),
                accessibility_bus: "unix:path=/nowhere".to_owned(),
            },
            round_trips: RoundTrips {
                count: 1,
                p50_us: 1,
                p95_us: 1,
                max_us: 1,
            },
        }
    }

    /// Catches a desk whose ready record couldn't be written but which `desk start` is told
    /// is ready, so every join of it fails: the start fails through the same path as any
    /// other failure, and the progress says failed, never ready.
    #[tokio::test]
    async fn a_ready_record_that_cant_be_written_fails_the_start() {
        let dir = crate::test_support::short_dir("publish");
        let (sender, mut receiver) = pipe::pipe().unwrap();
        let mut desk = desk(&dir, Some(sender));
        // A directory in the record's place, with something in it, can't be replaced.
        std::fs::create_dir_all(desk.layout.record().join("x")).unwrap();
        let (_asked, asked) = watch::channel(false);
        let root = desk.layout.root().to_path_buf();
        let outcome = desk
            .become_ready(&asked, async |_: &mut Desk| Ok(ready(&root)))
            .await;
        let error = outcome.clone().unwrap_err();
        assert!(error.contains("desk.json"), "{error}");
        let finished = desk.finish(outcome.map(|()| String::new()), Ok(())).await;
        assert!(finished.is_err());
        let mut said = vec![0; 64 * 1024];
        let read = receiver.read(&mut said).await.unwrap();
        let said = String::from_utf8_lossy(&said[..read]);
        assert!(!said.contains("\"ready\""), "{said}");
        assert!(said.contains("\"failed\""), "{said}");
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Fills the pipe, as a reader that stopped reading leaves it.
    async fn fill(sender: &mut pipe::Sender) {
        let mut wrote = async |chunk: &[u8]| {
            tokio::time::timeout(Duration::from_millis(20), sender.write_all(chunk))
                .await
                .is_ok()
        };
        while wrote(&[b'x'; 4096]).await {}
        while wrote(b"x").await {}
    }

    /// Catches a supervisor that a `desk start` which stopped reading holds in a progress
    /// write, so it never reaches its stop or its cleanup: each line has a deadline, and
    /// the record is still written.
    #[tokio::test]
    async fn a_stalled_progress_reader_costs_one_deadline_and_the_record_is_kept() {
        let dir = crate::test_support::short_dir("stall");
        let (mut sender, _unread) = pipe::pipe().unwrap();
        fill(&mut sender).await;
        let mut desk = desk(&dir, Some(sender));
        let began = Instant::now();
        desk.publish(Phase::Starting { step: Step::Niri })
            .await
            .unwrap();
        assert!(desk.progress.is_none());
        desk.publish(Phase::Starting { step: Step::Bus })
            .await
            .unwrap();
        assert!(began.elapsed() < PROGRESS + Duration::from_secs(1));
        let written: Record = record::read(&desk.layout.record()).unwrap();
        assert_eq!(written.phase, Phase::Starting { step: Step::Bus });
        std::fs::remove_dir_all(dir).unwrap();
    }
}
