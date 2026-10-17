//! Follows the audit log as servers append to it and rotate it.

use std::fs::File;
use std::io::{self, Read as _, Seek as _, SeekFrom};
use std::os::unix::fs::MetadataExt as _;
use std::path::PathBuf;

use super::Lines;

/// The most read from the log at once, so a burst can't hold up the state lines.
const READ_LIMIT: u64 = 1024 * 1024;

/// How far back from the end `backlog` looks for recent lines.
const BACKLOG_BYTES: u64 = 256 * 1024;

/// A position in the log file the path names now.
#[derive(Debug)]
pub(crate) struct Tail {
    path: PathBuf,
    /// The file read so far, by device and inode.
    file: Option<(u64, u64)>,
    offset: u64,
    lines: Lines,
}

impl Tail {
    /// Starts at the end of the log, returning its last `count` whole lines.
    pub(crate) fn open(path: PathBuf, count: usize) -> (Self, Vec<Vec<u8>>) {
        let mut tail = Self {
            path,
            file: None,
            offset: 0,
            lines: Lines::default(),
        };
        let recent = tail.backlog(count).unwrap_or_default();
        (tail, recent)
    }

    fn backlog(&mut self, count: usize) -> io::Result<Vec<Vec<u8>>> {
        let mut file = File::open(&self.path)?;
        let meta = file.metadata()?;
        let start = meta.len().saturating_sub(BACKLOG_BYTES);
        file.seek(SeekFrom::Start(start))?;
        let mut bytes = Vec::new();
        file.take(BACKLOG_BYTES).read_to_end(&mut bytes)?;
        self.file = Some((meta.dev(), meta.ino()));
        self.offset = start + bytes.len() as u64;
        let mut lines = Lines::default().feed(&bytes);
        // A start inside the file may cut its first line.
        if start > 0 && !lines.is_empty() {
            lines.remove(0);
        }
        // A last line still being written is read whole by the next `poll`.
        if bytes.last().is_some_and(|&byte| byte != b'\n') {
            self.offset -= bytes
                .iter()
                .rev()
                .take_while(|&&byte| byte != b'\n')
                .count() as u64;
        }
        let skip = lines.len().saturating_sub(count);
        Ok(lines.split_off(skip))
    }

    /// The whole lines appended since the last call. A log moved aside or replaced is
    /// read from its start; one that is missing yields nothing until it is back.
    pub(crate) fn poll(&mut self) -> io::Result<Vec<Vec<u8>>> {
        let mut file = match File::open(&self.path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            opened => opened?,
        };
        let meta = file.metadata()?;
        let identity = Some((meta.dev(), meta.ino()));
        if identity != self.file || meta.len() < self.offset {
            self.file = identity;
            self.offset = 0;
            self.lines = Lines::default();
        }
        file.seek(SeekFrom::Start(self.offset))?;
        let mut bytes = Vec::new();
        file.take(READ_LIMIT).read_to_end(&mut bytes)?;
        self.offset += bytes.len() as u64;
        Ok(self.lines.feed(&bytes))
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use super::*;

    const NONE: Vec<Vec<u8>> = Vec::new();

    fn append(path: &std::path::Path, text: &str) {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        file.write_all(text.as_bytes()).unwrap();
    }

    fn texts(lines: Vec<Vec<u8>>) -> Vec<String> {
        lines
            .into_iter()
            .map(|line| String::from_utf8(line).unwrap())
            .collect()
    }

    #[test]
    fn starts_with_the_last_lines_then_reads_each_new_line_once() {
        let dir = crate::test_support::fresh_dir("watch-tail");
        let path = dir.join("audit.jsonl");
        append(&path, "one\ntwo\nthree\nhalf");
        let (mut tail, recent) = Tail::open(path.clone(), 2);
        assert_eq!(texts(recent), ["two", "three"]);
        assert_eq!(tail.poll().unwrap(), NONE);
        append(&path, "-done\nfour\n");
        assert_eq!(texts(tail.poll().unwrap()), ["half-done", "four"]);
        assert_eq!(tail.poll().unwrap(), NONE);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn follows_the_log_when_it_is_moved_aside_and_started_again() {
        let dir = crate::test_support::fresh_dir("watch-rotate");
        let path = dir.join("audit.jsonl");
        append(&path, "old\n");
        let (mut tail, _) = Tail::open(path.clone(), 0);
        std::fs::rename(&path, dir.join("audit.jsonl.1")).unwrap();
        assert_eq!(tail.poll().unwrap(), NONE);
        append(&path, "new\n");
        assert_eq!(texts(tail.poll().unwrap()), ["new"]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_log_that_appears_later_is_read_from_its_start() {
        let dir = crate::test_support::fresh_dir("watch-late");
        let path = dir.join("audit.jsonl");
        let (mut tail, recent) = Tail::open(path.clone(), 5);
        assert_eq!(recent, NONE);
        assert_eq!(tail.poll().unwrap(), NONE);
        append(&path, "first\n");
        assert_eq!(texts(tail.poll().unwrap()), ["first"]);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
