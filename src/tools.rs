//! MCP tool definitions. Each tool only translates the call into a module call.

use base64::Engine as _;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer, ServerHandler, schemars, tool, tool_handler, tool_router};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::time::Instant;

use crate::act::{self, Outcome};
use crate::audit::{Audit, Call, Caller};
use crate::control::desk::Desk;
use crate::coords::ImagePx;
use crate::error::{CANCELLED, CallError, ToolError};
use crate::input::Input;
use crate::input::keyboard::{self, Expect, Typing};
use crate::input::pointer::{self, Button, Gesture};
use crate::niri::events::{EventStream, StreamState};
use crate::observe::{DEFAULT_MAX_WIDTH, Format, Rect, Target};
use crate::policy::{self, Loaded};
use crate::refs::Shot;
use crate::{Env, clipboard, niri, noctalia, observe, status};

/// Optional arguments are described as their own type with their real default, without
/// `null`, because clients that map tool schemas onto a single-type dialect reject
/// `["integer", "null"]`. The `schemars` attributes only shape the schema; serde still
/// takes an absent field or `null` as `None`.
#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
struct ScreenshotArgs {
    /// `focused_output`, `output:<name>` with a name from `outputs`, or `region`.
    target: String,
    /// Required with target `region`: a rectangle in layout coordinates that lies inside
    /// one output.
    #[schemars(with = "RegionArgs", default, skip_serializing_if = "Option::is_none")]
    region: Option<RegionArgs>,
    /// The widest image to return, in image pixels. The capture scale is lowered when the
    /// capture's logical width times the output's scale is wider. Defaults to 1280. To
    /// read small text, capture a small region around it, or raise `max_width`.
    #[schemars(with = "u32", default = "default_max_width")]
    max_width: Option<u32>,
    /// `jpeg` (the default) or `png`.
    #[schemars(with = "FormatArg", default = "default_format")]
    format: Option<FormatArg>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
struct RegionArgs {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(rename_all = "lowercase")]
enum FormatArg {
    Png,
    Jpeg,
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "schemars serializes the default as the field's type, `Option<u32>`"
)]
const fn default_max_width() -> Option<u32> {
    Some(DEFAULT_MAX_WIDTH)
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "schemars serializes the default as the field's type, `Option<FormatArg>`"
)]
const fn default_format() -> Option<FormatArg> {
    Some(FormatArg::Jpeg)
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

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
struct WindowArgs {
    /// A window id from `desktop_state`.
    id: u64,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
struct WorkspaceArgs {
    /// A workspace id from `desktop_state`, not its index.
    id: u64,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
struct LaunchArgs {
    /// A preset name from the policy file; `status` lists them as `policy.preset_names`.
    preset: String,
    /// With true, focus the preset's one existing window instead of starting another, and
    /// start nothing if several exist. Defaults to false.
    #[serde(default)]
    reuse: bool,
}

/// A pixel of a screenshot, counted from its top-left corner.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
struct PixelArgs {
    x: u32,
    y: u32,
}

impl From<PixelArgs> for ImagePx {
    fn from(pixel: PixelArgs) -> Self {
        Self {
            x: pixel.x,
            y: pixel.y,
        }
    }
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
struct PointArgs {
    /// The `screenshot_ref` of a screenshot taken under this lease, at most a minute old.
    screenshot_ref: String,
    /// The pixel's column in that image, from its left edge.
    x: u32,
    /// The pixel's row in that image, from its top edge.
    y: u32,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(rename_all = "lowercase")]
enum ButtonArg {
    #[default]
    Left,
    Right,
    Middle,
}

impl From<ButtonArg> for Button {
    fn from(button: ButtonArg) -> Self {
        match button {
            ButtonArg::Left => Self::Left,
            ButtonArg::Right => Self::Right,
            ButtonArg::Middle => Self::Middle,
        }
    }
}

const fn one() -> u8 {
    1
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
struct ClickArgs {
    /// The `screenshot_ref` of a screenshot taken under this lease, at most a minute old.
    screenshot_ref: String,
    /// The pixel's column in that image, from its left edge.
    x: u32,
    /// The pixel's row in that image, from its top edge.
    y: u32,
    /// `left` (the default), `right` or `middle`.
    #[serde(default)]
    button: ButtonArg,
    /// 1 (the default) to 3: 2 is a double click.
    #[serde(default = "one")]
    #[schemars(range(min = 1, max = 3))]
    count: u8,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
struct DragArgs {
    /// The `screenshot_ref` of a screenshot taken under this lease, at most a minute old.
    screenshot_ref: String,
    /// Where to press, as a pixel of that image.
    from: PixelArgs,
    /// Where to release, as a pixel of the same image.
    to: PixelArgs,
    /// `left` (the default), `right` or `middle`.
    #[serde(default)]
    button: ButtonArg,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
struct ScrollArgs {
    /// The `screenshot_ref` of a screenshot taken under this lease, at most a minute old.
    screenshot_ref: String,
    /// The pixel's column in that image, from its left edge.
    x: u32,
    /// The pixel's row in that image, from its top edge.
    y: u32,
    /// Wheel notches to the right (negative: left), at most 10. Defaults to 0.
    #[serde(default)]
    #[schemars(range(min = -10, max = 10))]
    notches_x: i32,
    /// Wheel notches down (negative: up), at most 10. Defaults to 0.
    #[serde(default)]
    #[schemars(range(min = -10, max = 10))]
    notches_y: i32,
}

/// Where keyboard focus must be before typing.
#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(rename_all = "snake_case")]
enum ExpectArg {
    /// `{"window_id": <id>}`: that window, from `desktop_state`, must have focus.
    WindowId(u64),
    /// `{"app_id": "<app_id>"}`: the focused window must have that `app_id`.
    AppId(String),
    /// `"none"`: don't check, for example to type into a shell panel or a dialog that
    /// holds keyboard focus outside the windows.
    None,
}

impl From<ExpectArg> for Expect {
    fn from(expect: ExpectArg) -> Self {
        match expect {
            ExpectArg::WindowId(id) => Self::Window(id),
            ExpectArg::AppId(app_id) => Self::App(app_id),
            ExpectArg::None => Self::Unchecked,
        }
    }
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
struct KeyArgs {
    /// Modifiers and one key joined by `+`, such as `ctrl+shift+t`, `Return` or `alt+F4`.
    /// The key is an XKB keysym name (`a`, `Return`, `Escape`, `F5`, `slash`, `Page_Down`);
    /// the modifiers are `shift`, `ctrl`, `alt`, `altgr` and `super`.
    combo: String,
    /// Where keyboard focus must be; checked before typing.
    expect: ExpectArg,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
struct TypeTextArgs {
    /// At most 100 characters; split longer text over several calls.
    text: String,
    /// Where keyboard focus must be; checked before typing.
    expect: ExpectArg,
}

#[derive(Debug, Clone)]
pub(crate) struct Server {
    env: Env,
    /// None without `NIRI_SOCKET`.
    events: Option<EventStream>,
    audit: Audit,
    desk: Desk,
    /// Read once at startup.
    policy: Loaded,
    /// Decided once, because the tool list depends on it.
    noctalia_installed: bool,
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl Server {
    /// `shell_status` exists only when `noctalia` is on `PATH`, so the tool list stays
    /// fixed for the session.
    pub(crate) fn new(env: Env, events: Option<EventStream>, audit: Audit) -> Self {
        let mut tool_router = Self::tool_router();
        let noctalia_installed = env.finds("noctalia");
        if !noctalia_installed {
            tool_router.remove_route("shell_status");
        }
        Self {
            desk: Desk::start(&env),
            policy: env.policy(),
            env,
            events,
            audit,
            noctalia_installed,
            tool_router,
        }
    }

    /// Readiness report: the niri instance, niri's version and whether this server
    /// supports it, whether niri's event stream is connected, the lock state, whether
    /// Noctalia is running, the audit log, and which required programs are on PATH. Call
    /// this first.
    #[tool(annotations(read_only_hint = true))]
    async fn status(
        &self,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.audited(&context, "status", Value::Null, async {
            structured(&self.report().await)
        })
        .await
    }

    /// Takes the exclusive lease on this niri desktop, which later action tools will
    /// require. Fails with `lease_held` naming the holder if another agent has it,
    /// `stopped` while the user's stop flag is set, `recovery_required` while input may be
    /// stuck, `read_only` when this build doesn't support the running niri or the policy
    /// file is invalid, and `screen_locked` while the screen is locked. Returns the holder; calling it again while holding the lease returns the
    /// same holder.
    #[tool(annotations(
        read_only_hint = false,
        destructive_hint = false,
        idempotent_hint = true,
        open_world_hint = false
    ))]
    async fn acquire_desktop(
        &self,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let label = session(&context);
        self.audited(&context, "acquire_desktop", Value::Null, async {
            let refusal = self.refusal().await;
            answer(
                self.desk
                    .acquire(&label, refusal)
                    .await
                    .map(|holder| serde_json::json!({ "holder": holder })),
            )
        })
        .await
    }

    /// Gives the lease up. `released` says whether this server held it; the user's stop
    /// flag also takes it back.
    #[tool(annotations(
        read_only_hint = false,
        destructive_hint = false,
        idempotent_hint = true,
        open_world_hint = false
    ))]
    async fn release_desktop(
        &self,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.audited(&context, "release_desktop", Value::Null, async {
            let released = self.desk.release().await;
            structured(&serde_json::json!({ "released": released }))
        })
        .await
    }

    /// Focuses a window. `accepted` says whether niri took the request; `observed` is
    /// `focused` once the window has keyboard focus, `timeout` if it didn't get it within
    /// five seconds, `interrupted` if focus went to another window meanwhile, or
    /// `uncertain` if niri's reply or event stream was lost. Requires the lease.
    #[tool(annotations(
        read_only_hint = false,
        destructive_hint = false,
        idempotent_hint = true,
        open_world_hint = false
    ))]
    async fn focus_window(
        &self,
        Parameters(args): Parameters<WindowArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let logged = serde_json::json!({ "id": args.id });
        let work = act::focus_window(self.niri(), args.id);
        self.act(&context, "focus_window", logged, work).await
    }

    /// Focuses a workspace by its id, on whichever output it is. Results as for
    /// `focus_window`; focus moving to one of the workspace's own windows is expected.
    /// Requires the lease.
    #[tool(annotations(
        read_only_hint = false,
        destructive_hint = false,
        idempotent_hint = true,
        open_world_hint = false
    ))]
    async fn focus_workspace(
        &self,
        Parameters(args): Parameters<WorkspaceArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let logged = serde_json::json!({ "id": args.id });
        let work = act::focus_workspace(self.niri(), args.id);
        self.act(&context, "focus_workspace", logged, work).await
    }

    /// Starts an app from a policy preset, whose command is fixed by the user. `observed`
    /// counts the new windows with the preset's `app_id`: `one` with its id in `windows`,
    /// `ambiguous` with several, or `none` within five seconds. With `reuse`, one existing
    /// window is focused instead (`focused`), and several give `ambiguous` without starting
    /// anything. Never call it again because a window didn't show up; look first. Requires
    /// the lease.
    #[tool(annotations(
        read_only_hint = false,
        destructive_hint = false,
        idempotent_hint = false,
        open_world_hint = false
    ))]
    async fn launch(
        &self,
        Parameters(args): Parameters<LaunchArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let logged = serde_json::to_value(&args).unwrap_or(Value::Null);
        let niri = self.niri();
        let preset = self.policy.preset(&args.preset);
        let work = async move { act::launch(niri, preset?, args.reuse).await };
        self.act(&context, "launch", logged, work).await
    }

    /// Asks a window to close, as its close button would. `observed` is `closed`, or
    /// `pending` if it is still open after five seconds, for example behind an
    /// unsaved-changes dialog; nothing forces it. Requires the lease.
    #[tool(annotations(
        read_only_hint = false,
        destructive_hint = true,
        idempotent_hint = false,
        open_world_hint = false
    ))]
    async fn close_window(
        &self,
        Parameters(args): Parameters<WindowArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let logged = serde_json::json!({ "id": args.id });
        let work = act::close_window(self.niri(), args.id);
        self.act(&context, "close_window", logged, work).await
    }

    /// Moves the pointer onto a pixel of a screenshot, to hover. `observed` is `sent` once
    /// niri has handled the motion; take a screenshot to see what it did. Requires the lease
    /// and a `screenshot_ref` taken under it; fails with `ref_invalid` if the ref is unknown,
    /// over a minute old, its output changed, or the pixel is outside its image, and with
    /// `app_denied` while the focused window's app is on the policy's deny list.
    #[tool(annotations(
        read_only_hint = false,
        destructive_hint = false,
        idempotent_hint = true,
        open_world_hint = false
    ))]
    async fn pointer_move(
        &self,
        Parameters(args): Parameters<PointArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let logged = serde_json::to_value(&args).unwrap_or(Value::Null);
        let gesture = Gesture::Move(ImagePx {
            x: args.x,
            y: args.y,
        });
        let id = args.screenshot_ref;
        self.point(&context, logged, id, gesture).await
    }

    /// Clicks a pixel of a screenshot: moves there, then presses and releases the button
    /// `count` times. Results and failures as for `pointer_move`. A click can change focus
    /// or do anything the app does on a click; take a screenshot before the next action.
    #[tool(annotations(
        read_only_hint = false,
        destructive_hint = true,
        idempotent_hint = false,
        open_world_hint = false
    ))]
    async fn click(
        &self,
        Parameters(args): Parameters<ClickArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let logged = serde_json::to_value(&args).unwrap_or(Value::Null);
        let gesture = Gesture::Click {
            at: ImagePx {
                x: args.x,
                y: args.y,
            },
            button: args.button.into(),
            count: args.count,
        };
        let id = args.screenshot_ref;
        self.point(&context, logged, id, gesture).await
    }

    /// Drags from one pixel of a screenshot to another: presses the button at `from`,
    /// moves to `to` in ten steps over about a quarter of a second, and releases it there.
    /// Results and failures as for `pointer_move`.
    #[tool(annotations(
        read_only_hint = false,
        destructive_hint = true,
        idempotent_hint = false,
        open_world_hint = false
    ))]
    async fn drag(
        &self,
        Parameters(args): Parameters<DragArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let logged = serde_json::to_value(&args).unwrap_or(Value::Null);
        let gesture = Gesture::Drag {
            from: args.from.into(),
            to: args.to.into(),
            button: args.button.into(),
        };
        let id = args.screenshot_ref;
        self.point(&context, logged, id, gesture).await
    }

    /// Scrolls with the mouse wheel over a pixel of a screenshot, by whole notches, as a
    /// wheel does. Results and failures as for `pointer_move`.
    #[tool(annotations(
        read_only_hint = false,
        destructive_hint = false,
        idempotent_hint = false,
        open_world_hint = false
    ))]
    async fn scroll(
        &self,
        Parameters(args): Parameters<ScrollArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let logged = serde_json::to_value(&args).unwrap_or(Value::Null);
        let gesture = Gesture::Scroll {
            at: ImagePx {
                x: args.x,
                y: args.y,
            },
            notches_x: args.notches_x,
            notches_y: args.notches_y,
        };
        let id = args.screenshot_ref;
        self.point(&context, logged, id, gesture).await
    }

    /// Presses a key combination in the focused app, as one press and release with its
    /// modifiers held, such as `ctrl+s`. It goes to the app, not to niri: niri's own
    /// keybinds don't fire from it. `expect` names the window or app that must have
    /// keyboard focus (`focus_mismatch` otherwise), or `"none"` to skip the check.
    /// `observed` is `sent`, or `interrupted` if focus moved meanwhile; take a screenshot to
    /// see what the key did. Refused with `app_denied` for an app on the policy's deny
    /// list. Requires the lease.
    #[tool(annotations(
        read_only_hint = false,
        destructive_hint = true,
        idempotent_hint = false,
        open_world_hint = false
    ))]
    async fn key(
        &self,
        Parameters(args): Parameters<KeyArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let logged = serde_json::to_value(&args).unwrap_or(Value::Null);
        let typing = Typing::Key(args.combo);
        self.type_input(&context, logged, typing, args.expect.into())
            .await
    }

    /// Types text into the focused app, at most 100 characters per call. `expect`, the
    /// results and the refusals are as for `key`; text over the limit is refused with
    /// `text_too_long`. The text is never logged. Requires the lease.
    #[tool(annotations(
        read_only_hint = false,
        destructive_hint = true,
        idempotent_hint = false,
        open_world_hint = false
    ))]
    async fn type_text(
        &self,
        Parameters(args): Parameters<TypeTextArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        // The length, never the text.
        let logged = serde_json::json!({
            "text_len": args.text.chars().count(),
            "expect": args.expect,
        });
        let typing = Typing::Text(args.text);
        self.type_input(&context, logged, typing, args.expect.into())
            .await
    }

    /// niri's outputs (monitors) by connector name: modes, logical position and size,
    /// scale and transform, as niri reports them.
    #[tool(annotations(read_only_hint = true))]
    async fn outputs(
        &self,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let outputs = niri::outputs(self.env.niri_socket.as_deref());
        self.audited(&context, "outputs", Value::Null, async {
            answer(outputs.await)
        })
        .await
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
        self.audited(&context, "desktop_state", Value::Null, async {
            answer(desktop.await)
        })
        .await
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
        // Targets, sizes and formats only: nothing in these arguments is content.
        let logged = serde_json::to_value(&args).unwrap_or(Value::Null);
        self.audited(&context, "screenshot", logged, async {
            let request = match args.request() {
                Ok(request) => request,
                Err(message) => return Ok(invalid(&message)),
            };
            match self.capture(request).await {
                Ok(shot) => image(&shot),
                Err(CallError::InvalidArguments(message)) => Ok(invalid(&message)),
                Err(CallError::Tool(error)) => Ok(error.into_result()),
            }
        })
        .await
    }

    /// Noctalia's status: whether its bar is visible, which panel is open, and whether
    /// its lock screen is up. `noctalia_unavailable` when Noctalia isn't answering.
    #[tool(annotations(read_only_hint = true))]
    async fn shell_status(
        &self,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let status = noctalia::status(&self.env);
        self.audited(&context, "shell_status", Value::Null, async {
            answer(status.await)
        })
        .await
    }

    /// The clipboard's text, read with `wl-paste`. `text` is null, with a `reason`, when
    /// nothing is copied (`nothing_copied`) or nothing copied is text (`no_text`).
    #[tool(annotations(read_only_hint = true))]
    async fn clipboard_read(
        &self,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.audited(&context, "clipboard_read", Value::Null, async {
            answer(clipboard::read_text().await)
        })
        .await
    }
}

