//! `desk supervise`: the process that owns one desk. It makes the desk's directory, runs
//! the start's steps, and reports each, then the outcome, as a JSON line on its stdout for
//! `desk start`. A ready desk runs until `desk stop` asks over the control socket or one of
//! its processes exits; then the supervisor stops every process, in reverse order, each
//! within a deadline, and removes the directory. It is the subreaper of what the desk
//! starts, so an app niri started and left is found and stopped too. If the supervisor
//! itself is killed, each of its processes gets `SIGTERM` from the kernel.

use std::path::Path;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::net::UnixListener;
use tokio::sync::watch;

use super::layout::Layout;
use super::name::DeskName;
use super::record::{self, Endpoints, Phase, Record, Step};
use super::steps::{Plan, Processes};
use crate::control::procs;
use crate::runner::desk::DeskChild;

/// What `desk stop` writes to the control socket.
pub(crate) const STOP: &str = "stop";
/// How long niri gets to exit after `SIGTERM`, and each other process.
const NIRI_GRACE: Duration = Duration::from_secs(5);
const GRACE: Duration = Duration::from_secs(2);
/// How long what the desk's apps left gets after `SIGTERM`.
const SWEEP_GRACE: Duration = Duration::from_secs(3);
const SWEEP_POLL: Duration = Duration::from_millis(50);

/// The record the supervisor keeps, and the pipe it reports on.
struct Desk {
    layout: Layout,
    record: Record,
    progress: Option<tokio::io::Stdout>,
}

impl Desk {
    async fn report(&mut self, line: serde_json::Value) {
        if let Some(progress) = &mut self.progress {
            let mut text = line.to_string();
            text.push('\n');
            // `desk start` may have gone; the desk runs on without it.
            if progress.write_all(text.as_bytes()).await.is_err() || progress.flush().await.is_err()
            {
                self.progress = None;
            }
        }
    }

    async fn enter(&mut self, phase: Phase) {
        self.record.phase = phase;
        if let Err(error) = record::write(&self.layout.record(), &self.record) {
            crate::cli::print_error(&error);
        }
        let line = serde_json::to_value(&self.record).unwrap_or_default();
        self.report(line).await;
    }

    const fn step(&self) -> Step {
        match &self.record.phase {
            Phase::Starting { step } | Phase::Failed { step, .. } => *step,
            Phase::Ready { .. } | Phase::Stopping => Step::RoundTrips,
        }
    }
}

/// Runs the desk `name` as `plan` says until it stops. Returns why it stopped early, if it
/// did not become ready.
pub(crate) async fn run(plan: Plan, name: DeskName, record: Record) -> Result<(), String> {
    let layout = plan.layout.clone();
    make_root(&layout)?;
    let mut desk = Desk {
        layout: layout.clone(),
        record,
        progress: Some(tokio::io::stdout()),
    };
    desk.enter(Phase::Starting { step: Step::Config }).await;
    let stop_asked = listen(&layout)?;
    rustix::process::set_child_subreaper(Some(rustix::process::getpid()))
        .map_err(|error| format!("become the desk's subreaper: {error}"))?;
    let mut processes = Processes::default();
    let started = {
        let mut asked = stop_asked.clone();
        tokio::select! {
            biased;
            _ = asked.wait_for(|asked| *asked) => Err("desk stop asked for it during the start".to_owned()),
            started = start(&plan, &mut processes, &mut desk) => started,
        }
    };
    let outcome = match started {
        Ok(()) => {
            let why = serve(&mut processes, stop_asked).await;
            crate::cli::print_error(&format!("desk {name} stops: {why}"));
            Ok(())
        }
        Err(reason) => {
            let step = desk.step();
            desk.enter(Phase::Failed {
                step,
                reason: reason.clone(),
            })
            .await;
            Err(reason)
        }
    };
    if outcome.is_ok() {
        desk.enter(Phase::Stopping).await;
    }
    stop(&mut processes).await;
    std::fs::remove_dir_all(layout.root())
        .map_err(|error| format!("remove {}: {error}", layout.root().display()))
        .and(outcome)
}

/// Runs the steps in order, entering each in the record first.
async fn start(plan: &Plan, processes: &mut Processes, desk: &mut Desk) -> Result<(), String> {
    plan.config().await?;
    desk.enter(Phase::Starting { step: Step::Parent }).await;
    let parent = plan.parent(processes).await?;
    desk.enter(Phase::Starting { step: Step::Bus }).await;
    let bus = plan.bus(processes).await?;
    desk.enter(Phase::Starting { step: Step::Niri }).await;
    let (niri_socket, wayland_display) = plan.niri(processes, &parent, &bus).await?;
    desk.enter(Phase::Starting {
        step: Step::Accessibility,
    })
    .await;
    let accessibility_bus = plan.accessibility(processes, &bus).await?;
    desk.enter(Phase::Starting {
        step: Step::RoundTrips,
    })
    .await;
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
    desk.enter(Phase::Ready {
        endpoints,
        round_trips,
    })
    .await;
    desk.progress = None;
    Ok(())
}

/// The desk's directory, made by its supervisor alone: a second `desk start` of the same
/// name finds it made and fails.
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

/// Stops every process, in reverse order: niri first, then what its apps left, which the
/// kernel gave the supervisor as their subreaper, then the daemons, then anything left.
async fn stop(processes: &mut Processes) {
    processes.listener = None;
    stop_one(&mut processes.niri, NIRI_GRACE).await;
    sweep(&processes.pids()).await;
    stop_one(&mut processes.registry, GRACE).await;
    stop_one(&mut processes.launcher, GRACE).await;
    stop_one(&mut processes.bus, GRACE).await;
    stop_one(&mut processes.cage, GRACE).await;
    sweep(&[]).await;
}

async fn stop_one(child: &mut Option<DeskChild>, grace: Duration) {
    if let Some(running) = child.as_mut()
        && let Err(error) = running.stop(grace).await
    {
        crate::cli::print_error(&format!("stop {}: {error}", running.program()));
    }
    // Gone, or no longer stoppable either way.
    child.take();
}

/// Stops every process left under the supervisor but `kept`: `SIGTERM`, then `SIGKILL`
/// for what is left after `SWEEP_GRACE`.
async fn sweep(kept: &[u32]) {
    let proc_root = Path::new("/proc");
    let me = std::process::id();
    let left = |root: &Path| -> Vec<u32> {
        procs::children(root, me)
            .into_iter()
            .filter(|pid| !kept.contains(pid))
            .filter(|&pid| procs::stat(root, pid).is_some_and(|stat| !stat.exited()))
            .collect()
    };
    let signal = |pids: &[u32], signal| {
        for &pid in pids {
            if let Some(pid) = i32::try_from(pid)
                .ok()
                .and_then(rustix::process::Pid::from_raw)
            {
                rustix::process::kill_process(pid, signal).ok();
            }
        }
    };
    let found = left(proc_root);
    if found.is_empty() {
        return;
    }
    signal(&found, rustix::process::Signal::TERM);
    let deadline = tokio::time::Instant::now() + SWEEP_GRACE;
    while tokio::time::Instant::now() < deadline && !left(proc_root).is_empty() {
        tokio::time::sleep(SWEEP_POLL).await;
    }
    signal(&left(proc_root), rustix::process::Signal::KILL);
}
