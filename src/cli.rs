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