impl Server {
    fn niri(&self) -> act::Niri<'_> {
        act::Niri {
            socket: self.env.niri_socket.as_deref(),
            events: self.events.as_ref(),
        }
    }

    /// Runs a pointer gesture through the action gate, aimed through the ref named `id`,
    /// which is looked up only once the gate has passed.
    async fn point(
        &self,
        context: &RequestContext<RoleServer>,
        logged: Value,
        id: String,
        gesture: Gesture,
    ) -> Result<CallToolResult, ErrorData> {
        let display = self.env.wayland_socket();
        let work = async {
            let input = Input {
                niri: self.niri(),
                display: display.as_deref(),
                runtime: self.desk.runtime()?,
                policy: &self.policy,
            };
            pointer::point(input, self.desk.shot(&id), gesture).await
        };
        self.act(context, gesture.tool(), logged, work).await
    }

    /// Runs one `wtype` call through the action gate.
    async fn type_input(
        &self,
        context: &RequestContext<RoleServer>,
        logged: Value,
        typing: Typing,
        expect: Expect,
    ) -> Result<CallToolResult, ErrorData> {
        let display = self.env.wayland_socket();
        let tool = typing.tool();
        let work = async {
            let input = Input {
                niri: self.niri(),
                display: display.as_deref(),
                runtime: self.desk.runtime()?,
                policy: &self.policy,
            };
            keyboard::type_input(input, typing, expect).await
        };
        self.act(context, tool, logged, work).await
    }

    /// Takes a screenshot and, while this server holds the lease, keeps it as a ref that
    /// the result names.
    async fn capture(&self, request: observe::Request) -> Result<observe::Screenshot, CallError> {
        let lease = self.desk.ref_lease();
        let connection = self.events.as_ref().and_then(EventStream::connection);
        let taken = Instant::now();
        let mut shot = observe::screenshot(self.env.niri_socket.as_deref(), &request).await?;
        if let (Some(lease), Some(connection)) = (lease, connection) {
            let kept = Shot::of(&shot, taken, connection);
            shot.metadata.screenshot_ref = self.desk.remember(lease, kept);
        }
        Ok(shot)
    }

    /// Why this server may not take the lease or act now, from the readiness report.
    async fn refusal(&self) -> Option<ToolError> {
        let report = self.report().await;
        policy::refuse_control(report.facts(&self.policy))
    }

    /// Runs one action through the desk's gate, with a screenshot when its outcome is in
    /// doubt, and logs it with what was accepted and observed.
    async fn act(
        &self,
        context: &RequestContext<RoleServer>,
        tool: &str,
        args: Value,
        work: impl Future<Output = Result<Outcome, CallError>>,
    ) -> Result<CallToolResult, ErrorData> {
        self.record(context, Call::action(tool), args, async {
            // Boxed, because the readiness report and the action's wait make large futures.
            let refusal = Box::pin(self.refusal());
            let evidence =
                |outcome| Box::pin(act::with_evidence(outcome, |request| self.capture(request)));
            match self.desk.act(refusal, Box::pin(work), evidence).await {
                Ok(evidenced) => outcome(&evidenced),
                Err(CallError::InvalidArguments(message)) => Ok(invalid(&message)),
                Err(CallError::Tool(error)) => Ok(error.into_result()),
            }
        })
        .await
    }

    /// The readiness report, as `status` returns it.
    async fn report(&self) -> status::Status {
        let event_stream = self
            .events
            .as_ref()
            .map_or(StreamState::Disconnected, EventStream::state);
        let sources = status::Sources {
            event_stream: Some(event_stream),
            audit: &self.audit,
            noctalia_installed: self.noctalia_installed,
            lease: self.desk.status(),
            policy: &self.policy,
        };
        status::collect(&self.env, sources).await
    }

    /// Runs one tool's work until it finishes or the client cancels the request, then
    /// writes the call to the audit log. rmcp only cancels the request's token and keeps
    /// running the handler, so cancelling drops `work`, and with it any connection, wait or
    /// child process it holds.
    async fn audited(
        &self,
        context: &RequestContext<RoleServer>,
        tool: &str,
        args: Value,
        work: impl Future<Output = Result<CallToolResult, ErrorData>>,
    ) -> Result<CallToolResult, ErrorData> {
        self.record(context, Call::start(tool), args, work).await
    }

    async fn record(
        &self,
        context: &RequestContext<RoleServer>,
        call: Call<'_>,
        args: Value,
        work: impl Future<Output = Result<CallToolResult, ErrorData>>,
    ) -> Result<CallToolResult, ErrorData> {
        let result = unless_cancelled(context.ct.cancelled(), work)
            .await
            .and_then(|result| result);
        let session = session(context);
        let instance = self.env.instance();
        let caller = Caller {
            session: &session,
            instance: instance.as_deref(),
        };
        self.audit.finish(&call, caller, &args, &result);
        result
    }
}

