//! The lock that serializes one desk's lifecycle. `desk start` takes it and hands the same
//! open file to the supervisor as its stdin, so the lock is held while either holds it: a
//! start whose command is killed stays locked until its supervisor has published the desk
//! ready, which lets go, or has exited. `desk stop` holds it for its whole run. So no start
//! or stop sees another's half-made or half-removed directory. It is a file beside the
//! desks (`Desks::lock`), and it names the generation of the desk's latest start, whose
//! scope a stop or start reconciles even when no directory or record says it exists.

use std::fs::{File, OpenOptions, TryLockError};
use std::os::fd::AsFd as _;
use std::os::unix::fs::{FileExt as _, OpenOptionsExt as _};
use std::path::PathBuf;
use std::time::Duration;

use tokio::time::Instant;

use super::layout::Desks;
use super::name::DeskName;
use super::scope::Generation;

/// How often a wait for the lock tries again.
const RETRY: Duration = Duration::from_millis(50);
/// The most of the lock file read back: a generation.
const MAX: usize = 64;

/// The lock of one desk, held until dropped.
#[derive(Debug)]
pub(crate) struct Held {
    file: File,
    path: PathBuf,
}

impl Held {
    /// Takes the lock of the desk `name`, waiting up to `wait` for another `desk start` or
    /// `desk stop` of it to finish.
    pub(crate) async fn take(
        desks: &Desks,
        name: &DeskName,
        wait: Duration,
    ) -> Result<Self, String> {
        let path = desks.lock(name);
        desks
            .create()
            .map_err(|error| format!("create {}: {error}", desks.path().display()))?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(&path)
            .map_err(|error| format!("open {}: {error}", path.display()))?;
        let until = Instant::now() + wait;
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(Self { file, path }),
                Err(TryLockError::WouldBlock) => {}
                Err(TryLockError::Error(error)) => {
                    return Err(format!("lock {}: {error}", path.display()));
                }
            }
            if Instant::now() >= until {
                return Err(format!(
                    "a start or stop of desk {name} is in progress (it holds {}); try again once it is done",
                    path.display()
                ));
            }
            tokio::time::sleep(RETRY).await;
        }
    }

    /// The generation of the desk's latest start, if one was recorded.
    pub(crate) fn generation(&self) -> Result<Option<Generation>, String> {
        let mut bytes = [0; MAX];
        let read = self
            .file
            .read_at(&mut bytes, 0)
            .map_err(|error| format!("read {}: {error}", self.path.display()))?;
        let text = String::from_utf8_lossy(bytes.get(..read).unwrap_or_default());
        let text = text.trim();
        if text.is_empty() {
            return Ok(None);
        }
        Generation::parse(text)
            .map(Some)
            .map_err(|error| format!("read {}: {error}", self.path.display()))
    }

    /// The lock's open file, for the supervisor's stdin: the same open file description,
    /// so the lock stays held while the supervisor runs, whatever happens to this process.
    pub(crate) fn share(&self) -> Result<File, String> {
        self.file
            .try_clone()
            .map_err(|error| format!("share {}: {error}", self.path.display()))
    }

    /// Records `generation` as the desk's latest start.
    pub(crate) fn set_generation(&self, generation: &Generation) -> Result<(), String> {
        let text = format!("{}\n", generation.as_str());
        self.file
            .set_len(0)
            .and_then(|()| self.file.write_all_at(text.as_bytes(), 0))
            .map_err(|error| format!("write {}: {error}", self.path.display()))
    }
}

/// The lock as the supervisor has it: the open file `desk start` handed it as its stdin.
#[derive(Debug)]
pub(crate) struct Handed(std::io::Stdin);

impl Handed {
    /// Checks the supervisor's stdin is the lock file of desk `name` and holds its lock: a
    /// lock on the same open file `desk start` took is already this process's, and any
    /// other is refused rather than waited for.
    pub(crate) fn from_stdin(desks: &Desks, name: &DeskName) -> Result<Self, String> {
        let path = desks.lock(name);
        let stdin = std::io::stdin();
        let given = rustix::fs::fstat(stdin.as_fd())
            .map_err(|error| format!("stat the supervisor's stdin: {error}"))?;
        let lock =
            rustix::fs::stat(&path).map_err(|error| format!("stat {}: {error}", path.display()))?;
        if (given.st_dev, given.st_ino) != (lock.st_dev, lock.st_ino) {
            return Err(format!(
                "the supervisor's stdin isn't {}; `desk start` hands it the desk's lock",
                path.display()
            ));
        }
        rustix::fs::flock(
            stdin.as_fd(),
            rustix::fs::FlockOperation::NonBlockingLockExclusive,
        )
        .map_err(|error| {
            format!(
                "the supervisor's stdin doesn't hold the lock of {}: {error}",
                path.display()
            )
        })?;
        Ok(Self(stdin))
    }

    /// Lets go of the lock once the desk is published ready. `desk start`'s copy lets go
    /// with it: both are one open file. A failed start never calls this; its exit lets go,
    /// once it is torn down.
    pub(crate) fn release(self) {
        if let Err(error) = rustix::fs::flock(self.0.as_fd(), rustix::fs::FlockOperation::Unlock) {
            crate::cli::print_error(&format!("let go of the desk's lock: {error}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Catches a lock that two lifecycle commands of one desk could hold at once, or one
    /// that waits without bound; and a generation that doesn't read back.
    #[tokio::test]
    async fn one_holder_at_a_time_and_the_generation_reads_back() {
        let dir = crate::test_support::short_dir("serial");
        let desks = Desks::of(&dir);
        let web = DeskName::parse("web").unwrap();
        let held = Held::take(&desks, &web, Duration::ZERO).await.unwrap();
        assert_eq!(held.generation(), Ok(None));
        let started = Instant::now();
        let busy = Held::take(&desks, &web, Duration::from_millis(200))
            .await
            .unwrap_err();
        assert!(
            busy.contains("a start or stop of desk web is in progress"),
            "{busy}"
        );
        assert!(started.elapsed() < Duration::from_secs(2));
        // Another desk's lock is its own.
        let other = DeskName::parse("mail").unwrap();
        drop(Held::take(&desks, &other, Duration::ZERO).await.unwrap());
        let generation = Generation::parse("17").unwrap();
        held.set_generation(&generation).unwrap();
        held.set_generation(&Generation::parse("18").unwrap())
            .unwrap();
        drop(held);
        let again = Held::take(&desks, &web, Duration::ZERO).await.unwrap();
        assert_eq!(
            again.generation(),
            Ok(Some(Generation::parse("18").unwrap()))
        );
        assert!(DeskName::parse(".web.lock").is_err());
        assert_eq!(desks.names().unwrap(), Vec::<DeskName>::new());
        drop(again);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
