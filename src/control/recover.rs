//! `niri-computer-use recover`, the human-only way to clear the input-dirty marker
//! (plan §11). It takes the lease so no server acts meanwhile, releases the keys and
//! pointer buttons the marker names, ends the input child a 0.1.0 server's marker names,
//! asks the human to check that nothing is held, and clears the marker only after an
//! explicit `yes`. Anything uncertain leaves the marker in place.

use std::path::Path;
use std::time::Duration;

use std::os::fd::OwnedFd;

use rustix::process::{Pid, PidfdFlags, Signal, kill_process_group, pidfd_open, pidfd_send_signal};

use super::lease::{Lease, Refused};
use super::marker::{self, Child, Found, Marker};
use super::procs;
use super::runtime::RuntimeDir;
use crate::niri::pointer::{Pointer, Step};
use crate::{Env, cli, niri};

const PROC: &str = "/proc";
/// How long an ended input child gets to exit.
const EXIT_DEADLINE: Duration = Duration::from_secs(5);
const EXIT_POLL: Duration = Duration::from_millis(50);
/// How long an input child that leads its process group gets to stop before the group is
/// killed.
const STOP_DEADLINE: Duration = Duration::from_secs(1);
const STOP_POLL: Duration = Duration::from_millis(10);

const MANUAL_CHECK: &str = "Check that no input is held: press and release Shift, Ctrl, Alt and Super, \
click once in an empty area, and check the application the input went to.";

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
    let Some(snapshot) = marker::snapshot(&runtime) else {
        cli::say("No input-dirty marker: nothing to recover.");
        return Ok(());
    };
    cli::say(&format!("Input-dirty marker: {}", snapshot.found.summary()));
    if let Found::Marker(marker) = &snapshot.found {
        release(env, marker).await?;
    }
    cli::say(MANUAL_CHECK);
    if !cli::confirm("Is all input released?") {
        return Err("not confirmed; the marker stays".to_owned());
    }
    // Only the marker this run released: another input's, written meanwhile, stays.
    snapshot.clear(&runtime).await.map_err(|error| {
        format!("{error}; run `niri-computer-use recover` again for the marker there now")
    })?;
    drop(lease);
    cli::say("Marker cleared. `niri-computer-use resume` clears the stop flag if it is set.");
    Ok(())
}

/// Ends the input child the marker names, then releases the keys and pointer buttons it
/// names, or after a child only zeroes the modifiers. A marker naming none of them leaves
/// the human's check as all there is.
async fn release(env: &Env, marker: &Marker) -> Result<(), String> {
    if let Some(child) = marker.child {
        end_child(Path::new(PROC), child).await?;
    }
    match (&marker.keyboard, marker.child) {
        (Some(keyboard), _) => release_keyboard(env, keyboard).await?,
        (None, Some(_)) => zero_modifiers(env).await,
        (None, None) => {}
    }
    if !marker.buttons.is_empty() {
        release_buttons(env, &marker.buttons, marker.output.as_deref()).await;
    }
    Ok(())
}

/// Kills a 0.1.0 server's input child, with its process group when it leads one, if it is
/// still the process the marker names, and waits for it to exit, as 0.1.0's `recover` did.
/// A server killed mid-input leaves its `wtype` child typing its part of the text. The child
/// is pinned by a pidfd opened before its start time is checked, and signalled through it,
/// so a PID reused after the check names no process recover signals.
async fn end_child(root: &Path, child: Child) -> Result<(), String> {
    let pid = child.pid;
    let gone = || cli::say(&format!("The input child, PID {pid}, has already exited."));
    let Some(pidfd) = pin(pid)? else {
        gone();
        return Ok(());
    };
    let killed = match ending(root, child) {
        Ending::Init => {
            cli::say(
                "The marker names PID 1, init, as its input child, which it can't be, so no process is ended.",
            );
            return Ok(());
        }
        Ending::Gone => {
            gone();
            return Ok(());
        }
        Ending::Group => {
            cli::say(&format!(
                "Ending the input child, PID {pid}, and its process group."
            ));
            kill_group(root, child, &pidfd).await
        }
        Ending::Alone => {
            cli::say(&format!(
                "Ending the input child, PID {pid}. It leads no process group, so only it is ended."
            ));
            pidfd_send_signal(&pidfd, Signal::KILL)
        }
    };
    // A process already gone is what killing it was for.
    match killed {
        Ok(()) | Err(rustix::io::Errno::SRCH) => {}
        Err(error) => return Err(format!("kill {pid}: {error}; the marker stays")),
    }
    exited(root, child).await
}