/// The session label: the MCP client's name and this server's PID, such as
/// `claude-code/4711`.
fn session(context: &RequestContext<RoleServer>) -> String {
    let client = context.peer.peer_info().map_or_else(
        || "unknown".to_owned(),
        |info| info.client_info.name.clone(),
    );
    format!("{client}/{}", std::process::id())
}

fn answer(result: Result<impl Serialize, ToolError>) -> Result<CallToolResult, ErrorData> {
    match result {
        Ok(value) => structured(&value),
        Err(error) => Ok(error.into_result()),
    }
}

/// The image first, then the metadata as structured content and its text.
fn image(shot: &observe::Screenshot) -> Result<CallToolResult, ErrorData> {
    let data = base64::engine::general_purpose::STANDARD.encode(&shot.image);
    let mut result = structured(&shot.metadata)?;
    result
        .content
        .insert(0, ContentBlock::image(data, shot.metadata.mime_type));
    Ok(result)
}

/// An action's outcome as structured content and its text, then its screenshot, if any.
fn outcome(evidenced: &act::Evidenced) -> Result<CallToolResult, ErrorData> {
    let mut result = structured(&evidenced.outcome)?;
    if let (Some(image), Some(metadata)) = (&evidenced.image, &evidenced.outcome.screenshot) {
        let data = base64::engine::general_purpose::STANDARD.encode(image);
        result
            .content
            .push(ContentBlock::image(data, metadata.mime_type));
    }
    Ok(result)
}

