//! `niri-computer-use recover`, the human-only way to clear the input-dirty marker
//! (plan §11). It takes the lease so no server acts meanwhile, ends the input child the
//! marker names, releases the pointer buttons it names, asks the human to check that
//! nothing is held, and clears the marker only after an explicit `yes`. Anything uncertain
//! leaves the marker in place.

use std::path::Path;
use std::time::Duration;

use rustix::process::{Pid, Signal, getuid, kill_process, kill_process_group};

use super::lease::{Lease, Refused};
use super::marker::{self, Found, Marker};
use super::procs;
use super::runtime::{INPUT_DIRTY, RuntimeDir};
use crate::niri::pointer::{Pointer, Step};
use crate::{Env, cli, niri};

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
    let root = Path::new(PROC);
    match &found {
        Found::Marker(Marker {
            keyboard: Some(keyboard),
            buttons,
            output,
            ..
        }) => {
            release_keyboard(env, keyboard).await?;
            if !buttons.is_empty() {
                release_buttons(env, buttons, output.as_deref()).await;
            }
        }
        Found::Marker(Marker {
            child: Some(child), ..
        }) => end_child(root, child.pid, child.start_time).await?,
        // The pointer runs in the server itself: there is no child to end.
        Found::Marker(Marker {
            buttons, output, ..
        }) if !buttons.is_empty() => {
            release_buttons(env, buttons, output.as_deref()).await;
        }
        Found::Marker(_) | Found::Unreadable { .. } => {
            end_wtype(root, getuid().as_raw(), cli::confirm).await?;
        }
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

async fn release_keyboard(env: &Env, marked: &marker::Native) -> Result<(), String> {
    if marked.codes.len() > 1000
        || marked.codes.iter().any(|code| *code > 767)
        || marked.group >= 32
    {
        return Err("invalid native keycodes or layout in the marker; the marker stays".into());
    }
    let display = env
        .wayland_socket()
        .ok_or("WAYLAND_DISPLAY or XDG_RUNTIME_DIR is not set")?;
    let pid = niri::pid(env.niri_socket.as_deref())
        .await
        .map_err(|error| error.detail)?;
    let mut keyboard = niri::keyboard::Keyboard::bind(&display, pid)
        .await
        .map_err(|error| error.detail)?;
    keyboard
        .release(&marked.codes, marked.group)
        .await
        .map_err(|error| error.detail)?;
    cli::say(
        "Sent native key releases and zero synthetic modifiers from a fresh virtual keyboard.",
    );
    Ok(())
}

/// Sends the release of each of `buttons` from a fresh virtual pointer, which clears a
/// button an ended pointer left pressed (M0, C8), or asks the human to.
async fn release_buttons(env: &Env, buttons: &[u32], output: Option<&str>) {
    match send_releases(env, buttons, output).await {
        Ok(()) => cli::say(&format!(
            "Sent the release of pointer buttons {buttons:?} from a fresh virtual pointer."
        )),
        Err(error) => cli::say(&format!(
            "The marker says pointer buttons {buttons:?} may be held, and their release couldn't be sent ({error}): press and release each of them once in an empty area."
        )),
    }
}

/// Binds the pointer to the marker's output if it is still enabled, else to the first
/// enabled one: a release needs no position.
async fn send_releases(env: &Env, buttons: &[u32], marked: Option<&str>) -> Result<(), String> {
    let socket = env.niri_socket.as_deref();
    let outputs = niri::outputs(socket).await.map_err(|error| error.detail)?;
    let mut enabled = outputs.values().filter(|output| output.logical.is_some());
    let output = enabled
        .clone()
        .find(|output| Some(output.name.as_str()) == marked)
        .or_else(|| enabled.next())
        .ok_or("niri has no enabled output")?;
    let display = env
        .wayland_socket()
        .ok_or("WAYLAND_DISPLAY or XDG_RUNTIME_DIR is not set")?;
    let pid = niri::pid(socket).await.map_err(|error| error.detail)?;
    let mut pointer = Pointer::bind(&display, pid, &output.name)
        .await
        .map_err(|error| error.detail)?;
    for &button in buttons {
        pointer
            .send(Step::Release(button))
            .map_err(|error| error.detail)?;
    }
    pointer.sync().await.map_err(|error| error.detail)
}

/// Kills the child, with its process group when it leads one, if the child is still the
/// process the marker names, and waits for it to exit.
async fn end_child(root: &Path, pid: u32, start_time: u64) -> Result<(), String> {
    let Some(stat) =
        procs::stat(root, pid).filter(|stat| stat.start_time == start_time && !stat.exited())
    else {
        cli::say(&format!("The input child, PID {pid}, has already exited."));
        return Ok(());
    };
    let target = to_pid(pid)?;
    let killed = if stat.group == pid {
        cli::say(&format!(
            "Ending the input child, PID {pid}, and its process group."
        ));
        kill_process_group(target, Signal::KILL)
    } else {
        cli::say(&format!(
            "Ending the input child, PID {pid}. It doesn't lead a process group, so only it is ended."
        ));
        kill_process(target, Signal::KILL)
    };
    gone_is_fine(killed).map_err(|error| format!("kill {pid}: {error}"))?;
    wait_for_exit(root, &[(pid, start_time)]).await
}

/// With no child PID recorded, offers to end the user's `wtype` processes. Each is pinned
/// by its start time when listed, and ended after the answer only if it is still that
/// process.
async fn end_wtype(
    root: &Path,
    uid: u32,
    confirm: impl FnOnce(&str) -> bool,
) -> Result<(), String> {
    let listed: Vec<(u32, u64)> = procs::named(root, "wtype", uid)
        .into_iter()
        .filter_map(|pid| Some((pid, procs::stat(root, pid)?.start_time)))
        .collect();
    if listed.is_empty() {
        cli::say("The marker names no input child, and no wtype process is running.");
        return Ok(());
    }
    let pids: Vec<u32> = listed.iter().map(|(pid, _)| *pid).collect();
    cli::say(&format!(
        "The marker names no input child. Your wtype processes: {pids:?}."
    ));
    if !confirm("End them?") {
        return Err("wtype processes left running; the marker stays".to_owned());
    }
    for &(pid, start_time) in &listed {
        if procs::alive_as(root, pid, start_time) {
            gone_is_fine(kill_process(to_pid(pid)?, Signal::KILL))
                .map_err(|error| format!("kill {pid}: {error}"))?;
        }
    }
    wait_for_exit(root, &listed).await
}

/// A process that is already gone is what killing it was for.
const fn gone_is_fine(result: rustix::io::Result<()>) -> rustix::io::Result<()> {
    match result {
        Err(rustix::io::Errno::SRCH) => Ok(()),
        other => other,
    }
}

async fn wait_for_exit(root: &Path, processes: &[(u32, u64)]) -> Result<(), String> {
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

#[cfg(test)]
mod tests {
    use std::process::Stdio;

    use super::*;

    /// A `sleep` this test owns, in a process group of its own when `own_group`.
    fn sleeper(own_group: bool) -> (tokio::process::Child, u32) {
        let mut command = command();
        command
            .args(["-c", "exec sleep 60"])
            .stdin(Stdio::null())
            .kill_on_drop(true);
        if own_group {
            command.process_group(0);
        }
        let child = command.spawn().unwrap();
        let pid = child.id().unwrap();
        (child, pid)
    }

    #[expect(
        clippy::disallowed_methods,
        reason = "the test starts the processes recover is pointed at"
    )]
    fn command() -> tokio::process::Command {
        tokio::process::Command::new("sh")
    }

    /// Makes `pid` appear in a fake `/proc` as a `wtype` process, its state and user taken
    /// live from the real one.
    fn as_wtype(root: &Path, pid: u32, real: u32) {
        let dir = root.join(pid.to_string());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("comm"), "wtype\n").unwrap();
        for file in ["stat", "status"] {
            std::fs::remove_file(dir.join(file)).ok();
            std::os::unix::fs::symlink(format!("/proc/{real}/{file}"), dir.join(file)).unwrap();
        }
    }

    fn uid() -> u32 {
        getuid().as_raw()
    }

    #[tokio::test]
    async fn ends_the_listed_wtype_processes_after_a_yes() {
        let root = crate::test_support::fresh_dir("recover-wtype");
        let (_child, pid) = sleeper(true);
        as_wtype(&root, pid, pid);
        end_wtype(&root, uid(), |_| true).await.unwrap();
        assert!(!procs::alive_as(
            Path::new(PROC),
            pid,
            procs::stat(Path::new(PROC), pid).map_or(0, |stat| stat.start_time)
        ));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn without_a_yes_nothing_is_ended() {
        let root = crate::test_support::fresh_dir("recover-wtype-no");
        let (_child, pid) = sleeper(true);
        as_wtype(&root, pid, pid);
        assert!(end_wtype(&root, uid(), |_| false).await.is_err());
        let stat = procs::stat(Path::new(PROC), pid).unwrap();
        assert!(!stat.exited());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn a_process_that_changed_during_the_question_is_left_alone() {
        let root = crate::test_support::fresh_dir("recover-wtype-reused");
        let (_listed, pid) = sleeper(true);
        // Start times count clock ticks of 10 ms; a reused PID always starts later.
        tokio::time::sleep(Duration::from_millis(50)).await;
        let (_other, other) = sleeper(true);
        as_wtype(&root, pid, pid);
        // While the human reads the list, the PID comes to name another process.
        end_wtype(&root, uid(), |_| {
            as_wtype(&root, pid, other);
            true
        })
        .await
        .unwrap();
        assert!(!procs::stat(Path::new(PROC), other).unwrap().exited());
        assert!(!procs::stat(Path::new(PROC), pid).unwrap().exited());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn a_child_that_leads_no_group_is_ended_alone() {
        // Shares this test's process group: killing the group would kill the test.
        let (_child, pid) = sleeper(false);
        let start = procs::stat(Path::new(PROC), pid).unwrap().start_time;
        end_child(Path::new(PROC), pid, start).await.unwrap();
        // The child is a zombie until reaped, which counts as ended.
        assert!(!procs::alive_as(Path::new(PROC), pid, start));
    }
}
