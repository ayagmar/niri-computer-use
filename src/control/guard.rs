//! `niri-computer-use guard <server-pid>`, the crash guardian. `serve` starts it through
//! the runner with a pipe as its stdin, so it reads end of file when its server exits,
//! however it exits; SIGKILL runs no `Drop`, and niri releases nothing when a virtual
//! device goes. It then reads the input-dirty marker. Only a marker its own server wrote,
//! naming native keycodes or pointer buttons, makes it send anything: the releases, zero
//! modifiers, in the layout niri has active, and the compositor's keymap, from fresh
//! devices whose peer must be the niri serving `NIRI_SOCKET`, under the lease and within
//! one deadline. The marker stays, with
//! the time of the releases, until a human runs `recover`.
//!
//! At its start, before it waits, it does the same for a marker a dead server left and
//! nothing released: that server's own guardian died with it, as when a signal or the OOM
//! killer ends a whole service, or couldn't finish. So the next server to start releases it.

use std::path::Path;
use std::time::Duration;

use tokio::time::Instant;

use super::lease::{Lease, Refused};
use super::marker::{self, Found, Marker};
use super::procs;
use super::recover;
use super::runtime::RuntimeDir;
use crate::{Env, cli};

const PROC: &str = "/proc";

/// Everything after the server's end: the lease, niri's PID, binding and both releases.
const DEADLINE: Duration = Duration::from_secs(10);
/// The key releases' share of it, which leaves the button releases time after the lease's
/// wait even when niri stalls the keyboard.
const KEYS_DEADLINE: Duration = Duration::from_secs(4);
/// How long the lease may stay held by someone else, such as a server refused by the
/// marker or a human's `recover`, which sends the same releases.
const LEASE_WAIT: Duration = Duration::from_secs(2);
const LEASE_RETRY: Duration = Duration::from_millis(20);

