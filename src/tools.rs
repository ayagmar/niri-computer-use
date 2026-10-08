//! MCP tool definitions. Each tool only translates the call into a module call.

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::model::CallToolResult;
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer, ServerHandler, tool, tool_handler, tool_router};

use crate::niri::events::{EventStream, StreamState};
use crate::{Env, niri, status};

#[derive(Debug, Clone)]
pub(crate) struct Server {
    env: Env,
    /// None without `NIRI_SOCKET`.
    events: Option<EventStream>,
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl Server {
    pub(crate) fn new(env: Env, events: Option<EventStream>) -> Self {
        Self {
            env,
            events,
            tool_router: Self::tool_router(),
        }
    }

    /// Readiness report: the niri instance, niri's version and whether this server
    /// supports it, whether niri's event stream is connected, and which required
    /// programs are on PATH. Call this first.
    #[tool(annotations(read_only_hint = true))]
    async fn status(
        &self,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let stream = self
            .events
            .as_ref()
            .map_or(StreamState::Disconnected, EventStream::state);
        let report = unless_cancelled(
            context.ct.cancelled(),
            status::collect(&self.env, Some(stream)),
        )
        .await?;
        structured(&report)
    }

    /// niri's outputs (monitors) by connector name: modes, logical position and size,
    /// scale and transform, as niri reports them.
    #[tool(annotations(read_only_hint = true))]
    async fn outputs(
        &self,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let outputs = niri::outputs(self.env.niri_socket.as_deref());
        match unless_cancelled(context.ct.cancelled(), outputs).await? {
            Ok(outputs) => structured(&outputs),
            Err(error) => Ok(error.into_result()),
        }
    }

    /// The desktop right now, as one snapshot of niri's event stream: windows (id,
    /// title, `app_id`, pid, workspace, floating, urgent, layout), workspaces, the focused
    /// window id, whether the overview is open, and the keyboard layouts. The focused
    /// window is null while keyboard focus is outside the window layout, for example on
    /// a shell panel, the lock screen or the overview.
    #[tool(annotations(read_only_hint = true))]
    async fn desktop_state(
        &self,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let desktop = niri::desktop(self.events.as_ref());
        match unless_cancelled(context.ct.cancelled(), desktop).await? {
            Ok(desktop) => structured(&desktop),
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
    instructions = "Read-only view of a niri desktop. Start with `status`. Use `desktop_state` for windows and workspaces, and `outputs` for the monitor layout. Errors carry a stable `error` name and the upstream `detail`."
)]
impl ServerHandler for Server {}

/// Runs `work` until it finishes or the client cancels the request. rmcp only cancels the
/// request's token and keeps running the handler, so this drops `work`, and with it any
/// niri connection or wait it holds.
async fn unless_cancelled<T>(
    cancelled: impl Future<Output = ()>,
    work: impl Future<Output = T>,
) -> Result<T, ErrorData> {
    tokio::select! {
        () = cancelled => Err(ErrorData::internal_error("the client cancelled the request", None)),
        done = work => Ok(done),
    }
}

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
        for tool in [
            Server::status_tool_attr(),
            Server::outputs_tool_attr(),
            Server::desktop_state_tool_attr(),
        ] {
            let annotations = tool.annotations.unwrap();
            assert_eq!(annotations.read_only_hint, Some(true), "{}", tool.name);
            assert!(tool.description.is_some_and(|text| !text.is_empty()));
        }
    }

    #[tokio::test]
    async fn cancelling_drops_the_work() {
        let (sender, receiver) = tokio::sync::oneshot::channel::<()>();
        let work = async move {
            let _held = sender;
            std::future::pending::<()>().await;
        };
        assert!(
            unless_cancelled(std::future::ready(()), work)
                .await
                .is_err()
        );
        // The work was dropped, so its sender is gone.
        assert!(receiver.await.is_err());
        assert_eq!(
            unless_cancelled(std::future::pending(), std::future::ready(7))
                .await
                .unwrap(),
            7
        );
    }

    #[test]
    fn the_server_names_itself_and_gives_instructions() {
        let server = Server::new(
            Env {
                niri_socket: None,
                path: None,
            },
            None,
        );
        let info = server.get_info();
        assert_eq!(info.server_info.name, "niri-desktop-mcp");
        assert!(
            info.instructions
                .is_some_and(|text| text.contains("status"))
        );
        assert!(info.capabilities.tools.is_some());
    }
}
