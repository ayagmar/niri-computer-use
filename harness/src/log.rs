use std::fs::File;
use std::io::Write as _;
use std::path::Path;

use crate::failure::{Context as _, Result};

/// A line log written to a file, optionally echoed to the terminal.
#[derive(Debug)]
pub(crate) struct Log {
    file: File,
    echo: bool,
}

impl Log {
    pub(crate) fn create(path: &Path, echo: bool) -> Result<Self> {
        let file = File::create(path).context(format!("create {}", path.display()))?;
        Ok(Self { file, echo })
    }

    pub(crate) fn line(&mut self, text: &str) -> Result<()> {
        writeln!(self.file, "{text}").context("write log")?;
        if self.echo {
            echo(text);
        }
        Ok(())
    }
}

#[expect(
    clippy::print_stderr,
    reason = "the harness is a terminal tool, never an MCP server"
)]
pub(crate) fn echo(text: &str) {
    eprintln!("{text}");
}