/// Arguments that don't fit the desktop. rmcp reports arguments that don't fit the schema
/// as a tool result with `isError` and plain text, so the model can correct the call; this
/// gives both kinds of argument mistake that one shape.
fn invalid(message: &str) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(format!(
        "invalid arguments: {message}"
    ))])
}

#[expect(
    clippy::unused_async_trait_impl,
    reason = "rmcp's tool_handler generates an async list_tools without an await"
)]
#[tool_handler(
    router = self.tool_router,
    name = "niri-computer-use",
    instructions = "View and act on a niri desktop; follow the `niri-computer-use` skill. Start with `status`. Use `desktop_state` for windows and workspaces and `outputs` for the monitor layout; take a `screenshot` only when you need to see pixels. `clipboard_read` returns the clipboard's text, and `shell_status`, when Noctalia is installed, its panel and lock state. To act, call `acquire_desktop`, then one action at a time (`focus_window`, `focus_workspace`, `launch`, `close_window`), reading `accepted` and `observed` before the next; never retry an action on your own, and call `release_desktop` when done. Failures carry a stable `error` name and the upstream `detail`; a mistake in the arguments comes back as a plain-text error to correct."
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
        () = cancelled => Err(ErrorData::internal_error(CANCELLED, None)),
        done = work => Ok(done),
    }
}

