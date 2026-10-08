//! The clipboard, read with `wl-paste`. It uses the data-control protocol, so it needs no
//! keyboard focus and works without Noctalia.

use std::time::Duration;

use serde::Serialize;

use crate::error::{ErrorName, ToolError};
use crate::runner::{self, Finished};

const DEADLINE: Duration = Duration::from_secs(2);
const MAX_TEXT: u64 = 1024 * 1024;

/// The clipboard's text, or why there is none.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Clipboard {
    text: Option<String>,
    /// `nothing_copied` or `no_text` when `text` is null.
    reason: Option<&'static str>,
}

pub(crate) async fn read_text() -> Result<Clipboard, ToolError> {
    let args = ["--no-newline", "--type", "text"].map(str::to_owned);
    interpret(runner::run("wl-paste", &args, DEADLINE, MAX_TEXT).await?)
}

/// wl-paste 2.3.0 exits 1 with `Nothing is copied` for an empty clipboard, and with
/// `Clipboard content is not available as requested type "text"` when nothing offers text
/// (`src/wl-paste.c:222–245`, `:265–268`).
fn interpret(done: Finished) -> Result<Clipboard, ToolError> {
    if done.status.success() {
        let text = String::from_utf8(done.stdout).map_err(|_| {
            ToolError::new(
                ErrorName::UpstreamError,
                "the clipboard's text isn't valid UTF-8",
            )
        })?;
        return Ok(Clipboard {
            text: Some(text),
            reason: None,
        });
    }
    let reason = if done.stderr.starts_with("Nothing is copied") {
        "nothing_copied"
    } else if done.stderr.contains("is not available as requested type") {
        "no_text"
    } else {
        return Err(done.failure("wl-paste"));
    };
    Ok(Clipboard {
        text: None,
        reason: Some(reason),
    })
}

#[cfg(test)]
mod tests {
    use std::os::unix::process::ExitStatusExt as _;
    use std::process::ExitStatus;

    use super::*;

    fn finished(code: i32, stdout: &[u8], stderr: &str) -> Finished {
        Finished {
            status: ExitStatus::from_raw(code << 8),
            stdout: stdout.to_vec(),
            stderr: stderr.to_owned(),
        }
    }

    #[test]
    fn text_empty_clipboards_and_non_text_are_results() {
        assert_eq!(
            interpret(finished(0, "héllo".as_bytes(), "")),
            Ok(Clipboard {
                text: Some("héllo".to_owned()),
                reason: None
            })
        );
        assert_eq!(
            interpret(finished(1, b"", "Nothing is copied\n"))
                .unwrap()
                .reason,
            Some("nothing_copied")
        );
        let no_text = "Clipboard content is not available as requested type \"text\"\nUse \"wl-paste --list-types\" to view available types.\n";
        assert_eq!(
            interpret(finished(1, b"", no_text)).unwrap().reason,
            Some("no_text")
        );
    }

    #[test]
    fn other_failures_and_invalid_text_are_upstream_errors() {
        let failed = interpret(finished(1, b"", "Failed to connect to a Wayland server\n"));
        assert_eq!(
            failed.unwrap_err(),
            ToolError::new(
                ErrorName::UpstreamError,
                "wl-paste exited with exit status: 1: Failed to connect to a Wayland server"
            )
        );
        assert_eq!(
            interpret(finished(0, &[0xFF, 0xFE], "")).unwrap_err().name,
            ErrorName::UpstreamError
        );
    }
}