/// A pidfd for process `pid`, or none when no process has that PID.
fn pin(pid: u32) -> Result<Option<OwnedFd>, String> {
    let Some(target) = i32::try_from(pid).ok().and_then(Pid::from_raw) else {
        return Ok(None);
    };
    match pidfd_open(target, PidfdFlags::empty()) {
        Ok(pidfd) => Ok(Some(pidfd)),
        Err(rustix::io::Errno::SRCH) => Ok(None),
        Err(error) => Err(format!("pidfd_open {pid}: {error}; the marker stays")),
    }
}

/// Kills the process group `child` leads, `pidfd` naming `child`. The leader is stopped
/// first, through `pidfd`: a stopped process can't exit until it is killed, so while it is
/// stopped its PID, which is the group's ID, can't come to name another process or group.
/// A leader not seen stopped in its group within `STOP_DEADLINE`, as in an uninterruptible
/// wait, is killed alone.
async fn kill_group(root: &Path, child: Child, pidfd: &OwnedFd) -> rustix::io::Result<()> {
    pidfd_send_signal(pidfd, Signal::STOP)?;
    let until = tokio::time::Instant::now() + STOP_DEADLINE;
    loop {
        let Some(stat) = running(root, child) else {
            return Ok(());
        };
        if stat.stopped() && stat.group == child.pid {
            break;
        }
        if tokio::time::Instant::now() >= until {
            cli::say(&format!(
                "It wasn't seen stopped in its process group within {STOP_DEADLINE:?}, so only it is ended."
            ));
            return pidfd_send_signal(pidfd, Signal::KILL);
        }
        tokio::time::sleep(STOP_POLL).await;
    }
    let group = i32::try_from(child.pid)
        .ok()
        .and_then(Pid::from_raw)
        .ok_or(rustix::io::Errno::SRCH)?;
    kill_process_group(group, Signal::KILL)
}

/// Waits until `child` has exited, up to `EXIT_DEADLINE`.
async fn exited(root: &Path, child: Child) -> Result<(), String> {
    let until = tokio::time::Instant::now() + EXIT_DEADLINE;
    while running(root, child).is_some() {
        if tokio::time::Instant::now() >= until {
            return Err(format!(
                "the input child, PID {}, didn't exit within {EXIT_DEADLINE:?}; the marker stays",
                child.pid
            ));
        }
        tokio::time::sleep(EXIT_POLL).await;
    }
    Ok(())
}

/// How `end_child` ends the child a marker names.
#[derive(Debug, PartialEq, Eq)]
enum Ending {
    /// PID 1 is init, never an input child. It leads its own group, and killing group 1,
    /// `kill(-1)`, would kill every process the user may signal.
    Init,
    /// It no longer runs as the process the marker names.
    Gone,
    /// It leads its own process group, which is ended with it.
    Group,
    /// It is in another process's group, so only it is ended.
    Alone,
}

/// How to end `child`, read from the `/proc` at `root`.
fn ending(root: &Path, child: Child) -> Ending {
    if child.pid == 1 {
        return Ending::Init;
    }
    match running(root, child) {
        None => Ending::Gone,
        Some(stat) if stat.group == child.pid => Ending::Group,
        Some(_) => Ending::Alone,
    }
}

/// The child, if it still runs as the process the marker names.
fn running(root: &Path, child: Child) -> Option<procs::Stat> {
    procs::stat(root, child.pid)
        .filter(|stat| stat.start_time == child.start_time && !stat.exited())
}

async fn release_keyboard(env: &Env, marked: &marker::Native) -> Result<(), String> {
    let line = match send_key_releases(env, marked).await? {
        Layout::Active(group) => format!(
            "Sent native key releases and zero synthetic modifiers from a fresh virtual keyboard, in niri's active layout (index {group})."
        ),
        Layout::Marked(detail) => format!(
            "Sent native key releases and zero synthetic modifiers from a fresh virtual keyboard. {}; the focused application may show another layout until the next key.",
            in_marked(&detail, marked.group)
        ),
    };
    cli::say(&line);
    Ok(())
}

