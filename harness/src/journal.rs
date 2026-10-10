//! The host journal check: nothing a nested run starts may write to the user's journal.
//! The nested environment replaces every socket the harness knows of, but programs that
//! log to journald connect to `/run/systemd/journal/socket` directly; Qt does by default,
//! and so does `dbus-broker-launch`. Every process of the run stays in the harness's own
//! cgroup, so the check reads the journal entries written since the run started from that
//! cgroup. Only their program name and PID are read, never their message. Like C1, it
//! fails conservatively: anything else in the same cgroup that logs meanwhile, such as the
//! terminal the harness runs in, fails the run too.

use std::ffi::OsString;
use std::fs;
use std::time::Duration;

use serde_json::Value;

use crate::failure::{Context as _, Failure, Result};
use crate::log::Log;
use crate::runner::{self, ChildEnv, Group, Invocation, Sink};

const DEADLINE: Duration = Duration::from_secs(10);

/// Where the user journal stood before the run, and the cgroup to check.
#[derive(Debug)]
pub(crate) struct Mark {
    cursor: String,
    cgroup: String,
}

/// Reads the harness's cgroup and the journal's current cursor.
pub(crate) fn mark() -> Result<Mark> {
    let cgroup = fs::read_to_string("/proc/self/cgroup").context("read /proc/self/cgroup")?;
    let cgroup = unified_cgroup(&cgroup)
        .ok_or_else(|| Failure::new(format!("no cgroup v2 path in {cgroup:?}")))?;
    let output = journalctl(&["--show-cursor", "-n", "0", "-q"])?;
    let output = String::from_utf8_lossy(&output);
    let cursor = cursor(&output).ok_or_else(|| Failure::new("journalctl printed no cursor"))?;
    Ok(Mark {
        cursor: cursor.to_owned(),
        cgroup: cgroup.to_owned(),
    })
}

/// Fails if anything in the harness's cgroup wrote to the user journal since `mark`,
/// logging each entry's program and PID.
pub(crate) fn check(mark: &Mark, log: &mut Log) -> Result<()> {
    let output = journalctl(&[
        &format!("--after-cursor={}", mark.cursor),
        "-o",
        "json",
        "--output-fields=_COMM,_PID",
        &format!("_SYSTEMD_CGROUP={}", mark.cgroup),
    ])?;
    let entries = entries(&String::from_utf8_lossy(&output))?;
    for entry in &entries {
        log.line(&format!("  host journal: an entry from {entry}"))?;
    }
    if entries.is_empty() {
        log.line("host journal: no entries from the run")
    } else {
        Err(Failure::new(format!(
            "host journal: {} entries from the run's cgroup",
            entries.len()
        )))
    }
}

fn journalctl(args: &[&str]) -> Result<Vec<u8>> {
    let mut all = vec![OsString::from("--user"), OsString::from("--no-pager")];
    all.extend(args.iter().map(OsString::from));
    let output = runner::run(&Invocation {
        program: "journalctl",
        args: all,
        env: ChildEnv::Inherit,
        output: Sink::Capture,
        group: Group::Own,
        deadline: DEADLINE,
    })?;
    Ok(output.stdout)
}

/// The cgroup v2 path in `/proc/<pid>/cgroup`: the `0::<path>` line.
fn unified_cgroup(text: &str) -> Option<&str> {
    text.lines()
        .find_map(|line| line.strip_prefix("0::"))
        .filter(|path| path.starts_with('/'))
}

/// The cursor in `journalctl --show-cursor`'s `-- cursor: <cursor>` line.
fn cursor(text: &str) -> Option<&str> {
    text.lines()
        .find_map(|line| line.strip_prefix("-- cursor: "))
        .map(str::trim)
        .filter(|cursor| !cursor.is_empty())
}

/// `<program> pid <pid>` for each line of `journalctl -o json`.
fn entries(text: &str) -> Result<Vec<String>> {
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let entry: Value = serde_json::from_str(line).context("parse a journal entry")?;
            let field = |name| entry.get(name).and_then(Value::as_str).unwrap_or("?");
            Ok(format!("{} pid {}", field("_COMM"), field("_PID")))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_unified_cgroup() {
        let text = "0::/user.slice/user-1000.slice/user@1000.service/app.slice/kitty-1.scope\n";
        assert_eq!(
            unified_cgroup(text),
            Some("/user.slice/user-1000.slice/user@1000.service/app.slice/kitty-1.scope")
        );
        assert_eq!(unified_cgroup("1:name=systemd:/x\n"), None);
        assert_eq!(unified_cgroup("0::\n"), None);
    }

    #[test]
    fn reads_the_cursor_line() {
        assert_eq!(cursor("-- cursor: s=ab;i=2eef1\n"), Some("s=ab;i=2eef1"));
        assert_eq!(cursor("-- No entries --\n-- cursor: s=1\n"), Some("s=1"));
        assert_eq!(cursor("-- No entries --\n"), None);
    }

    #[test]
    fn names_each_entry_by_program_and_pid_only() {
        let text = concat!(
            r#"{"__CURSOR":"s=1","_COMM":"dbus-broker-lau","_PID":"445629"}"#,
            "\n",
            r#"{"__CURSOR":"s=2","_COMM":"qml6","_PID":"7","MESSAGE":"private"}"#,
            "\n"
        );
        assert_eq!(
            entries(text).unwrap(),
            ["dbus-broker-lau pid 445629", "qml6 pid 7"]
        );
        assert_eq!(entries("").unwrap(), Vec::<String>::new());
        assert!(entries("not json\n").is_err());
    }
}
