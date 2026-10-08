//! What `recover` needs to know about processes, read from `/proc`: whether a process
//! is still the one a marker names, and which `wtype` processes the user runs. The root
//! is a parameter so tests can use a fake `/proc`.

use std::path::Path;

/// One process, as `/proc/<pid>/stat` describes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Stat {
    /// `R`, `S`, `Z` and so on.
    pub(crate) state: char,
    /// The process group.
    pub(crate) group: u32,
    /// Field 22, in clock ticks since boot. With the PID it names one process for good.
    pub(crate) start_time: u64,
}

impl Stat {
    pub(crate) const fn exited(self) -> bool {
        self.state == 'Z' || self.state == 'X'
    }
}

/// Parses `/proc/<pid>/stat`. The command name in parentheses may hold spaces and
/// parentheses itself, so the fields are counted from its last `)`.
pub(crate) fn parse_stat(text: &str) -> Option<Stat> {
    let (_, rest) = text.rsplit_once(") ")?;
    let mut fields = rest.split(' ');
    let state = fields.next()?.chars().next()?;
    // `rest` starts at field 3 (state). Field 4 is the parent, 5 the process group, and
    // 22 the start time.
    let group = fields.nth(1)?.parse().ok()?;
    let start_time = fields.nth(16)?.parse().ok()?;
    Some(Stat {
        state,
        group,
        start_time,
    })
}

/// The process `pid` under `proc_root`, if it exists.
pub(crate) fn stat(proc_root: &Path, pid: u32) -> Option<Stat> {
    let text = std::fs::read_to_string(proc_root.join(pid.to_string()).join("stat")).ok()?;
    parse_stat(&text)
}

/// Whether `pid` is still the process that started at `start_time` and hasn't exited.
pub(crate) fn alive_as(proc_root: &Path, pid: u32, start_time: u64) -> bool {
    stat(proc_root, pid).is_some_and(|stat| stat.start_time == start_time && !stat.exited())
}

/// The value of `name` in the environment `pid` started with, from
/// `/proc/<pid>/environ`. `Ok(None)` when the process has no such variable.
pub(crate) fn environ_var(
    proc_root: &Path,
    pid: u32,
    name: &str,
) -> std::io::Result<Option<String>> {
    let environ = std::fs::read(proc_root.join(pid.to_string()).join("environ"))?;
    let prefix = format!("{name}=");
    Ok(environ
        .split(|&byte| byte == 0)
        .find_map(|entry| entry.strip_prefix(prefix.as_bytes()))
        .map(|value| String::from_utf8_lossy(value).into_owned()))
}

/// The PIDs of running processes named `name` whose real user is `uid`.
pub(crate) fn named(proc_root: &Path, name: &str, uid: u32) -> Vec<u32> {
    let Ok(entries) = std::fs::read_dir(proc_root) else {
        return Vec::new();
    };
    let mut pids: Vec<u32> = entries
        .filter_map(|entry| entry.ok()?.file_name().to_str()?.parse::<u32>().ok())
        .filter(|&pid| {
            let dir = proc_root.join(pid.to_string());
            let comm = std::fs::read_to_string(dir.join("comm")).unwrap_or_default();
            let status = std::fs::read_to_string(dir.join("status")).unwrap_or_default();
            comm.trim_end() == name
                && real_uid(&status) == Some(uid)
                && stat(proc_root, pid).is_some_and(|stat| !stat.exited())
        })
        .collect();
    pids.sort_unstable();
    pids
}

/// The real user ID from `/proc/<pid>/status`'s `Uid:` line.
fn real_uid(status: &str) -> Option<u32> {
    status
        .lines()
        .find_map(|line| line.strip_prefix("Uid:"))?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const STAT: &str = "4711 (we (i)rd) S 1 4711 4711 0 -1 4194560 100 0 0 0 1 2 0 0 20 0 1 0 98765 1000 100 18446744073709551615";

    #[test]
    fn reads_state_and_start_time_past_a_tricky_name() {
        assert_eq!(
            parse_stat(STAT),
            Some(Stat {
                state: 'S',
                group: 4711,
                start_time: 98765
            })
        );
        assert_eq!(parse_stat("4711 (short) S 1"), None);
        assert_eq!(parse_stat(""), None);
        assert!(parse_stat(&STAT.replace(") S", ") Z")).unwrap().exited());
    }

    fn process(root: &Path, pid: u32, comm: &str, uid: u32, stat: &str) {
        let dir = root.join(pid.to_string());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("comm"), format!("{comm}\n")).unwrap();
        std::fs::write(
            dir.join("status"),
            format!("Name:\t{comm}\nUid:\t{uid}\t{uid}\t{uid}\t{uid}\n"),
        )
        .unwrap();
        std::fs::write(dir.join("stat"), stat).unwrap();
    }

    #[test]
    fn finds_the_users_running_processes_by_name() {
        let root = crate::test_support::fresh_dir("procs");
        process(&root, 10, "wtype", 1000, STAT);
        process(&root, 11, "wtype", 0, STAT);
        process(&root, 12, "wtyper", 1000, STAT);
        process(&root, 13, "wtype", 1000, &STAT.replace(") S", ") Z"));
        process(&root, 9, "wtype", 1000, STAT);
        std::fs::create_dir(root.join("self-not-a-pid")).unwrap();
        assert_eq!(named(&root, "wtype", 1000), [9, 10]);
        assert!(alive_as(&root, 10, 98765));
        assert!(!alive_as(&root, 10, 98766));
        assert!(!alive_as(&root, 13, 98765));
        assert!(!alive_as(&root, 99, 98765));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reads_one_variable_from_an_environ() {
        let root = crate::test_support::fresh_dir("procs-environ");
        std::fs::create_dir(root.join("5")).unwrap();
        std::fs::write(
            root.join("5/environ"),
            b"A=1\0XDG_SESSION_ID=3\0XDG_SESSION_IDX=9\0",
        )
        .unwrap();
        assert_eq!(
            environ_var(&root, 5, "XDG_SESSION_ID").unwrap().as_deref(),
            Some("3")
        );
        assert_eq!(environ_var(&root, 5, "B").unwrap(), None);
        assert!(environ_var(&root, 6, "A").is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reads_this_process_from_the_real_proc() {
        let me = stat(Path::new("/proc"), std::process::id()).unwrap();
        assert!(!me.exited());
        assert!(alive_as(
            Path::new("/proc"),
            std::process::id(),
            me.start_time
        ));
    }
}
