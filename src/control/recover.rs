//! `niri-computer-use recover`, the human-only way to clear the input-dirty marker
//! (plan §11). It takes the lease so no server acts meanwhile, releases the keys and
//! pointer buttons the marker names, asks the human to check that nothing is held, and
//! clears the marker only after an explicit `yes`. Anything uncertain leaves the marker in
//! place.

use super::lease::{Lease, Refused};
use super::marker::{self, Found, Marker};
use super::runtime::RuntimeDir;
use crate::niri::pointer::{Pointer, Step};
use crate::{Env, cli, niri};

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
    match &snapshot.found {
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
            buttons, output, ..
        }) if !buttons.is_empty() => {
            release_buttons(env, buttons, output.as_deref()).await;
        }
        // Nothing named to release: the human's check below is all there is.
        Found::Marker(_) | Found::Unreadable { .. } => {}
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

async fn release_keyboard(env: &Env, marked: &marker::Native) -> Result<(), String> {
    let line = match send_key_releases(env, marked).await? {
        Layout::Active(group) => format!(
            "Sent native key releases and zero synthetic modifiers from a fresh virtual keyboard, in niri's active layout (index {group})."
        ),
        Layout::Marked(why) => format!(
            "Sent native key releases and zero synthetic modifiers from a fresh virtual keyboard. {why}; the focused application may show another layout until the next key."
        ),
    };
    cli::say(&line);
    Ok(())
}

/// The layout a fresh keyboard's zero modifiers went out in.
pub(super) enum Layout {
    /// The one niri has active, by its index.
    Active(u32),
    /// The marker's, from the call's start, since niri didn't say: why.
    Marked(String),
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
        Err(error) => Layout::Marked(format!(
            "niri didn't say which layout is active ({}), so they went out in layout index {}, where the call started",
            error.detail, marked.group
        )),
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
