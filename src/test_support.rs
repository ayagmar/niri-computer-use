//! Helpers shared by unit tests.

pub(crate) mod output_mode;

use std::path::PathBuf;

/// A directory the calling test creates itself, so the test reads and removes nothing
/// else. Creation fails if the name already exists.
pub(crate) fn fresh_dir(name: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "niri-computer-use-{name}-{}-{nanos}",
        std::process::id()
    ));
    std::fs::create_dir(&dir).unwrap();
    dir
}
