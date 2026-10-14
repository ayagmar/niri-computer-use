//! What the server needs to know about processes, read from `/proc`. The root is a
//! parameter so tests can use a fake `/proc`.

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

/// Whether `pid` was started with `flag` among its arguments, after the program name, from
/// `/proc/<pid>/cmdline`.
pub(crate) fn has_arg(proc_root: &Path, pid: u32, flag: &str) -> std::io::Result<bool> {
    let cmdline = std::fs::read(proc_root.join(pid.to_string()).join("cmdline"))?;
    Ok(cmdline
        .split(|&byte| byte == 0)
        .skip(1)
        .any(|arg| arg == flag.as_bytes()))
}

/// The process's name from `/proc/<pid>/comm`.
pub(crate) fn comm(proc_root: &Path, pid: u32) -> Option<String> {
    let comm = std::fs::read_to_string(proc_root.join(pid.to_string()).join("comm")).ok()?;
    Some(comm.trim_end().to_owned())
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
    fn finds_a_flag_among_the_arguments_but_not_in_the_program_name() {
        let root = crate::test_support::fresh_dir("procs-cmdline");
        for (pid, cmdline) in [
            (5, &b"niri\0--session\0"[..]),
            (6, b"niri\0"),
            (7, b"--session\0x\0"),
            (8, b"niri\0--session=no\0"),
        ] {
            std::fs::create_dir(root.join(pid.to_string())).unwrap();
            std::fs::write(root.join(format!("{pid}/cmdline")), cmdline).unwrap();
        }
        let session = |pid| has_arg(&root, pid, "--session").unwrap();
        assert!(session(5));
        assert!(!session(6));
        assert!(!session(7));
        assert!(!session(8));
        assert!(has_arg(&root, 9, "--session").is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reads_this_process_from_the_real_proc() {
        let me = stat(Path::new("/proc"), std::process::id()).unwrap();
        assert!(!me.exited());
    }
}
