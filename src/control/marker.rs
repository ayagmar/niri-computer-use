//! The input-dirty marker, `<runtime dir>/input-dirty`: written before any input is
//! dispatched and removed only once the input is known to be released (plan §11). While
//! it exists, no server takes the lease and `resume` refuses; only `recover` and the input
//! that wrote it clear it.
//!
//! Every change to the file, by a server, the crash guardian or `recover`, happens under
//! `input-dirty.lock`, a lock file that stays, and only after checking under that lock
//! that the marker is still the one the change is for: a server's own, by its `owner`, or
//! the one the guardian or `recover` read, byte for byte. So a late change from one input
//! never replaces or removes another's marker. The lock is held only for the check and
//! the change, never across input or a human's answer; when it can't be taken within
//! `LOCK_WAIT`, the change fails and the marker stays.

use std::fs::{File, OpenOptions, TryLockError};
use std::io::Write as _;
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tokio::time::Instant;

use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};

use super::runtime::{INPUT_DIRTY, RuntimeDir};

/// Taken around every change to the marker.
const LOCK: &str = "input-dirty.lock";
/// How long a change waits for another one's lock, which is held only for file operations.
const LOCK_WAIT: Duration = Duration::from_millis(500);
const LOCK_RETRY: Duration = Duration::from_millis(5);

/// The marker's content, one JSON object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Marker {
    /// The tool whose input may be stuck, such as `type_text`.
    pub(crate) operation: String,
    pub(crate) phase: Phase,
    /// The server that wrote it.
    pub(crate) server_pid: u32,
    pub(crate) since: String,
    /// The input child, once its PID is known (`running`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) child: Option<Child>,
    /// Pointer buttons pressed and not yet released, as evdev codes such as 272.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) buttons: Vec<u32>,
    /// The output the pointer that pressed them is bound to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) output: Option<String>,
    /// Native protocol evdev keycodes that may need release, never typed text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) keyboard: Option<Native>,
    /// When the crash guardian sent the releases, after the server that wrote the marker
    /// died. The marker still blocks until `recover`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) released: Option<String>,
    /// Which write this is, unique to the input that wrote it, so a stale owner never
    /// changes or removes another input's marker. Markers written before it existed have
    /// none, and only the guardian and `recover` change them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) owner: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Native {
    pub(crate) codes: Vec<u32>,
    pub(crate) group: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Phase {
    /// Written before the child is started; its PID isn't known.
    Pending,
    /// The child runs; its PID and start time are known.
    Running,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Child {
    pub(crate) pid: u32,
    /// From `/proc/<pid>/stat`, so a reused PID isn't mistaken for the child.
    pub(crate) start_time: u64,
}

impl Marker {
    /// A `pending` marker for `operation`, written by this server now.
    pub(crate) fn pending(operation: &str, buttons: Vec<u32>) -> Self {
        Self {
            operation: operation.to_owned(),
            phase: Phase::Pending,
            server_pid: std::process::id(),
            since: now(),
            child: None,
            buttons,
            output: None,
            keyboard: None,
            released: None,
            owner: Some(owner()),
        }
    }
}

/// A name no other marker has: this process's ID, the time and a count.
fn owner() -> String {
    static COUNT: AtomicU64 = AtomicU64::new(0);
    let count = COUNT.fetch_add(1, Ordering::Relaxed);
    let nanos = Utc::now().timestamp_nanos_opt().unwrap_or_default();
    format!("{}-{nanos}-{count}", std::process::id())
}

/// The time now, as markers record it.
pub(crate) fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// The marker this server wrote. Dropping it leaves the file in place: only `clear`
/// removes it, once the input is known to be released.
#[derive(Debug)]
pub(crate) struct Written {
    dir: PathBuf,
    marker: Marker,
}

impl Written {
    /// Writes `marker` if no marker is set, replacing the file whole, so a reader never
    /// sees half of it.
    pub(crate) async fn write(runtime: &RuntimeDir, marker: Marker) -> std::io::Result<Self> {
        let written = Self {
            dir: runtime.path().to_owned(),
            marker,
        };
        let _locked = lock(&written.dir).await?;
        if let Some(found) = current(&written.dir)? {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!(
                    "another input's marker is set ({})",
                    Snapshot::parse(&written.dir, &found).summary()
                ),
            ));
        }
        save(&written.dir, &written.marker)?;
        Ok(written)
    }

    /// Changes the marker and writes it again, if the file is still this marker.
    pub(crate) async fn update(&mut self, change: impl FnOnce(&mut Marker)) -> std::io::Result<()> {
        let _locked = lock(&self.dir).await?;
        if !self.is_current()? {
            return Err(std::io::Error::other(
                "the marker was removed or replaced by another input's meanwhile",
            ));
        }
        change(&mut self.marker);
        save(&self.dir, &self.marker)
    }

    /// Removes the marker, if the file is still this marker. Another input's stays.
    pub(crate) async fn clear(self) -> std::io::Result<()> {
        let _locked = lock(&self.dir).await?;
        if self.is_current()? {
            std::fs::remove_file(self.dir.join(INPUT_DIRTY))?;
        }
        Ok(())
    }

    fn is_current(&self) -> std::io::Result<bool> {
        let Some(bytes) = current(&self.dir)? else {
            return Ok(false);
        };
        let found: Option<Marker> = serde_json::from_slice(&bytes).ok();
        Ok(found.is_some_and(|found| found.owner == self.marker.owner))
    }
}

