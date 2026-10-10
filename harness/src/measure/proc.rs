//! What `measure` reads from `/proc` (pure): a process's PSS, RSS and CPU time, and the
//! median of a few samples.

/// `Pss:` in `/proc/<pid>/smaps_rollup`, in KiB.
pub(super) fn pss_kib(smaps_rollup: &str) -> Option<u64> {
    kib(smaps_rollup, "Pss:")
}

/// `VmRSS:` in `/proc/<pid>/status`, in KiB.
pub(super) fn rss_kib(status: &str) -> Option<u64> {
    kib(status, "VmRSS:")
}

fn kib(text: &str, key: &str) -> Option<u64> {
    let line = text.lines().find(|line| line.starts_with(key))?;
    line.get(key.len()..)?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

/// `utime + stime` from `/proc/<pid>/stat`, in clock ticks. The command name, in
/// parentheses, may hold spaces and parentheses itself, so the fields count from the last
/// `)`: `utime` and `stime` are the 14th and 15th fields of the line.
pub(super) fn cpu_ticks(stat: &str) -> Option<u64> {
    let (_, rest) = stat.rsplit_once(") ")?;
    let mut fields = rest.split_whitespace().skip(11);
    let utime: u64 = fields.next()?.parse().ok()?;
    let stime: u64 = fields.next()?.parse().ok()?;
    Some(utime + stime)
}

/// The median of `values`, the lower one of the middle two for an even count.
pub(super) fn median<T: Ord + Copy>(values: &[T]) -> Option<T> {
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    sorted.get(sorted.len().checked_sub(1)? / 2).copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_time_counts_fields_after_a_command_name_with_spaces_and_parentheses() {
        let stat = "4711 (a (b) c) S 1 4711 4711 0 -1 4194560 100 0 0 0 7 3 0 0 20 0 1 0 5";
        assert_eq!(cpu_ticks(stat), Some(10));
        assert_eq!(cpu_ticks("garbage"), None);
    }

    #[test]
    fn memory_comes_from_its_own_line_in_kib() {
        let rollup = "55d0-7ffc ---p 00000000 00:00 0 [rollup]\nRss: 9000 kB\nPss: 4321 kB\nPss_Anon: 1 kB\n";
        assert_eq!(pss_kib(rollup), Some(4321));
        assert_eq!(rss_kib("Name:\tx\nVmRSS:\t  8765 kB\n"), Some(8765));
        assert_eq!(rss_kib("Name:\tzombie\n"), None);
    }
}
