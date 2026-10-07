//! The C1 host snapshot, taken before and after a nested run. All reads are read-only.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime};

use crate::environment::Host;
use crate::failure::{Context as _, Failure, Result};
use crate::log::Log;
use crate::runner::{self, ChildEnv, Group, Invocation, Sink};

const DEADLINE: Duration = Duration::from_secs(5);

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Snapshot {
    niri_outputs: String,
    noctalia_status: String,
    runtime_entries: BTreeSet<OsString>,
    x11_entries: BTreeSet<OsString>,
    dconf_modified: Option<SystemTime>,
}

pub(crate) fn take(host: &Host) -> Result<Snapshot> {
    Ok(Snapshot {
        niri_outputs: host_command("niri", &["msg", "--json", "outputs"])?,
        noctalia_status: host_command("noctalia", &["msg", "status"])
            .unwrap_or_else(|failure| format!("unavailable: {failure}")),
        runtime_entries: entries(&host.runtime_dir)?,
        x11_entries: entries(Path::new("/tmp/.X11-unix"))?,
        dconf_modified: fs::metadata(host.home().join(".config/dconf/user"))
            .and_then(|metadata| metadata.modified())
            .ok(),
    })
}

/// One line per field that changed.
pub(crate) fn diff(before: &Snapshot, after: &Snapshot) -> Vec<String> {
    let mut changes = Vec::new();
    if before.niri_outputs != after.niri_outputs {
        changes.push("host niri outputs changed".to_owned());
    }
    if before.noctalia_status != after.noctalia_status {
        changes.push(format!(
            "host Noctalia status changed: {} -> {}",
            before.noctalia_status, after.noctalia_status
        ));
    }
    changes.extend(set_diff(
        "host XDG_RUNTIME_DIR",
        &before.runtime_entries,
        &after.runtime_entries,
    ));
    changes.extend(set_diff(
        "/tmp/.X11-unix",
        &before.x11_entries,
        &after.x11_entries,
    ));
    if before.dconf_modified != after.dconf_modified {
        changes.push("~/.config/dconf/user modification time changed".to_owned());
    }
    changes
}

/// C1: logs each change and the verdict, and fails if anything changed.
pub(crate) fn report(log: &mut Log, before: &Snapshot, after: Result<Snapshot>) -> Result<()> {
    let changes = diff(before, &after?);
    for change in &changes {
        log.line(&format!("  {change}"))?;
    }
    if changes.is_empty() {
        log.line("C1, host snapshot: unchanged")
    } else {
        Err(Failure::new(format!(
            "C1, host snapshot: {} changes",
            changes.len()
        )))
    }
}

fn set_diff(place: &str, before: &BTreeSet<OsString>, after: &BTreeSet<OsString>) -> Vec<String> {
    let added = after
        .difference(before)
        .map(|name| format!("{place}: added {}", name.display()));
    let removed = before
        .difference(after)
        .map(|name| format!("{place}: removed {}", name.display()));
    added.chain(removed).collect()
}

fn host_command(program: &'static str, args: &[&str]) -> Result<String> {
    let output = runner::run(&Invocation {
        program,
        args: args.iter().map(OsString::from).collect(),
        env: ChildEnv::Inherit,
        output: Sink::Capture,
        group: Group::Own,
        deadline: DEADLINE,
    })?;
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn entries(dir: &Path) -> Result<BTreeSet<OsString>> {
    let doing = format!("list {}", dir.display());
    fs::read_dir(dir)
        .context(&doing)?
        .map(|entry| entry.map(|entry| entry.file_name()).context(&doing))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot() -> Snapshot {
        Snapshot {
            niri_outputs: "{}".to_owned(),
            noctalia_status: "{\"locked\":false}".to_owned(),
            runtime_entries: BTreeSet::from([OsString::from("wayland-1")]),
            x11_entries: BTreeSet::new(),
            dconf_modified: None,
        }
    }

    #[test]
    fn report_logs_every_change_and_the_verdict() {
        let path = std::env::temp_dir().join(format!("harness-c1-{}", std::process::id()));
        let mut log = Log::create(&path, false).unwrap();
        report(&mut log, &snapshot(), Ok(snapshot())).unwrap();
        let mut after = snapshot();
        after.dconf_modified = Some(SystemTime::UNIX_EPOCH);
        let changed = report(&mut log, &snapshot(), Ok(after));
        assert_eq!(
            changed.unwrap_err().to_string(),
            "C1, host snapshot: 1 changes"
        );
        let unreadable = report(&mut log, &snapshot(), Err(Failure::new("niri msg failed")));
        assert_eq!(unreadable.unwrap_err().to_string(), "niri msg failed");
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "C1, host snapshot: unchanged\n  ~/.config/dconf/user modification time changed\n"
        );
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn equal_snapshots_have_no_diff() {
        assert_eq!(diff(&snapshot(), &snapshot()), Vec::<String>::new());
    }

    #[test]
    fn diff_names_added_and_removed_entries() {
        let mut after = snapshot();
        after.runtime_entries = BTreeSet::from([OsString::from("dbus-x")]);
        after.dconf_modified = Some(SystemTime::UNIX_EPOCH);
        assert_eq!(
            diff(&snapshot(), &after),
            [
                "host XDG_RUNTIME_DIR: added dbus-x",
                "host XDG_RUNTIME_DIR: removed wayland-1",
                "~/.config/dconf/user modification time changed",
            ]
        );
    }
}