/// The marker as the guardian or `recover` read it, which they change or remove only if
/// the file still holds the same bytes.
#[derive(Debug)]
pub(crate) struct Snapshot {
    /// `None` when the file couldn't be read.
    bytes: Option<Vec<u8>>,
    pub(crate) found: Found,
}

impl Snapshot {
    fn parse(dir: &Path, bytes: &[u8]) -> Found {
        serde_json::from_slice(bytes).map_or_else(
            |error| Found::Unreadable {
                error: format!("parse {}: {error}", dir.join(INPUT_DIRTY).display()),
            },
            Found::Marker,
        )
    }

    /// Removes the marker if it is still the one read.
    pub(crate) async fn clear(&self, runtime: &RuntimeDir) -> Result<(), String> {
        let dir = runtime.path();
        let _locked = lock(dir).await.map_err(|error| lock_failed(&error))?;
        self.unchanged(dir)?;
        let path = dir.join(INPUT_DIRTY);
        std::fs::remove_file(&path).map_err(|error| format!("remove {}: {error}", path.display()))
    }

    /// Writes the marker again with the time the crash guardian sent its releases, if it
    /// is still the one read.
    pub(crate) async fn note_released(&self, runtime: &RuntimeDir) -> Result<(), String> {
        let Found::Marker(marker) = &self.found else {
            return Err("the marker can't be read, so the releases aren't noted in it".to_owned());
        };
        let dir = runtime.path();
        let _locked = lock(dir).await.map_err(|error| lock_failed(&error))?;
        self.unchanged(dir)?;
        let mut marker = marker.clone();
        marker.released = Some(now());
        save(dir, &marker).map_err(|error| format!("write the marker: {error}"))
    }

    fn unchanged(&self, dir: &Path) -> Result<(), String> {
        let Some(read) = &self.bytes else {
            return Err(format!(
                "{} couldn't be read, so it stays; remove it by hand once all input is released",
                dir.join(INPUT_DIRTY).display()
            ));
        };
        if current(dir).ok().flatten().as_ref() != Some(read) {
            return Err(
                "the marker was removed or replaced meanwhile, so it was left as it is now"
                    .to_owned(),
            );
        }
        Ok(())
    }
}

fn lock_failed(error: &std::io::Error) -> String {
    format!("{error}; the marker stays")
}

/// Takes the marker's lock, waiting up to `LOCK_WAIT` for another change to finish.
async fn lock(dir: &Path) -> std::io::Result<File> {
    let path = dir.join(LOCK);
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(&path)?;
    let until = Instant::now() + LOCK_WAIT;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(TryLockError::WouldBlock) if Instant::now() < until => {
                tokio::time::sleep(LOCK_RETRY).await;
            }
            Err(TryLockError::WouldBlock) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::WouldBlock,
                    format!("{} stayed locked for {LOCK_WAIT:?}", path.display()),
                ));
            }
            Err(TryLockError::Error(error)) => {
                return Err(std::io::Error::new(
                    error.kind(),
                    format!("lock {}: {error}", path.display()),
                ));
            }
        }
    }
}

/// The marker file's bytes, `None` when there is none.
fn current(dir: &Path) -> std::io::Result<Option<Vec<u8>>> {
    match std::fs::read(dir.join(INPUT_DIRTY)) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// Writes `marker` whole: to `input-dirty.new`, then renamed into place. Only called with
/// the lock held, so no two writers share the staging file.
fn save(dir: &Path, marker: &Marker) -> std::io::Result<()> {
    let path = dir.join(INPUT_DIRTY);
    let staged = path.with_extension("new");
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&staged)?;
    file.write_all(&serde_json::to_vec(marker).map_err(std::io::Error::other)?)?;
    std::fs::rename(&staged, path)
}

