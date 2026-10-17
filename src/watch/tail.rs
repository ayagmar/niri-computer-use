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

/// A position in the log file read so far, which the path named when it was opened.
#[derive(Debug)]
pub(crate) struct Tail {
    path: PathBuf,
    /// The file read so far, kept open so what was appended to it before it was moved
    /// aside is still read.
    file: Option<File>,
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
        let start = file.metadata()?.len().saturating_sub(BACKLOG_BYTES);
        file.seek(SeekFrom::Start(start))?;
        let mut bytes = Vec::new();
        (&mut file).take(BACKLOG_BYTES).read_to_end(&mut bytes)?;
        self.file = Some(file);
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

    /// The whole lines appended since the last call. A log moved aside is read to its end,
    /// then the one the path names now from its start; one cut short is read from its
    /// start. While the path names none, only the file read so far is followed.
    pub(crate) fn poll(&mut self) -> io::Result<Vec<Vec<u8>>> {
        let named = self.named()?;
        self.poll_after(named)
    }

    /// The file the path names now, when it isn't the one read so far.
    fn named(&self) -> io::Result<Option<File>> {
        let file = match File::open(&self.path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            opened => opened?,
        };
        Ok(if self.reading(&file)? {
            None
        } else {
            Some(file)
        })
    }

    /// Reads the file read so far to its end, then `named`, the file that replaced it. The
    /// old one is read after `named` was opened, so what was appended to it before it was
    /// moved aside is read too.
    fn poll_after(&mut self, named: Option<File>) -> io::Result<Vec<Vec<u8>>> {
        let (mut lines, more) = self.read()?;
        let Some(file) = named.filter(|_| !more) else {
            return Ok(lines);
        };
        self.file = Some(file);
        self.offset = 0;
        self.lines = Lines::default();
        lines.extend(self.read()?.0);
        Ok(lines)
    }

    /// The whole lines appended to the file read so far, and whether more is left to read.
    fn read(&mut self) -> io::Result<(Vec<Vec<u8>>, bool)> {
        let Some(file) = &mut self.file else {
            return Ok((Vec::new(), false));
        };
        if file.metadata()?.len() < self.offset {
            self.offset = 0;
            self.lines = Lines::default();
        }
        file.seek(SeekFrom::Start(self.offset))?;
        let mut bytes = Vec::new();
        file.take(READ_LIMIT).read_to_end(&mut bytes)?;
        self.offset += bytes.len() as u64;
        let more = bytes.len() as u64 == READ_LIMIT;
        Ok((self.lines.feed(&bytes), more))
    }

    /// Whether `file` is the one read so far.
    fn reading(&self, file: &File) -> io::Result<bool> {
        let Some(current) = &self.file else {
            return Ok(false);
        };
        let (current, other) = (current.metadata()?, file.metadata()?);
        Ok((current.dev(), current.ino()) == (other.dev(), other.ino()))
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

    /// A call appended after the last poll, right before the log is moved aside, is read
    /// before the calls in the new log.
    #[test]
    fn reads_what_was_appended_before_the_log_was_moved_aside() {
        let dir = crate::test_support::fresh_dir("watch-rotate-unread");
        let path = dir.join("audit.jsonl");
        append(&path, "old\n");
        let (mut tail, _) = Tail::open(path.clone(), 0);
        append(&path, "unread\n");
        std::fs::rename(&path, dir.join("audit.jsonl.1")).unwrap();
        append(&path, "new\n");
        assert_eq!(texts(tail.poll().unwrap()), ["unread", "new"]);
        assert_eq!(tail.poll().unwrap(), NONE);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A call appended and the log moved aside after the new log was opened, but before
    /// the old one was read to its end, is still read.
    #[test]
    fn reads_what_was_appended_while_the_log_was_moved_aside_during_a_poll() {
        let dir = crate::test_support::fresh_dir("watch-rotate-race");
        let path = dir.join("audit.jsonl");
        append(&path, "old\n");
        let (mut tail, _) = Tail::open(path.clone(), 0);
        std::fs::rename(&path, dir.join("audit.jsonl.1")).unwrap();
        append(&path, "new\n");
        let named = tail.named().unwrap();
        append(&dir.join("audit.jsonl.1"), "unread\n");
        assert_eq!(texts(tail.poll_after(named).unwrap()), ["unread", "new"]);
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
