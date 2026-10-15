//! The session a bridge serves when its client requires the shared engine and none could
//! be used: it lists the client's tools, and every call fails with `engine_unavailable` and
//! why, so the agent can tell the user. It never reaches an engine, niri or the desktop.

use rmcp::model::{
    CallToolRequestParams, CallToolResponse, Implementation, ListToolsResult,
    PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer, ServerHandler, ServiceExt as _};

use crate::error::{ErrorName, ToolError};
use crate::session::Incoming;

/// Answers every call with `engine_unavailable`.
pub(crate) struct Unavailable {
    tools: Vec<Tool>,
    detail: String,
}

impl Unavailable {
    /// A session listing `tools`, whose calls fail with `reason`, why the shared engine
    /// couldn't be used.
    pub(crate) fn new(tools: Vec<Tool>, reason: &str) -> Self {
        Self {
            tools,
            detail: format!(
                "this client requires the shared engine (shared = \"required\" or NIRI_COMPUTER_USE_SHARED=required), which couldn't be used: {reason}. Fix the cause, then restart this MCP server"
            ),
        }
    }

    /// Serves the client on stdin and stdout until its end, or a line over `MAX_LINE`, as
    /// every other session.
    pub(crate) async fn run(self) -> Result<(), String> {
        let (input, _gone) = Incoming::new(tokio::io::stdin());
        let service = self
            .serve((input, tokio::io::stdout()))
            .await
            .map_err(|error| format!("start MCP session: {error}"))?;
        service
            .waiting()
            .await
            .map(drop)
            .map_err(|error| format!("MCP session: {error}"))
    }
}

impl ServerHandler for Unavailable {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(
                "niri-computer-use",
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(format!(
                "Every tool fails with engine_unavailable: {}",
                self.detail
            ))
    }

    #[expect(
        clippy::unused_async_trait_impl,
        reason = "rmcp's handler methods return futures; this answer needs nothing awaited"
    )]
    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult::with_all_items(self.tools.clone()))
    }

    #[expect(
        clippy::unused_async_trait_impl,
        reason = "rmcp's handler methods return futures; this answer needs nothing awaited"
    )]
    async fn call_tool(
        &self,
        _request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let error = ToolError::new(ErrorName::EngineUnavailable, self.detail.clone());
        Ok(error.into_result().into())
    }
}
