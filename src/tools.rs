//! MCP tool definitions. Each tool only translates the call into a module call.

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::model::CallToolResult;
use rmcp::{ErrorData, ServerHandler, tool, tool_handler, tool_router};

use crate::{Env, niri, status};

#[derive(Debug, Clone)]
pub(crate) struct Server {
    env: Env,
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl Server {
    pub(crate) fn new(env: Env) -> Self {
        Self {
            env,
            tool_router: Self::tool_router(),
        }
    }

    /// Readiness report: the niri instance, niri's version and whether this server
    /// supports it, and which required programs are on PATH. Call this first.
    #[tool(annotations(read_only_hint = true))]
    async fn status(&self) -> Result<CallToolResult, ErrorData> {
        structured(&status::collect(&self.env).await)
    }

    /// niri's outputs (monitors) by connector name: modes, logical position and size,
    /// scale and transform, as niri reports them.
    #[tool(annotations(read_only_hint = true))]
    async fn outputs(&self) -> Result<CallToolResult, ErrorData> {
        match niri::outputs(self.env.niri_socket.as_deref()).await {
            Ok(outputs) => structured(&outputs),
            Err(error) => Ok(error.into_result()),
        }
    }
}

#[expect(
    clippy::unused_async_trait_impl,
    reason = "rmcp's tool_handler generates an async list_tools without an await"
)]
#[tool_handler(
    router = self.tool_router,
    name = "niri-desktop-mcp",
    instructions = "Read-only view of a niri desktop. Start with `status`, then use `outputs` for the monitor layout. Errors carry a stable `error` name and the upstream `detail`."
)]
impl ServerHandler for Server {}

fn structured(value: &impl serde::Serialize) -> Result<CallToolResult, ErrorData> {
    let value = serde_json::to_value(value)
        .map_err(|error| ErrorData::internal_error(error.to_string(), None))?;
    Ok(CallToolResult::structured(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tool_is_marked_read_only() {
        for tool in [Server::status_tool_attr(), Server::outputs_tool_attr()] {
            let annotations = tool.annotations.unwrap();
            assert_eq!(annotations.read_only_hint, Some(true), "{}", tool.name);
            assert!(tool.description.is_some_and(|text| !text.is_empty()));
        }
    }

    #[test]
    fn the_server_names_itself_and_gives_instructions() {
        let server = Server::new(Env {
            niri_socket: None,
            path: None,
        });
        let info = server.get_info();
        assert_eq!(info.server_info.name, "niri-desktop-mcp");
        assert!(
            info.instructions
                .is_some_and(|text| text.contains("status"))
        );
        assert!(info.capabilities.tools.is_some());
    }
}