fn structured(value: &impl Serialize) -> Result<CallToolResult, ErrorData> {
    let value = serde_json::to_value(value)
        .map_err(|error| ErrorData::internal_error(error.to_string(), None))?;
    Ok(CallToolResult::structured(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_close_window_is_destructive_and_only_actions_change_anything() {
        let hints = |tool: rmcp::model::Tool| {
            let annotations = tool.annotations.unwrap();
            (
                annotations.read_only_hint,
                annotations.destructive_hint,
                annotations.idempotent_hint,
            )
        };
        let (no, yes) = (Some(false), Some(true));
        for (tool, expected) in [
            (Server::acquire_desktop_tool_attr(), (no, no, yes)),
            (Server::release_desktop_tool_attr(), (no, no, yes)),
            (Server::focus_window_tool_attr(), (no, no, yes)),
            (Server::focus_workspace_tool_attr(), (no, no, yes)),
            (Server::launch_tool_attr(), (no, no, no)),
            (Server::close_window_tool_attr(), (no, yes, no)),
        ] {
            let name = tool.name.clone();
            assert_eq!(hints(tool), expected, "{name}");
        }
        for tool in [
            Server::status_tool_attr(),
            Server::outputs_tool_attr(),
            Server::desktop_state_tool_attr(),
            Server::screenshot_tool_attr(),
            Server::clipboard_read_tool_attr(),
            Server::shell_status_tool_attr(),
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

    #[test]
    fn argument_mistakes_are_plain_text_tool_errors() {
        let result = invalid("no enabled output named \"NOPE\"");
        assert_eq!(result.is_error, Some(true));
        assert_eq!(result.structured_content, None);
        assert_eq!(
            serde_json::to_value(&result.content).unwrap(),
            serde_json::json!([{
                "type": "text",
                "text": "invalid arguments: no enabled output named \"NOPE\""
            }])
        );
        // These fail in rmcp's own deserialization, which gives the same shape.
        for bad in [
            serde_json::json!({}),
            serde_json::json!({"target": "focused_output", "format": "gif"}),
            serde_json::json!({"target": "region", "region": {"x": 0}}),
            serde_json::json!({"target": "focused_output", "max_width": -1}),
        ] {
            assert!(
                serde_json::from_value::<ScreenshotArgs>(bad.clone()).is_err(),
                "{bad}"
            );
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
    fn shell_status_exists_only_with_noctalia_installed() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = crate::test_support::fresh_dir("noctalia");
        let noctalia = dir.join("noctalia");
        std::fs::write(&noctalia, "").unwrap();
        std::fs::set_permissions(&noctalia, std::fs::Permissions::from_mode(0o755)).unwrap();
        let installed = Env {
            path: Some(dir.clone().into_os_string()),
            ..Env::default()
        };
        assert!(
            Server::new(installed, None, Audit::new(None))
                .tool_router
                .has_route("shell_status")
        );
        let absent = Server::new(Env::default(), None, Audit::new(None));
        assert!(!absent.tool_router.has_route("shell_status"));
        assert!(absent.tool_router.has_route("status"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_server_names_itself_and_gives_instructions() {
        let server = Server::new(Env::default(), None, Audit::new(None));
        let info = server.get_info();
        assert_eq!(info.server_info.name, "niri-computer-use");
        assert!(
            info.instructions
                .is_some_and(|text| text.contains("status"))
        );
        assert!(info.capabilities.tools.is_some());
    }
}
