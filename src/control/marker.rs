//! The input-dirty marker, `<runtime dir>/input-dirty`: written before any input is
//! dispatched and removed only once the input is known to be released (plan §11). While
//! it exists, no server takes the lease and `resume` refuses; only `recover` and the input
//! that wrote it clear it.

use std::io::Write as _;
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::{Path, PathBuf};

use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};

use super::runtime::{INPUT_DIRTY, RuntimeDir};

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
        }
    }
}

/// The time now, as markers record it.
pub(crate) fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// The marker this server wrote. Dropping it leaves the file in place: only `clear`
/// removes it, once the input is known to be released.
#[derive(Debug)]
pub(crate) struct Written {
    path: PathBuf,
    marker: Marker,
}

impl Written {
    /// Writes `marker`, replacing the file whole, so a reader never sees half of it.
    pub(crate) fn write(runtime: &RuntimeDir, marker: Marker) -> std::io::Result<Self> {
        let written = Self {
            path: runtime.path().join(INPUT_DIRTY),
            marker,
        };
        written.save()?;
        Ok(written)
    }

    /// Changes the marker and writes it again.
    pub(crate) fn update(&mut self, change: impl FnOnce(&mut Marker)) -> std::io::Result<()> {
        change(&mut self.marker);
        self.save()
    }

    pub(crate) fn clear(self) -> std::io::Result<()> {
        std::fs::remove_file(&self.path)
    }

    fn save(&self) -> std::io::Result<()> {
        save(&self.path, &self.marker)
    }
}

/// Writes `marker` again with the time the crash guardian sent its releases.
pub(crate) fn note_released(runtime: &RuntimeDir, mut marker: Marker) -> std::io::Result<()> {
    marker.released = Some(now());
    save(&runtime.path().join(INPUT_DIRTY), &marker)
}

fn save(path: &Path, marker: &Marker) -> std::io::Result<()> {
    let staged = path.with_extension("new");
    let mut file = std::fs::OpenOptions::new()
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

/// The marker, `None` when there is none.
pub(crate) fn read(runtime: &RuntimeDir) -> Option<Found> {
    let path = runtime.path().join(INPUT_DIRTY);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => {
            return Some(Found::Unreadable {
                error: format!("read {}: {error}", path.display()),
            });
        }
    };
    Some(serde_json::from_str(&text).map_or_else(
        |error| Found::Unreadable {
            error: format!("parse {}: {error}", path.display()),
        },
        Found::Marker,
    ))
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
    use crate::Env;

    fn runtime(dir: &Path) -> RuntimeDir {
        RuntimeDir::of(&Env {
            niri_socket: Some(dir.join("niri.test.sock")),
            runtime_dir: Some(dir.to_path_buf()),
            ..Env::default()
        })
        .unwrap()
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

    #[test]
    fn a_written_marker_is_read_back_through_each_change_until_cleared() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = crate::test_support::fresh_dir("marker-write");
        let runtime = runtime(&dir);
        runtime.create().unwrap();
        let mut written = Written::write(&runtime, Marker::pending("click", vec![272])).unwrap();
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
            .unwrap();
        let Some(Found::Marker(running)) = read(&runtime) else {
            panic!("no marker")
        };
        assert_eq!(
            (running.phase, running.child),
            (Phase::Running, Some(child))
        );
        // Dropping it leaves the marker; only clearing removes it.
        written.clear().unwrap();
        assert_eq!(read(&runtime), None);
        drop(Written::write(&runtime, Marker::pending("drag", Vec::new())).unwrap());
        assert!(read(&runtime).is_some());
        assert_eq!(std::fs::read_dir(runtime.path()).unwrap().count(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