/// Zeroes the modifiers from a fresh virtual keyboard after a 0.1.0 server's `wtype`
/// child, which pressed modifiers on a keyboard of its own that a physical key doesn't
/// always release. Its marker names no keys or layout. If they can't be sent, the human's
/// check is all there is, as for 0.1.0's own `recover`.
async fn zero_modifiers(env: &Env) {
    let none = marker::Native {
        codes: Vec::new(),
        group: 0,
    };
    let line = match send_key_releases(env, &none).await {
        Ok(Layout::Active(group)) => format!(
            "Sent zero modifiers from a fresh virtual keyboard, in niri's active layout (index {group}), for any wtype left held."
        ),
        Ok(Layout::Marked(detail)) => format!(
            "Sent zero modifiers from a fresh virtual keyboard, for any wtype left held. niri didn't say which layout is active ({detail}), so they went out in its first layout; the focused application may show that layout until the next key."
        ),
        Err(error) => format!(
            "Zero modifiers couldn't be sent from a fresh virtual keyboard ({error}). A physical key doesn't always release a key wtype pressed."
        ),
    };
    cli::say(&line);
}

/// The layout a fresh keyboard's zero modifiers went out in.
pub(super) enum Layout {
    /// The one niri has active, by its index.
    Active(u32),
    /// The marker's, from the call's start, since niri didn't say: its error.
    Marked(String),
}

/// Why the releases went out in the marker's layout `group`: niri's `detail`.
pub(super) fn in_marked(detail: &str, group: u32) -> String {
    format!(
        "niri didn't say which layout is active ({detail}), so they went out in layout index {group}, where the call started"
    )
}

