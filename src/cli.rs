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
