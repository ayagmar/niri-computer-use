//! `niri-computer-use recover`, the human-only way to clear the input-dirty marker
//! (plan §11). It takes the lease so no server acts meanwhile, releases the keys and
//! pointer buttons the marker names, ends the input child a 0.1.0 server's marker names,
//! asks the human to check that nothing is held, and clears the marker only after an
//! explicit `yes`. Anything uncertain leaves the marker in place.

use std::path::Path;
use std::time::Duration;

use rustix::process::{Pid, Signal, kill_process, kill_process_group};

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
/// A server killed mid-input leaves its `wtype` child typing its part of the text.
async fn end_child(root: &Path, child: Child) -> Result<(), String> {
    let pid = child.pid;
    let Some(stat) = running(root, child) else {
        cli::say(&format!("The input child, PID {pid}, has already exited."));
        return Ok(());
    };
    let target = i32::try_from(pid)
        .ok()
        .and_then(Pid::from_raw)
        .ok_or_else(|| format!("{pid} isn't a process ID; the marker stays"))?;
    let killed = if stat.group == pid {
        cli::say(&format!(
            "Ending the input child, PID {pid}, and its process group."
        ));
        kill_process_group(target, Signal::KILL)
    } else {
        cli::say(&format!(
            "Ending the input child, PID {pid}. It leads no process group, so only it is ended."
        ));
        kill_process(target, Signal::KILL)
    };
    // A process already gone is what killing it was for.
    match killed {
        Ok(()) | Err(rustix::io::Errno::SRCH) => {}
        Err(error) => return Err(format!("kill {pid}: {error}; the marker stays")),
    }
    let until = tokio::time::Instant::now() + EXIT_DEADLINE;
    while running(root, child).is_some() {
        if tokio::time::Instant::now() >= until {
            return Err(format!(
                "the input child, PID {pid}, didn't exit within {EXIT_DEADLINE:?}; the marker stays"
            ));
        }
        tokio::time::sleep(EXIT_POLL).await;
    }
    Ok(())
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

    #[tokio::test]
    async fn ends_the_input_child_a_0_1_0_marker_names() {
        let (_group, leader) = sleeper(true);
        end_child(Path::new(PROC), leader).await.unwrap();
        assert!(ended(leader));
        // Shares this test's process group, which killing the group would end too.
        let (_alone, member) = sleeper(false);
        end_child(Path::new(PROC), member).await.unwrap();
        assert!(ended(member));
    }

    #[tokio::test]
    async fn a_process_that_only_reuses_the_pid_of_the_child_is_left_alone() {
        let (_other, other) = sleeper(true);
        let reused = Child {
            start_time: other.start_time - 1,
            ..other
        };
        end_child(Path::new(PROC), reused).await.unwrap();
        assert!(!ended(other));
    }
}
