//! One client of the engine.

/// A client: who it is, for the audit log and the lease record.
#[derive(Debug, Clone)]
pub(crate) struct Session {
    /// The process the client started: this server.
    pid: u32,
}

impl Session {
    /// The one client of a server that serves its own stdio.
    pub(crate) fn local() -> Self {
        Self {
            pid: std::process::id(),
        }
    }

    /// The label for the audit log and the lease: the MCP client's name and the process it
    /// started, such as `claude-code/4711`.
    pub(crate) fn label(&self, client: &str) -> String {
        format!("{client}/{}", self.pid)
    }
}
