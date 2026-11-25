//! Terminal output for the human-only subcommands. In `serve` mode stdout is the MCP
//! transport, so only rmcp writes there.

use serde::Serialize;

#[expect(
    clippy::print_stdout,
    reason = "CLI subcommand output; never runs in serve mode"
)]
pub(crate) fn print_json(value: &impl Serialize) -> serde_json::Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

#[expect(
    clippy::print_stderr,
    reason = "reports a failure to the human; stdout stays the MCP transport"
)]
pub(crate) fn print_error(message: &str) {
    eprintln!("niri-computer-use: {message}");
}

/// Ends the process at once, after `message` on stderr: stdout can't take what it is
/// sent. A write to stdout already running blocks a thread that can't be stopped, which
/// returning from `main` would wait for, maybe forever. The line on stderr waits 100 ms
/// at most, in case stderr blocks too.
#[expect(
    clippy::exit,
    reason = "returning would wait for a stdout write that may never end"
)]
pub(crate) fn exit_at_once(message: String) -> ! {
    let (said, heard) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        print_error(&message);
        said.send(()).ok();
    });
    heard
        .recv_timeout(std::time::Duration::from_millis(100))
        .ok();
    std::process::exit(1)
}

/// Writes `message` to stderr from a thread of its own, waiting at most `limit` for it,
/// for a server on its way out: a client that stopped reading stderr can't hold it.
/// Returns whether the line was written in time.
pub(crate) async fn print_error_within(message: String, limit: std::time::Duration) -> bool {
    within(limit, move || print_error(&message)).await
}

/// Runs `write` on a thread of its own and waits at most `limit` for it to finish.
async fn within(limit: std::time::Duration, write: impl FnOnce() + Send + 'static) -> bool {
    let (done, finished) = tokio::sync::oneshot::channel();
    let spawned = std::thread::Builder::new()
        .name("stderr".to_owned())
        .spawn(move || {
            write();
            done.send(()).ok();
        });
    spawned.is_ok() && tokio::time::timeout(limit, finished).await.is_ok()
}

/// One line for the human running a subcommand.
#[expect(
    clippy::print_stdout,
    reason = "CLI subcommand output; never runs in serve mode"
)]
pub(crate) fn say(line: &str) {
    println!("{line}");
}

/// Asks a question and returns whether the human answered exactly `yes`. End of input or
/// a read error counts as no.
pub(crate) fn confirm(question: &str) -> bool {
    say(&format!("{question} Type yes to confirm:"));
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer).is_ok() && answer.trim() == "yes"
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::within;

    /// A stderr nobody drains blocks its write; the server still goes on at the limit.
    #[tokio::test]
    async fn a_write_that_blocks_is_given_up_at_its_limit() {
        let (unblock, blocked) = std::sync::mpsc::channel::<()>();
        let started = Instant::now();

        let written = within(Duration::from_millis(100), move || {
            blocked.recv().ok();
        })
        .await;

        assert!(!written);
        assert!(started.elapsed() < Duration::from_millis(600));
        drop(unblock);
        assert!(within(Duration::from_secs(1), || {}).await);
    }
}
