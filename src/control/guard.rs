//! `niri-computer-use guard <server-pid>`, the crash guardian. `serve` starts it through
//! the runner with a pipe as its stdin, so it reads end of file when its server exits,
//! however it exits; SIGKILL runs no `Drop`, and niri releases nothing when a virtual
//! device goes. It then reads the input-dirty marker. Only a marker its own server wrote,
//! naming native keycodes or pointer buttons, makes it send anything: the releases, zero
//! modifiers and the compositor's keymap, from fresh devices whose peer must be the niri
//! serving `NIRI_SOCKET`, under the lease and within one deadline. The marker stays, with
//! the time of the releases, until a human runs `recover`.

use std::time::Duration;

use tokio::time::Instant;

use super::lease::{Lease, Refused};
use super::marker::{self, Found, Marker};
use super::recover;
use super::runtime::RuntimeDir;
use crate::Env;

/// Everything after the server's end: the lease, niri's PID, binding and both releases.
const DEADLINE: Duration = Duration::from_secs(10);
/// How long the lease may stay held by someone else, such as a server refused by the
/// marker or a human's `recover`, which sends the same releases.
const LEASE_WAIT: Duration = Duration::from_secs(2);
const LEASE_RETRY: Duration = Duration::from_millis(20);

pub(crate) async fn run(env: &Env, server: u32) -> Result<(), String> {
    // Nothing else runs in this process: blocking until the server's end is the point.
    std::io::copy(&mut std::io::stdin().lock(), &mut std::io::sink())
        .map_err(|error| format!("watch server {server}: {error}"))?;
    // A server without a runtime directory can't have written a marker.
    let Ok(runtime) = RuntimeDir::of(env) else {
        return Ok(());
    };
    if releases(marker::read(&runtime).as_ref(), server).is_none() {
        return Ok(());
    }
    tokio::time::timeout(DEADLINE, release(env, &runtime, server))
        .await
        .map_err(|_| format!("releases for server {server} didn't finish within {DEADLINE:?}"))?
}

async fn release(env: &Env, runtime: &RuntimeDir, server: u32) -> Result<(), String> {
    let lease = lease(runtime).await?;
    // Read again under the lease: `recover` may have cleared it meanwhile.
    let Some(snapshot) = marker::snapshot(runtime) else {
        return Ok(());
    };
    let Some(marker) = releases(Some(&snapshot.found), server) else {
        return Ok(());
    };
    if let Some(keyboard) = &marker.keyboard {
        recover::send_key_releases(env, keyboard).await?;
    }
    if !marker.buttons.is_empty() {
        recover::send_releases(env, &marker.buttons, marker.output.as_deref()).await?;
    }
    snapshot
        .note_released(runtime)
        .await
        .map_err(|error| format!("note the releases in the input-dirty marker: {error}"))?;
    drop(lease);
    Ok(())
}

async fn lease(runtime: &RuntimeDir) -> Result<Lease, String> {
    let label = format!("guard/{}", std::process::id());
    let until = Instant::now() + LEASE_WAIT;
    loop {
        match Lease::acquire(runtime, &label) {
            Ok(lease) => return Ok(lease),
            Err(Refused::Io(detail)) => return Err(detail),
            Err(Refused::Held(holder)) if Instant::now() >= until => {
                return Err(format!(
                    "the lease stayed held ({}); recover sends the releases",
                    holder.map_or_else(|| "unknown holder".to_owned(), |holder| holder.label)
                ));
            }
            Err(Refused::Held(_)) => tokio::time::sleep(LEASE_RETRY).await,
        }
    }
}

/// The marker whose input the guardian of `server` releases: one `server` wrote, naming
/// native keycodes or pointer buttons, not yet released. A `wtype` child outlives the
/// server and finishes by itself, and an unreadable marker is the human's to inspect.
fn releases(found: Option<&Found>, server: u32) -> Option<&Marker> {
    match found? {
        Found::Marker(marker)
            if marker.server_pid == server
                && marker.released.is_none()
                && (marker.keyboard.is_some() || !marker.buttons.is_empty()) =>
        {
            Some(marker)
        }
        Found::Marker(_) | Found::Unreadable { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::marker::{Child, Native};

    fn marker(server: u32) -> Marker {
        let mut marker = Marker::pending("type_text", Vec::new());
        marker.server_pid = server;
        marker
    }

    #[test]
    fn releases_only_native_keys_or_buttons_its_own_server_left() {
        let mut keys = marker(7);
        keys.keyboard = Some(Native {
            codes: vec![38],
            group: 0,
        });
        let found = Found::Marker(keys.clone());
        assert_eq!(releases(Some(&found), 7), Some(&keys));
        assert_eq!(releases(Some(&found), 8), None);
        let buttons = Found::Marker(Marker {
            server_pid: 7,
            ..Marker::pending("drag", vec![272])
        });
        assert!(releases(Some(&buttons), 7).is_some());
    }

    #[test]
    fn sends_nothing_without_a_marker_that_names_input() {
        assert_eq!(releases(None, 7), None);
        let unreadable = Found::Unreadable {
            error: "parse".into(),
        };
        assert_eq!(releases(Some(&unreadable), 7), None);
        // A wtype child types to the end by itself; recover handles its marker.
        let mut wtype = marker(7);
        wtype.child = Some(Child {
            pid: 9,
            start_time: 5,
        });
        assert_eq!(releases(Some(&Found::Marker(wtype)), 7), None);
        // Released once already: the human's `recover` is what remains.
        let mut done = marker(7);
        done.keyboard = Some(Native {
            codes: Vec::new(),
            group: 0,
        });
        done.released = Some("t".into());
        assert_eq!(releases(Some(&Found::Marker(done)), 7), None);
    }
}