/// Releases the marker's keycodes and zeroes the modifiers from a fresh virtual keyboard,
/// which carries the compositor's current map, so niri sends that map to clients again
/// with the releases. The modifiers go out in the layout niri has active, the user's,
/// or without niri's answer in the marker's. The display's peer must be the niri serving
/// `NIRI_SOCKET`.
pub(super) async fn send_key_releases(
    env: &Env,
    marked: &marker::Native,
) -> Result<Layout, String> {
    if marked.codes.len() > 1000
        || marked.codes.iter().any(|code| *code > 767)
        || marked.group >= 32
    {
        return Err("invalid native keycodes or layout in the marker; the marker stays".into());
    }
    let display = env.display.path().map_err(|error| error.detail.clone())?;
    let pid = niri::pid(&env.niri_socket)
        .await
        .map_err(|error| error.detail)?;
    let mut keyboard = niri::keyboard::Keyboard::bind(display, pid)
        .await
        .map_err(|error| error.detail)?;
    let layout = match niri::keyboard_group(&env.niri_socket).await {
        Ok(group) => Layout::Active(group),
        Err(error) => Layout::Marked(error.detail),
    };
    let group = match layout {
        Layout::Active(group) => group,
        Layout::Marked(_) => marked.group,
    };
    keyboard
        .release(&marked.codes, group)
        .await
        .map_err(|error| error.detail)?;
    Ok(layout)
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
pub(super) async fn send_releases(
    env: &Env,
    buttons: &[u32],
    marked: Option<&str>,
) -> Result<(), String> {
    let socket = &env.niri_socket;
    let outputs = niri::outputs(socket).await.map_err(|error| error.detail)?;
    let mut enabled = outputs.values().filter(|output| output.logical.is_some());
    let output = enabled
        .clone()
        .find(|output| Some(output.name.as_str()) == marked)
        .or_else(|| enabled.next())
        .ok_or("niri has no enabled output")?;
    let display = env.display.path().map_err(|error| error.detail.clone())?;
    let pid = niri::pid(socket).await.map_err(|error| error.detail)?;
    let mut pointer = Pointer::bind(display, pid, &output.name)
        .await
        .map_err(|error| error.detail)?;
    for &button in buttons {
        pointer
            .send(Step::Release(button))
            .map_err(|error| error.detail)?;
    }
    pointer.sync().await.map_err(|error| error.detail)
}

#[cfg(test)]
mod tests {
    use std::process::Stdio;

    use super::*;

    /// A `sleep` standing in for a 0.1.0 server's `wtype` child, in a process group of its
    /// own when `own_group`.
    fn sleeper(own_group: bool) -> (tokio::process::Child, Child) {
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
        let start_time = procs::stat(Path::new(PROC), pid).unwrap().start_time;
        (child, Child { pid, start_time })
    }

    /// A `sleep` leading its own process group, as `sleeper(true)`, and another `sleep` in
    /// that group, with a pidfd the test ends it through.
    async fn leader_with_member() -> (tokio::process::Child, Child, Child, OwnedFd) {
        let mut command = command();
        command
            .args(["-c", "sleep 60 & echo $!; exec sleep 60"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .kill_on_drop(true)
            .process_group(0);
        let mut leader = command.spawn().unwrap();
        let pid = leader.id().unwrap();
        let mut line = String::new();
        let stdout = leader.stdout.take().unwrap();
        tokio::io::AsyncBufReadExt::read_line(&mut tokio::io::BufReader::new(stdout), &mut line)
            .await
            .unwrap();
        let member: u32 = line.trim().parse().unwrap();
        let pidfd = pin(member).unwrap().unwrap();
        let stat = |process| procs::stat(Path::new(PROC), process).unwrap();
        assert_eq!(stat(member).group, pid);
        let child = |process| Child {
            pid: process,
            start_time: stat(process).start_time,
        };
        (leader, child(pid), child(member), pidfd)
    }

    #[expect(
        clippy::disallowed_methods,
        reason = "the test starts the processes recover is pointed at"
    )]
    fn command() -> tokio::process::Command {
        tokio::process::Command::new("sh")
    }

    fn ended(child: Child) -> bool {
        // An ended child is a zombie until reaped, which counts as ended.
        running(Path::new(PROC), child).is_none()
    }

    /// Whether `child` ends `within`: a killed process ends a moment after the signal.
    async fn ends(child: Child, within: Duration) -> bool {
        let until = tokio::time::Instant::now() + within;
        while !ended(child) {
            if tokio::time::Instant::now() >= until {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        true
    }

    #[tokio::test]
    async fn ends_the_input_child_a_0_1_0_marker_names() {
        let (_group, leader, member, _member) = leader_with_member().await;
        end_child(Path::new(PROC), leader).await.unwrap();
        assert!(ended(leader));
        assert!(
            ends(member, EXIT_DEADLINE).await,
            "the rest of its group was left running"
        );
        // Shares this test's process group, which killing the group would end too.
        let (_alone, alone) = sleeper(false);
        end_child(Path::new(PROC), alone).await.unwrap();
        assert!(ended(alone));
    }

    #[tokio::test]
    async fn a_process_that_only_reuses_the_pid_of_the_child_is_left_alone() {
        let (_other, other) = sleeper(true);
        let reused = Child {
            start_time: other.start_time - 1,
            ..other
        };
        end_child(Path::new(PROC), reused).await.unwrap();
        assert!(!ends(other, Duration::from_millis(500)).await);
    }

    /// The bug: once the start time was checked, the group was killed by its number. The
    /// leader could exit and be reaped meanwhile, and the number then name a group recover
    /// never checked. Here the leader is reaped right after the check, and the rest of its
    /// group stands for such a group: it is left alone.
    #[tokio::test]
    async fn a_group_whose_leader_exited_after_the_check_isnt_killed() {
        let (mut group, leader, member, member_pidfd) = leader_with_member().await;
        let pidfd = pin(leader.pid).unwrap().unwrap();
        assert_eq!(ending(Path::new(PROC), leader), Ending::Group);
        group.start_kill().unwrap();
        group.wait().await.unwrap();
        let killed = kill_group(Path::new(PROC), leader, &pidfd).await;
        assert!(
            matches!(killed, Ok(()) | Err(rustix::io::Errno::SRCH)),
            "{killed:?}"
        );
        assert!(
            !ends(member, Duration::from_millis(500)).await,
            "a group recover didn't check was killed"
        );
        pidfd_send_signal(&member_pidfd, Signal::KILL).unwrap();
    }

    /// The bug: a marker naming PID 1 at init's start time took init for the child, which
    /// leads its own group, and killing group 1 is `kill(-1)`, every process the user may
    /// signal. A protocol test's marker named PID 1 at start time 1, which init has on a
    /// machine that starts it within the first tick of boot. Only `ending` is called, so a
    /// regression here fails without sending a signal.
    #[test]
    fn init_is_never_taken_for_the_input_child() {
        let root = crate::test_support::fresh_dir("recover-init");
        for pid in [1, 7] {
            let process = root.join(pid.to_string());
            std::fs::create_dir(&process).unwrap();
            let stat =
                format!("{pid} (init) S 0 {pid} {pid} 0 -1 0 0 0 0 0 0 0 0 0 20 0 1 0 1 0 0");
            std::fs::write(process.join("stat"), stat).unwrap();
        }
        let named = |pid| Child { pid, start_time: 1 };
        assert_eq!(ending(&root, named(1)), Ending::Init);
        // The same stat under another PID is a child leading its own group.
        assert_eq!(ending(&root, named(7)), Ending::Group);
        std::fs::remove_dir_all(root).unwrap();
    }
}
