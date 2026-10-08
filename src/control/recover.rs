//! `niri-computer-use recover`, the human-only way to clear the input-dirty marker
//! (plan §11). It takes the lease so no server acts meanwhile, ends the input child the
//! marker names, asks the human to check that nothing is held, and clears the marker only
//! after an explicit `yes`. Anything uncertain leaves the marker in place.

use std::path::Path;
use std::time::Duration;

use rustix::process::{Pid, Signal, getuid, kill_process, kill_process_group};

use super::lease::{Lease, Refused};
use super::marker::{self, Found, Marker};
use super::procs;
use super::runtime::{INPUT_DIRTY, RuntimeDir};
use crate::{Env, cli};

const PROC: &str = "/proc";
/// How long a killed process gets to exit.
const EXIT_DEADLINE: Duration = Duration::from_secs(5);

const MANUAL_CHECK: &str = "Check that no input is held: press and release Shift, Ctrl, Alt and Super, \
click once in an empty area, and check the application the input went to. A physical key \
doesn't always release a key wtype pressed.";

pub(crate) async fn run(env: &Env) -> Result<(), String> {
    let runtime = RuntimeDir::of(env)?;
    let lease = Lease::acquire(&runtime, &format!("recover/{}", std::process::id())).map_err(
        |refused| match refused {
            Refused::Held(Some(holder)) => format!(
                "a server holds the lease: PID {} ({}); end that agent or its server first",
                holder.pid, holder.label
            ),
            Refused::Held(None) => "another process holds the lease".to_owned(),
            Refused::Io(detail) => detail,
        },
    )?;
    let Some(found) = marker::read(&runtime) else {
        cli::say("No input-dirty marker: nothing to recover.");
        return Ok(());
    };
    cli::say(&format!("Input-dirty marker: {}", found.summary()));
    match &found {
        Found::Marker(Marker {
            child: Some(child), ..
        }) => end_child(child.pid, child.start_time).await?,
        Found::Marker(_) | Found::Unreadable { .. } => end_wtype().await?,
    }
    if let Found::Marker(marker) = &found
        && !marker.buttons.is_empty()
    {
        cli::say(&format!(
            "The marker says pointer buttons {:?} may be held. This version can't send their release: press and release each of them once in an empty area.",
            marker.buttons
        ));
    }
    cli::say(MANUAL_CHECK);
    if !cli::confirm("Is all input released?") {
        return Err("not confirmed; the marker stays".to_owned());
    }
    let path = runtime.path().join(INPUT_DIRTY);
    std::fs::remove_file(&path).map_err(|error| format!("remove {}: {error}", path.display()))?;
    drop(lease);
    cli::say("Marker cleared. `niri-computer-use resume` clears the stop flag if it is set.");
    Ok(())
}

/// Kills the child's process group if the child is still the process the marker names,
/// and waits for it to exit.
async fn end_child(pid: u32, start_time: u64) -> Result<(), String> {
    let root = Path::new(PROC);
    if !procs::alive_as(root, pid, start_time) {
        cli::say(&format!("The input child, PID {pid}, has already exited."));
        return Ok(());
    }
    cli::say(&format!(
        "Ending the input child, PID {pid}, and its process group."
    ));
    let group = to_pid(pid)?;
    kill_process_group(group, Signal::KILL)
        .map_err(|error| format!("kill process group {pid}: {error}"))?;
    wait_for_exit(&[(pid, start_time)]).await
}

/// With no child PID recorded, offers to end the user's `wtype` processes.
async fn end_wtype() -> Result<(), String> {
    let root = Path::new(PROC);
    let pids = procs::named(root, "wtype", getuid().as_raw());
    if pids.is_empty() {
        cli::say("The marker names no input child, and no wtype process is running.");
        return Ok(());
    }
    cli::say(&format!(
        "The marker names no input child. Your wtype processes: {pids:?}."
    ));
    if !cli::confirm("End them?") {
        return Err("wtype processes left running; the marker stays".to_owned());
    }
    let mut ended = Vec::new();
    for pid in pids {
        let Some(stat) = procs::stat(root, pid) else {
            continue;
        };
        kill_process(to_pid(pid)?, Signal::KILL).map_err(|error| format!("kill {pid}: {error}"))?;
        ended.push((pid, stat.start_time));
    }
    wait_for_exit(&ended).await
}

async fn wait_for_exit(processes: &[(u32, u64)]) -> Result<(), String> {
    let root = Path::new(PROC);
    let started = tokio::time::Instant::now();
    while started.elapsed() < EXIT_DEADLINE {
        if !processes
            .iter()
            .any(|(pid, start)| procs::alive_as(root, *pid, *start))
        {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let pids: Vec<u32> = processes.iter().map(|(pid, _)| *pid).collect();
    Err(format!(
        "processes {pids:?} didn't exit within {EXIT_DEADLINE:?}; the marker stays"
    ))
}

fn to_pid(pid: u32) -> Result<Pid, String> {
    i32::try_from(pid)
        .ok()
        .and_then(Pid::from_raw)
        .ok_or_else(|| format!("{pid} isn't a process ID"))
}
