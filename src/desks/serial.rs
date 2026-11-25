//! The lock that serializes one desk's lifecycle. `desk start` holds it until the desk's
//! record says it is ready, or until its failed start is cleaned up; `desk stop` holds it
//! for its whole run. So no start or stop sees another's half-made or half-removed
//! directory. It is a file beside the desks (`Desks::lock`), and it names the generation of
//! the desk's latest start, which a stop needs when the supervisor died before it wrote a
//! record.

use std::fs::{File, OpenOptions, TryLockError};
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
                    "another `desk start` or `desk stop` of desk {name} is running (it holds {}); try again once it is done",
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

    /// Records `generation` as the desk's latest start.
    pub(crate) fn set_generation(&self, generation: &Generation) -> Result<(), String> {
        let text = format!("{}\n", generation.as_str());
        self.file
            .set_len(0)
            .and_then(|()| self.file.write_all_at(text.as_bytes(), 0))
            .map_err(|error| format!("write {}: {error}", self.path.display()))
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
            busy.contains("another `desk start` or `desk stop`"),
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
