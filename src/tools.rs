//! MCP tool definitions. Each tool only translates the call into a module call.

use base64::Engine as _;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer, ServerHandler, schemars, tool, tool_handler, tool_router};
use serde::Deserialize;

use crate::error::CallError;
use crate::niri::events::{EventStream, StreamState};
use crate::observe::{Format, Rect, Target};
use crate::{Env, clipboard, niri, observe, status};

/// The default `max_width` (plan §4). Provisional until M1's image delivery check.
const DEFAULT_MAX_WIDTH: u32 = 1280;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
struct ScreenshotArgs {
    /// `focused_output`, `output:<name>` with a name from `outputs`, or `region`.
    target: String,
    /// Required with target `region`: a rectangle in layout coordinates that lies inside
    /// one output.
    region: Option<RegionArgs>,
    /// The widest image to return, in pixels. The capture scale is lowered to fit.
    /// Defaults to 1280. A region narrower than this keeps full detail, so to read small
    /// text, capture a region around it.
    max_width: Option<u32>,
    /// `jpeg` (the default) or `png`.
    format: Option<FormatArg>,
}

#[derive(Debug, Clone, Copy, Deserialize, schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
struct RegionArgs {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

#[derive(Debug, Clone, Copy, Deserialize, schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(rename_all = "lowercase")]
enum FormatArg {
    Png,
    Jpeg,
}

impl ScreenshotArgs {
    fn request(self) -> Result<observe::Request, String> {
        let region = self.region.map(|r| Rect {
            x: r.x,
            y: r.y,
            width: r.width,
            height: r.height,
        });
        Ok(observe::Request {
            target: Target::parse(&self.target, region)?,
            max_width: Some(self.max_width.unwrap_or(DEFAULT_MAX_WIDTH)),
            format: match self.format {
                Some(FormatArg::Png) => Format::Png,
                Some(FormatArg::Jpeg) | None => Format::Jpeg,
            },
        })
    }
}

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

    /// A screenshot of one output or of a region inside one output, as an image plus
    /// metadata: the output, its transform and layout origin, the captured rectangle in
    /// layout coordinates, and the scale from logical pixels to image pixels. Prefer
    /// `desktop_state` when structured data answers the question.
    #[tool(annotations(read_only_hint = true))]
    async fn screenshot(
        &self,
        Parameters(args): Parameters<ScreenshotArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let request = args.request().map_err(invalid)?;
        let shot = observe::screenshot(self.env.niri_socket.as_deref(), &request);
        match unless_cancelled(context.ct.cancelled(), shot).await? {
            Ok(shot) => {
                let image = base64::engine::general_purpose::STANDARD.encode(&shot.image);
                let mut result = structured(&shot.metadata)?;
                result
                    .content
                    .insert(0, ContentBlock::image(image, shot.metadata.mime_type));
                Ok(result)
            }
            Err(CallError::InvalidArguments(message)) => Err(invalid(message)),
            Err(CallError::Tool(error)) => Ok(error.into_result()),
        }
    }

    /// The clipboard's text, read with `wl-paste`. `text` is null, with a `reason`, when
    /// nothing is copied (`nothing_copied`) or nothing copied is text (`no_text`).
    #[tool(annotations(read_only_hint = true))]
    async fn clipboard_read(
        &self,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        match unless_cancelled(context.ct.cancelled(), clipboard::read_text()).await? {
            Ok(clipboard) => structured(&clipboard),
            Err(error) => Ok(error.into_result()),
        }
    }
}

fn invalid(message: String) -> ErrorData {
    ErrorData::invalid_params(message, None)
}

#[expect(
    clippy::unused_async_trait_impl,
    reason = "rmcp's tool_handler generates an async list_tools without an await"
)]
#[tool_handler(
    router = self.tool_router,
    name = "niri-desktop-mcp",
    instructions = "Read-only view of a niri desktop. Start with `status`. Use `desktop_state` for windows and workspaces and `outputs` for the monitor layout; take a `screenshot` only when you need to see pixels. `clipboard_read` returns the clipboard's text. Errors carry a stable `error` name and the upstream `detail`."
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
            Server::screenshot_tool_attr(),
            Server::clipboard_read_tool_attr(),
        ] {
            let annotations = tool.annotations.unwrap();
            assert_eq!(annotations.read_only_hint, Some(true), "{}", tool.name);
            assert!(tool.description.is_some_and(|text| !text.is_empty()));
        }
    }

    #[test]
    fn screenshot_arguments_default_to_jpeg_at_1280_pixels() {
        let args: ScreenshotArgs = serde_json::from_value(serde_json::json!({
            "target": "region",
            "region": {"x": 1, "y": 2, "width": 3, "height": 4}
        }))
        .unwrap();
        assert_eq!(
            args.request(),
            Ok(observe::Request {
                target: Target::Region(Rect {
                    x: 1,
                    y: 2,
                    width: 3,
                    height: 4
                }),
                max_width: Some(1280),
                format: Format::Jpeg,
            })
        );
        let png: ScreenshotArgs = serde_json::from_value(
            serde_json::json!({"target": "focused_output", "format": "png", "max_width": 640}),
        )
        .unwrap();
        let request = png.request().unwrap();
        assert_eq!(
            (request.format, request.max_width),
            (Format::Png, Some(640))
        );
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