/// What `status` and `recover` find.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub(crate) enum Found {
    Marker(Marker),
    /// The file exists but can't be read or parsed; it still blocks everything.
    Unreadable {
        error: String,
    },
}

/// The marker as it is now, to change or remove only if it stays so; `None` when there
/// is none.
pub(crate) fn snapshot(runtime: &RuntimeDir) -> Option<Snapshot> {
    let dir = runtime.path();
    match current(dir) {
        Ok(None) => None,
        Ok(Some(bytes)) => Some(Snapshot {
            found: Snapshot::parse(dir, &bytes),
            bytes: Some(bytes),
        }),
        Err(error) => Some(Snapshot {
            bytes: None,
            found: Found::Unreadable {
                error: format!("read {}: {error}", dir.join(INPUT_DIRTY).display()),
            },
        }),
    }
}

/// The marker, `None` when there is none.
pub(crate) fn read(runtime: &RuntimeDir) -> Option<Found> {
    snapshot(runtime).map(|snapshot| snapshot.found)
}

impl Found {
    /// One line for an error detail: the operation and phase, or why it can't be read.
    pub(crate) fn summary(&self) -> String {
        match self {
            Self::Marker(marker) => format!(
                "{} {} since {}{}",
                marker.operation,
                match marker.phase {
                    Phase::Pending => "pending",
                    Phase::Running => "running",
                },
                marker.since,
                marker
                    .released
                    .as_ref()
                    .map(|at| format!("; the crash guardian sent its releases at {at}"))
                    .unwrap_or_default()
            ),
            Self::Unreadable { error } => error.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runtime(dir: &Path) -> RuntimeDir {
        RuntimeDir::of(&crate::test_support::niri_env(dir)).unwrap()
    }

    #[test]
    fn reads_both_phases_and_keeps_an_unreadable_marker_blocking() {
        let dir = crate::test_support::fresh_dir("marker");
        let runtime = runtime(&dir);
        runtime.create().unwrap();
        assert_eq!(read(&runtime), None);
        let path = runtime.path().join(INPUT_DIRTY);
        std::fs::write(
            &path,
            r#"{"operation":"type_text","phase":"pending","server_pid":7,"since":"2026-10-08T00:00:00.000Z"}"#,
        )
        .unwrap();
        let Some(Found::Marker(pending)) = read(&runtime) else {
            panic!("no marker")
        };
        assert_eq!((pending.phase, pending.child), (Phase::Pending, None));
        assert_eq!(
            Found::Marker(pending).summary(),
            "type_text pending since 2026-10-08T00:00:00.000Z"
        );
        std::fs::write(
            &path,
            r#"{"operation":"click","phase":"running","server_pid":7,"since":"t","child":{"pid":9,"start_time":5},"buttons":[272]}"#,
        )
        .unwrap();
        let Some(Found::Marker(running)) = read(&runtime) else {
            panic!("no marker")
        };
        assert_eq!(
            running.child,
            Some(Child {
                pid: 9,
                start_time: 5
            })
        );
        assert_eq!(running.buttons, [272]);
        std::fs::write(&path, r#"{"operation":"x","surprise":1}"#).unwrap();
        assert!(matches!(read(&runtime), Some(Found::Unreadable { .. })));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn a_written_marker_is_read_back_through_each_change_until_cleared() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = crate::test_support::fresh_dir("marker-write");
        let runtime = runtime(&dir);
        runtime.create().unwrap();
        let mut written = Written::write(&runtime, Marker::pending("click", vec![272]))
            .await
            .unwrap();
        let Some(Found::Marker(pending)) = read(&runtime) else {
            panic!("no marker")
        };
        assert_eq!(
            (pending.phase, pending.server_pid, pending.buttons),
            (Phase::Pending, std::process::id(), vec![272])
        );
        let path = runtime.path().join(INPUT_DIRTY);
        assert_eq!(path.metadata().unwrap().permissions().mode() & 0o777, 0o600);
        let child = Child {
            pid: 9,
            start_time: 5,
        };
        written
            .update(|marker| {
                marker.phase = Phase::Running;
                marker.child = Some(child);
            })
            .await
            .unwrap();
        let Some(Found::Marker(running)) = read(&runtime) else {
            panic!("no marker")
        };
        assert_eq!(
            (running.phase, running.child),
            (Phase::Running, Some(child))
        );
        // Dropping it leaves the marker; only clearing removes it.
        written.clear().await.unwrap();
        assert_eq!(read(&runtime), None);
        drop(
            Written::write(&runtime, Marker::pending("drag", Vec::new()))
                .await
                .unwrap(),
        );
        assert!(read(&runtime).is_some());
        let mut left: Vec<_> = std::fs::read_dir(runtime.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        left.sort();
        assert_eq!(left, ["input-dirty", "input-dirty.lock"]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// The marker file's content, parsed.
    fn marker_now(runtime: &RuntimeDir) -> Marker {
        let Some(Found::Marker(marker)) = read(runtime) else {
            panic!("no marker")
        };
        marker
    }

    #[tokio::test]
    async fn a_stale_owner_never_changes_or_removes_another_inputs_marker() {
        let dir = crate::test_support::fresh_dir("marker-stale");
        let runtime = runtime(&dir);
        runtime.create().unwrap();
        let mut a = Written::write(&runtime, Marker::pending("key", Vec::new()))
            .await
            .unwrap();
        // While A's marker is set, no other input writes one.
        let refused = Written::write(&runtime, Marker::pending("click", vec![272])).await;
        assert_eq!(
            refused.unwrap_err().kind(),
            std::io::ErrorKind::AlreadyExists
        );
        // `recover` clears A's, and B writes its own.
        snapshot(&runtime).unwrap().clear(&runtime).await.unwrap();
        let b = Written::write(&runtime, Marker::pending("click", vec![272]))
            .await
            .unwrap();
        let theirs = marker_now(&runtime);
        let late = a.update(|marker| marker.phase = Phase::Running).await;
        assert!(late.is_err());
        assert_eq!(marker_now(&runtime), theirs);
        a.clear().await.unwrap();
        assert_eq!(marker_now(&runtime), theirs);
        b.clear().await.unwrap();
        assert_eq!(read(&runtime), None);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn a_clear_compares_under_the_lock_and_fails_closed_without_it() {
        let dir = crate::test_support::fresh_dir("marker-lock");
        let runtime = runtime(&dir);
        runtime.create().unwrap();
        let a = Written::write(&runtime, Marker::pending("key", Vec::new()))
            .await
            .unwrap();
        // Recovery and B, holding the lock, replace A's marker while A's clear waits.
        let locked = lock(runtime.path()).await.unwrap();
        let clearing = tokio::spawn(a.clear());
        tokio::time::sleep(LOCK_RETRY * 4).await;
        assert!(!clearing.is_finished());
        let theirs = Marker::pending("click", vec![272]);
        save(runtime.path(), &theirs).unwrap();
        drop(locked);
        clearing.await.unwrap().unwrap();
        assert_eq!(marker_now(&runtime), theirs);
        // A lock that stays taken fails the change and leaves the marker.
        let b = Written {
            dir: runtime.path().to_owned(),
            marker: theirs.clone(),
        };
        let held = lock(runtime.path()).await.unwrap();
        let error = b.clear().await.unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock);
        assert_eq!(marker_now(&runtime), theirs);
        drop(held);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn the_guardian_and_recover_change_only_the_marker_they_read() {
        let dir = crate::test_support::fresh_dir("marker-snapshot");
        let runtime = runtime(&dir);
        runtime.create().unwrap();
        let path = runtime.path().join(INPUT_DIRTY);
        // A marker from before owners, as an older server wrote it.
        std::fs::write(
            &path,
            r#"{"operation":"drag","phase":"pending","server_pid":7,"since":"t","buttons":[272]}"#,
        )
        .unwrap();
        let old = snapshot(&runtime).unwrap();
        assert!(matches!(&old.found, Found::Marker(marker) if marker.owner.is_none()));
        // B's marker replaces it before the guardian or `recover` acts on what it read.
        std::fs::remove_file(&path).unwrap();
        let b = Written::write(&runtime, Marker::pending("click", vec![272]))
            .await
            .unwrap();
        let theirs = marker_now(&runtime);
        assert!(old.note_released(&runtime).await.is_err());
        assert!(old.clear(&runtime).await.is_err());
        assert_eq!(marker_now(&runtime), theirs);
        // On the marker they read, the guardian notes its releases and `recover` clears.
        let current = snapshot(&runtime).unwrap();
        current.note_released(&runtime).await.unwrap();
        assert!(marker_now(&runtime).released.is_some());
        snapshot(&runtime).unwrap().clear(&runtime).await.unwrap();
        assert_eq!(read(&runtime), None);
        drop(b);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