pub(crate) async fn run(env: &Env, server: u32) -> Result<(), String> {
    if let Ok(runtime) = RuntimeDir::of(env)
        && let Err(error) = left_behind(env, &runtime, server, Path::new(PROC)).await
    {
        cli::print_error(&format!("the input a dead server left held: {error}"));
    }
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

/// Releases what the marker names when the server that wrote it isn't `server` and no
/// longer runs, as `proc_root` tells, and nothing released it yet, within the deadline.
async fn left_behind(
    env: &Env,
    runtime: &RuntimeDir,
    server: u32,
    proc_root: &Path,
) -> Result<(), String> {
    let Some(dead) = orphan(marker::read(runtime).as_ref(), server, proc_root) else {
        return Ok(());
    };
    tokio::time::timeout(DEADLINE, release(env, runtime, dead))
        .await
        .map_err(|_| format!("releases for server {dead} didn't finish within {DEADLINE:?}"))?
}

/// The dead server whose input `found` names and nothing released: one other than `server`
/// that `proc_root` lists as exited or not at all. The marker records no start time, so a
/// PID reused since reads as running, and its input waits for `recover`.
fn orphan(found: Option<&Found>, server: u32, proc_root: &Path) -> Option<u32> {
    let Some(Found::Marker(marker)) = found else {
        return None;
    };
    let writer = marker.server_pid;
    let dead = procs::stat(proc_root, writer).is_none_or(procs::Stat::exited);
    (writer != server && dead && releases(found, writer).is_some()).then_some(writer)
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
    send(env, marker).await?;
    snapshot
        .note_released(runtime)
        .await
        .map_err(|error| format!("note the releases in the input-dirty marker: {error}"))?;
    drop(lease);
    Ok(())
}

/// Sends the key releases and the button releases `marker` names, each whatever became
/// of the other, the keys within `KEYS_DEADLINE` so the buttons keep time of their own.
/// Fails with every failure when either does.
async fn send(env: &Env, marker: &Marker) -> Result<(), String> {
    let mut failures = Vec::new();
    if let Some(keyboard) = &marker.keyboard {
        match tokio::time::timeout(KEYS_DEADLINE, recover::send_key_releases(env, keyboard)).await {
            Ok(Ok(recover::Layout::Active(_))) => {}
            Ok(Ok(recover::Layout::Marked(why))) => {
                cli::print_error(&format!("the native key releases: {why}"));
            }
            Ok(Err(error)) => failures.push(format!("the native key releases: {error}")),
            Err(_) => failures.push(format!(
                "the native key releases didn't finish within {KEYS_DEADLINE:?}"
            )),
        }
    }
    if !marker.buttons.is_empty()
        && let Err(error) =
            recover::send_releases(env, &marker.buttons, marker.output.as_deref()).await
    {
        failures.push(format!("the button releases: {error}"));
    }
    if failures.is_empty() {
        return Ok(());
    }
    Err(failures.join("; "))
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
    use crate::test_support::{fresh_dir, wayland::Compositor};

    const KEYMAP: &str = include_str!("../../tests/fixtures/us-de.xkb");

    /// Writes `/proc/<pid>/stat` under `proc_root` for a process in `state`.
    fn process(proc_root: &Path, pid: u32, state: char) {
        let process = proc_root.join(pid.to_string());
        std::fs::create_dir_all(&process).unwrap();
        let stat = format!(
            "{pid} (niri-computer-u) {state} 1 {pid} {pid} 0 -1 0 0 0 0 0 0 0 0 0 20 0 1 0 98765 0 0"
        );
        std::fs::write(process.join("stat"), stat).unwrap();
    }

    #[test]
    fn a_dead_servers_unreleased_input_is_left_behind() {
        let proc_root = fresh_dir("guard-orphan-proc");
        let mut keys = marker(7);
        keys.keyboard = Some(Native {
            codes: vec![38],
            group: 0,
        });
        let found = Found::Marker(keys.clone());
        assert_eq!(orphan(Some(&found), 8, &proc_root), Some(7), "not listed");
        process(&proc_root, 7, 'Z');
        assert_eq!(orphan(Some(&found), 8, &proc_root), Some(7), "a zombie");
        process(&proc_root, 7, 'S');
        assert_eq!(orphan(Some(&found), 8, &proc_root), None, "it runs");
        std::fs::remove_dir_all(proc_root.join("7")).unwrap();
        assert_eq!(
            orphan(Some(&found), 7, &proc_root),
            None,
            "its own server's"
        );
        keys.released = Some("t".into());
        assert_eq!(orphan(Some(&Found::Marker(keys)), 8, &proc_root), None);
        assert_eq!(orphan(None, 8, &proc_root), None);
        std::fs::remove_dir_all(proc_root).unwrap();
    }

    /// A server killed with its guardian, mid-text in an extended keymap and with a
    /// button held: the next guardian to start releases the key and the button, zeroes
    /// the modifiers in niri's active layout and puts the compositor's keymap back, then
    /// notes the releases in the marker, which stays for `recover`.
    #[tokio::test]
    async fn the_next_guardian_releases_what_a_dead_server_left_held() {
        let dir = fresh_dir("guard-left-behind");
        let proc_root = fresh_dir("guard-left-behind-proc");
        let niri = Compositor::start(&dir, KEYMAP);
        let env = niri.env(&dir);
        let runtime = RuntimeDir::of(&env).unwrap();
        runtime.create().unwrap();
        // What the dead server sent before it died: an extension and a key pressed. Its
        // connection closing releases nothing in niri.
        let mut keyboard =
            crate::niri::keyboard::Keyboard::bind(niri.display(), std::process::id())
                .await
                .unwrap();
        keyboard
            .extend(&KEYMAP.replace(
                "    modifier_map Shift",
                "    key <I60> { [eacute] };\n    modifier_map Shift",
            ))
            .unwrap();
        keyboard.key(52, true).unwrap();
        keyboard.sync().await.unwrap();
        drop(keyboard);
        std::fs::write(
            runtime.path().join(super::super::runtime::INPUT_DIRTY),
            format!(
                r#"{{"operation":"drag","phase":"pending","server_pid":7,"since":"t","buttons":[272],"output":"{}","keyboard":{{"codes":[52],"group":0}}}}"#,
                crate::test_support::wayland::OUTPUT
            ),
        )
        .unwrap();
        niri.switch(1).await;
        left_behind(&env, &runtime, std::process::id(), &proc_root)
            .await
            .unwrap();
        let seen = niri.seen();
        assert!(seen.held_keys().is_empty(), "{:?}", seen.keys);
        assert_eq!(seen.held_buttons(), Vec::<u32>::new());
        assert!(seen.buttons.contains(&(272, false)));
        assert_eq!(seen.active, seen.compositor);
        assert_eq!(seen.last_modifiers(), Some((0, 1)));
        let Some(Found::Marker(left)) = marker::read(&runtime) else {
            panic!("the marker went")
        };
        assert!(left.released.is_some());
        std::fs::remove_dir_all(dir).unwrap();
        std::fs::remove_dir_all(proc_root).unwrap();
    }

    /// A dead server's marker whose keys can't be released, here because its codes are
    /// invalid, still gets its button released; the failure names the keys, and without
    /// all its releases the marker isn't noted released.
    #[tokio::test]
    async fn a_key_release_that_fails_still_lets_the_button_go() {
        let dir = fresh_dir("guard-mixed");
        let proc_root = fresh_dir("guard-mixed-proc");
        let niri = Compositor::start(&dir, KEYMAP);
        let env = niri.env(&dir);
        let runtime = RuntimeDir::of(&env).unwrap();
        runtime.create().unwrap();
        std::fs::write(
            runtime.path().join(super::super::runtime::INPUT_DIRTY),
            format!(
                r#"{{"operation":"drag","phase":"pending","server_pid":7,"since":"t","buttons":[272],"output":"{}","keyboard":{{"codes":[800],"group":0}}}}"#,
                crate::test_support::wayland::OUTPUT
            ),
        )
        .unwrap();
        let error = left_behind(&env, &runtime, std::process::id(), &proc_root)
            .await
            .unwrap_err();
        assert!(error.contains("the native key releases"), "{error}");
        assert!(!error.contains("button"), "{error}");
        assert_eq!(niri.seen().buttons, [(272, false)]);
        let Some(Found::Marker(left)) = marker::read(&runtime) else {
            panic!("the marker went")
        };
        assert_eq!(left.released, None);
        std::fs::remove_dir_all(dir).unwrap();
        std::fs::remove_dir_all(proc_root).unwrap();
    }

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
