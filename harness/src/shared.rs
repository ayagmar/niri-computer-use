//! A `--shared` run's own checks, after the nested session: every server bridged its
//! client to the nested niri's shared engine, and the engine exited on its own, removing
//! its socket, before `TEST_DIR` is removed.

use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::failure::{Context as _, Failure, Result};
use crate::log::Log;
use crate::test_dir::TestDir;

/// What a server in shared mode writes to stderr when it serves its client itself.
const FALLBACK: &str = "serving this client standalone";
/// The engine's two seconds of idle grace, and room to exit.
const ENGINE_EXIT: Duration = Duration::from_secs(5);

pub(crate) fn check(test_dir: &TestDir, artifacts: &Path, log: &mut Log) -> Result<()> {
    let fell_back = fallbacks(artifacts)?;
    for line in &fell_back {
        log.line(&format!("  {line}"))?;
    }
    let deadline = Instant::now() + ENGINE_EXIT;
    let mut sockets = engine_sockets(test_dir)?;
    while !sockets.is_empty() && Instant::now() < deadline {
        pause();
        sockets = engine_sockets(test_dir)?;
    }
    if !fell_back.is_empty() {
        return Err(Failure::new(
            "a server in shared mode served its client standalone",
        ));
    }
    if !sockets.is_empty() {
        return Err(Failure::new(format!(
            "the shared engine still listens on {} {ENGINE_EXIT:?} after the run",
            sockets.join(", ")
        )));
    }
    log.line("shared engine: every server bridged to it, and it exited")
}

/// The fallback notes in the servers' logs, each with its log's name.
fn fallbacks(artifacts: &Path) -> Result<Vec<String>> {
    let mut found = Vec::new();
    for entry in fs::read_dir(artifacts).context(format!("list {}", artifacts.display()))? {
        let path = entry.context("list the artifacts")?.path();
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let is_log = path.extension().is_some_and(|extension| extension == "log");
        if !(name.starts_with("server-") && is_log) {
            continue;
        }
        let text = fs::read_to_string(&path).context(format!("read {}", path.display()))?;
        found.extend(
            text.lines()
                .filter(|line| line.contains(FALLBACK))
                .map(|line| format!("{name}: {line}")),
        );
    }
    Ok(found)
}

/// Every `engine.sock` in the servers' runtime directories under `TEST_DIR`.
fn engine_sockets(test_dir: &TestDir) -> Result<Vec<String>> {
    let parent = test_dir.run().join("niri-computer-use");
    let Ok(instances) = fs::read_dir(&parent) else {
        return Ok(Vec::new());
    };
    let mut found = Vec::new();
    for entry in instances {
        let socket = entry
            .context(format!("list {}", parent.display()))?
            .path()
            .join("engine.sock");
        if socket.exists() {
            found.push(socket.display().to_string());
        }
    }
    Ok(found)
}

#[expect(
    clippy::disallowed_methods,
    reason = "the synchronous harness has no async runtime; the wait for the engine is bounded"
)]
fn pause() {
    std::thread::sleep(Duration::from_millis(100));
}
