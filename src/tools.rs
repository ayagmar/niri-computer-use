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
use crate::error::{CANCELLED, CallError, ErrorName, ToolError};
use crate::input::Input;
use crate::input::keyboard::{self, Expect, Typing};
use crate::input::pointer::{self, Button, Gesture};
use crate::niri::events::{EventStream, StreamState};
use crate::observe::{DEFAULT_MAX_WIDTH, Format, Rect, Target};
use crate::policy::{self, Loaded};
use crate::refs::Shot;
use crate::{Env, clipboard, niri, noctalia, observe, settle, status, wait};

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
    /// With true, the result also has a screenshot of the focused output, taken once the
    /// screen stopped changing, so no separate `screenshot` call is needed. Defaults to
    /// false.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    screenshot: bool,
}

/// What `wait_for` waits for.
#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
#[serde(rename_all = "snake_case")]
enum UntilArg {
    /// `{"window": {"app_id": "<app_id>", "title": "<text>"}}`: a window with that `app_id`
    /// and a title containing that text exists; give either or both.
    Window(WindowMatchArg),
    /// `{"closed": <id>}`: the window with that id from `desktop_state` is gone.
    Closed(u64),
    /// `{"title": {"window_id": <id>, "contains": "<text>"}}`: that window's title contains
    /// the text.
    Title(TitleArg),
    /// `"screen_stable"`: the focused output stopped changing.
    ScreenStable,
}

#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
struct WindowMatchArg {
    #[schemars(with = "String", default)]
    app_id: Option<String>,
    /// Text the title contains.
    #[schemars(with = "String", default)]
    title: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
struct TitleArg {
    window_id: u64,
    contains: String,
}

impl From<UntilArg> for wait::Until {
    fn from(until: UntilArg) -> Self {
        match until {
            UntilArg::Window(window) => Self::Window {
                app_id: window.app_id,
                title: window.title,
            },
            UntilArg::Closed(id) => Self::Closed(id),
            UntilArg::Title(title) => Self::Title {
                window_id: title.window_id,
                contains: title.contains,
            },
            UntilArg::ScreenStable => Self::ScreenStable,
        }
    }
}

const fn default_timeout_ms() -> u32 {
    10_000
}

/// The longest `wait_for`.
const MAX_WAIT_MS: u32 = 30_000;

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
struct WaitForArgs {
    until: UntilArg,
    /// How long to wait, in milliseconds: 100 to 30000. Defaults to 10000.
    #[serde(default = "default_timeout_ms")]
    #[schemars(range(min = 100, max = 30_000))]
    timeout_ms: u32,
    /// With true, the result also has a screenshot of the focused output, taken once the
    /// screen stopped changing, so no separate `screenshot` call is needed. Defaults to
    /// false.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    screenshot: bool,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
struct ReleaseArgs {
    /// true to give keyboard focus back to `users_window`, the window that had it when you
    /// took the lease, before releasing; false to leave focus where your task put it, such
    /// as on an app the user asked you to open.
    restore_focus: bool,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
struct WorkspaceArgs {
    /// A workspace id from `desktop_state`, not its index.
    id: u64,
    /// With true, the result also has a screenshot of the focused output, taken once the
    /// screen stopped changing, so no separate `screenshot` call is needed. Defaults to
    /// false.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    screenshot: bool,
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
    /// With true, the result also has a screenshot of the focused output, taken once the
    /// screen stopped changing, so no separate `screenshot` call is needed. Defaults to
    /// false.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    screenshot: bool,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
struct PanelArgs {
    /// A Noctalia panel: `control-center`, `wallpaper` or `tray-drawer`.
    panel: String,
    /// With true, the result also has a screenshot of the focused output, taken once the
    /// screen stopped changing, so no separate `screenshot` call is needed. Defaults to
    /// false.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    screenshot: bool,
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
    /// With true, the result also has a screenshot of the focused output, taken once the
    /// screen stopped changing, so no separate `screenshot` call is needed. Defaults to
    /// false.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    screenshot: bool,
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
    /// With true, the result also has a screenshot of the focused output, taken once the
    /// screen stopped changing, so no separate `screenshot` call is needed. Defaults to
    /// false.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    screenshot: bool,
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
    /// With true, the result also has a screenshot of the focused output, taken once the
    /// screen stopped changing, so no separate `screenshot` call is needed. Defaults to
    /// false.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    screenshot: bool,
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
    /// With true, the result also has a screenshot of the focused output, taken once the
    /// screen stopped changing, so no separate `screenshot` call is needed. Defaults to
    /// false.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    screenshot: bool,
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
    /// 1 to 16 combinations, pressed in order, such as `["ctrl+l"]` or `["Down", "Down",
    /// "Return"]`. Each is modifiers and one key joined by `+`, such as `ctrl+shift+t`,
    /// `Return` or `alt+F4`. The key is an XKB keysym name (`a`, `Return`, `Escape`, `F5`,
    /// `slash`, `Page_Down`); the modifiers are `shift`, `ctrl`, `alt`, `altgr` and `super`.
    #[schemars(length(min = 1, max = 16))]
    keys: Vec<String>,
    /// Where keyboard focus must be; checked before typing.
    expect: ExpectArg,
    /// With true, the result also has a screenshot of the focused output, taken once the
    /// screen stopped changing, so no separate `screenshot` call is needed. Defaults to
    /// false.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    screenshot: bool,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
struct TypeTextArgs {
    /// 1 to 1000 characters.
    text: String,
    /// Where keyboard focus must be; checked before typing.
    expect: ExpectArg,
    /// With true, press `Return` after the text, but only if all of it went out and focus
    /// stayed. Defaults to false.
    #[serde(default)]
    submit: bool,
    /// With true, the result also has a screenshot of the focused output, taken once the
    /// screen stopped changing, so no separate `screenshot` call is needed. Defaults to
    /// false.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    screenshot: bool,
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
    /// The `shell_*` tools exist only when `noctalia` is on `PATH`, so the tool list
    /// stays fixed for the session.
    pub(crate) fn new(env: Env, events: Option<EventStream>, audit: Audit) -> Self {
        let mut tool_router = Self::tool_router();
        let noctalia_installed = env.finds("noctalia");
        if !noctalia_installed {
            for tool in SHELL_TOOLS {
                tool_router.remove_route(tool);
            }
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
    /// file is invalid, and `screen_locked` while the screen is locked. Returns the holder
    /// and `users_window`, the window that had keyboard focus, which `release_desktop` can
    /// give focus back to; calling it again while holding the lease returns the same.
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
            let focused = niri::waiter(self.events.as_ref())
                .await
                .ok()
                .and_then(|waiter| waiter.view().focused_window());
            let acquired = self.desk.acquire(&label, refusal, focused).await;
            answer(acquired.map(|holder| {
                serde_json::json!({
                    "holder": holder,
                    "users_window": self.desk.users_window(),
                })
            }))
        })
        .await
    }

    /// Gives the lease up. `released` says whether this server held it; the user's stop
    /// flag also takes it back. With `restore_focus`, keyboard focus first goes back to
    /// `users_window`, the window the user was on when you took the lease, and `restored`
    /// says how that went (`focused`, `closed` if the window is gone, or an error). Restore
    /// focus unless the task was to leave another window in front.
    #[tool(annotations(
        read_only_hint = false,
        destructive_hint = false,
        idempotent_hint = true,
        open_world_hint = false
    ))]
    async fn release_desktop(
        &self,
        Parameters(args): Parameters<ReleaseArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let logged = serde_json::to_value(&args).unwrap_or(Value::Null);
        self.audited(&context, "release_desktop", logged, async {
            let users_window = self.desk.users_window();
            let restored = match (args.restore_focus, users_window) {
                (true, Some(id)) => Some(self.restore(id).await),
                _ => None,
            };
            structured(&Release {
                users_window,
                restored,
                released: self.desk.release().await,
            })
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
        let logged = serde_json::to_value(&args).unwrap_or(Value::Null);
        let work = act::focus_window(self.niri(), args.id);
        self.act(
            &context,
            Asked {
                tool: "focus_window",
                logged,
                shoot: args.screenshot,
            },
            work,
        )
        .await
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
        let logged = serde_json::to_value(&args).unwrap_or(Value::Null);
        let work = act::focus_workspace(self.niri(), args.id);
        self.act(
            &context,
            Asked {
                tool: "focus_workspace",
                logged,
                shoot: args.screenshot,
            },
            work,
        )
        .await
    }

    /// Starts an app from a policy preset, whose command is fixed by the user. `observed`
    /// counts the new windows with the preset's `app_id`: `one` with its id in `windows`,
    /// `ambiguous` with several, or `none` within five seconds. With `reuse`, one existing
    /// window is focused instead (`focused`), and several give `ambiguous` without starting
    /// anything. Never call it again because a window didn't show up; look first. With no
    /// preset for the app, ask the user to add one rather than starting it another way.
    /// Requires the lease.
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
        let (reuse, shoot) = (args.reuse, args.screenshot);
        let work = async move { act::launch(niri, preset?, reuse).await };
        self.act(
            &context,
            Asked {
                tool: "launch",
                logged,
                shoot,
            },
            work,
        )
        .await
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
        let logged = serde_json::to_value(&args).unwrap_or(Value::Null);
        let work = act::close_window(self.niri(), args.id);
        self.act(
            &context,
            Asked {
                tool: "close_window",
                logged,
                shoot: args.screenshot,
            },
            work,
        )
        .await
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
        let aim = Aim {
            id: args.screenshot_ref,
            shoot: args.screenshot,
        };
        self.point(&context, logged, aim, gesture).await
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
        let aim = Aim {
            id: args.screenshot_ref,
            shoot: args.screenshot,
        };
        self.point(&context, logged, aim, gesture).await
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
        let aim = Aim {
            id: args.screenshot_ref,
            shoot: args.screenshot,
        };
        self.point(&context, logged, aim, gesture).await
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
        let aim = Aim {
            id: args.screenshot_ref,
            shoot: args.screenshot,
        };
        self.point(&context, logged, aim, gesture).await
    }

    /// Presses key combinations in the focused app, in order, each as one press and
    /// release with its modifiers held, such as `["ctrl+l"]` or `["Down", "Down",
    /// "Return"]`. They go to the app, not to niri: niri's own keybinds don't fire from
    /// them. `expect` names the window or app that must have keyboard focus
    /// (`focus_mismatch` otherwise), or `"none"` to skip the check. `observed` is `sent`, or
    /// `interrupted` if focus moved; then the keys after that aren't pressed and `pressed`
    /// counts the ones that were. Pass `screenshot: true` to see what the keys did.
    /// Refused with `app_denied` for an app on the policy's deny list. Requires the lease.
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
        let typing = Typing::Keys(args.keys);
        let keying = Keying {
            typing,
            expect: args.expect.into(),
            shoot: args.screenshot,
        };
        self.type_input(&context, logged, keying).await
    }

    /// Types text into the focused app, up to 1000 characters, sent in parts of 100.
    /// `expect`, the results and the refusals are as for `key`. If focus moves during a
    /// part, the rest isn't typed: `observed` is `interrupted` and `typed` counts the
    /// characters sent. A failed call's detail says how much was typed before it; text
    /// over the limit is refused with `text_too_long` and nothing is typed. To send a
    /// message, pass `submit: true`: `Return` is pressed only once all of the text went out,
    /// and `submitted` says whether it was; never press Enter yourself after a call that
    /// stopped early. The text is never logged. Requires the lease.
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
        let mut logged = serde_json::json!({
            "text_len": args.text.chars().count(),
            "expect": args.expect,
        });
        flag(&mut logged, "submit", args.submit);
        flag(&mut logged, "screenshot", args.screenshot);
        let typing = Typing::Text {
            text: args.text,
            submit: args.submit,
        };
        let keying = Keying {
            typing,
            expect: args.expect.into(),
            shoot: args.screenshot,
        };
        self.type_input(&context, logged, keying).await
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
    /// layout coordinates, and the scale from logical pixels to image pixels, plus a
    /// `screenshot_ref` for the pointer tools while you hold the lease. It waits for an
    /// action still running to finish and excludes this server's next action during
    /// capture, not external input or redraws. Call it after the action's result, not
    /// alongside it; better, pass `screenshot: true` to the action itself. Prefer
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
            match self.desk.observe(self.capture(request)).await {
                Ok(shot) => image(&shot),
                Err(CallError::InvalidArguments(message)) => Ok(invalid(&message)),
                Err(CallError::Tool(error)) => Ok(error.into_result()),
            }
        })
        .await
    }

    /// Waits until something happens on the desktop, instead of polling with screenshots:
    /// a window appears (`window`, by `app_id` and title text), a window closes
    /// (`closed`), a window's title contains some text (`title`), or the focused output
    /// stops changing (`screen_stable`). A condition already true returns at once.
    /// `observed` is `met`, with the matching `windows`, `timeout`, or `uncertain` if
    /// niri's event stream was lost. Changes nothing and needs no lease.
    #[tool(annotations(read_only_hint = true))]
    async fn wait_for(
        &self,
        Parameters(args): Parameters<WaitForArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let logged = waited_for(&args);
        self.audited(&context, "wait_for", logged, async {
            if !(100..=MAX_WAIT_MS).contains(&args.timeout_ms) {
                return Ok(invalid("`timeout_ms` must be 100 to 30000"));
            }
            let limit = std::time::Duration::from_millis(args.timeout_ms.into());
            let until = wait::Until::from(args.until);
            if let Err(message) = until.check() {
                return Ok(invalid(&message));
            }
            match self.wait(&until, limit, args.screenshot).await {
                Ok(waited) => waited_result(waited),
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

    /// Opens a Noctalia panel: `control-center`, `wallpaper` or `tray-drawer`; any other
    /// panel is refused with `panel_not_allowed`. `observed` is `opened` once Noctalia
    /// reports it open, `timeout` if it doesn't within two seconds, or `uncertain` if
    /// Noctalia's reply was lost; `shell.active_panel` is the panel open at the end. An
    /// open panel holds keyboard focus, so type into it with `expect: "none"`. Fails with
    /// `noctalia_unavailable` when Noctalia isn't answering. Requires the lease.
    #[tool(annotations(
        read_only_hint = false,
        destructive_hint = false,
        idempotent_hint = true,
        open_world_hint = false
    ))]
    async fn shell_open(
        &self,
        Parameters(args): Parameters<PanelArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let logged = serde_json::to_value(&args).unwrap_or(Value::Null);
        let (env, niri) = (&self.env, self.niri());
        let shoot = args.screenshot;
        let work = async move { act::shell::open(env, niri, policy::panel(&args.panel)?).await };
        self.act(
            &context,
            Asked {
                tool: "shell_open",
                logged,
                shoot,
            },
            work,
        )
        .await
    }

    /// Closes a Noctalia panel opened with `shell_open`. `observed` is `closed` once
    /// Noctalia no longer reports it open; otherwise as for `shell_open`. Requires the
    /// lease.
    #[tool(annotations(
        read_only_hint = false,
        destructive_hint = false,
        idempotent_hint = true,
        open_world_hint = false
    ))]
    async fn shell_close(
        &self,
        Parameters(args): Parameters<PanelArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let logged = serde_json::to_value(&args).unwrap_or(Value::Null);
        let (env, niri) = (&self.env, self.niri());
        let shoot = args.screenshot;
        let work = async move { act::shell::close(env, niri, policy::panel(&args.panel)?).await };
        self.act(
            &context,
            Asked {
                tool: "shell_close",
                logged,
                shoot,
            },
            work,
        )
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

    /// Runs a pointer gesture through the action gate, aimed through the ref `aim` names,
    /// which is looked up only once the gate has passed.
    async fn point(
        &self,
        context: &RequestContext<RoleServer>,
        logged: Value,
        aim: Aim,
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
            pointer::point(input, self.desk.shot(&aim.id), gesture).await
        };
        self.act(
            context,
            Asked {
                tool: gesture.tool(),
                logged,
                shoot: aim.shoot,
            },
            work,
        )
        .await
    }

    /// Runs a keyboard tool's `wtype` calls through the action gate.
    async fn type_input(
        &self,
        context: &RequestContext<RoleServer>,
        logged: Value,
        keying: Keying,
    ) -> Result<CallToolResult, ErrorData> {
        let display = self.env.wayland_socket();
        let Keying {
            typing,
            expect,
            shoot,
        } = keying;
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
        self.act(
            context,
            Asked {
                tool,
                logged,
                shoot,
            },
            work,
        )
        .await
    }

    /// Waits for `until`, then with `shoot` adds a screenshot taken once the screen stopped
    /// changing. Waiting for the screen first lets a running action of this server end.
    async fn wait(
        &self,
        until: &wait::Until,
        limit: std::time::Duration,
        screenshot: bool,
    ) -> Result<(wait::Report, Option<observe::Screenshot>), CallError> {
        let capture = || self.capture(observe::Request::focused());
        if *until == wait::Until::ScreenStable {
            let (report, last) = self.desk.observe(wait::screen(capture, limit)).await?;
            return Ok((report, screenshot.then_some(last)));
        }
        let report = wait::window(self.events.as_ref(), until, limit).await?;
        if !screenshot {
            return Ok((report, None));
        }
        match self
            .desk
            .observe(settle::screenshot(capture, settle::LIMIT))
            .await
        {
            Ok(shot) => Ok((report, Some(shot))),
            Err(CallError::Tool(error)) => Ok((
                wait::Report {
                    screenshot_error: Some(error),
                    ..report
                },
                None,
            )),
            Err(mistake @ CallError::InvalidArguments(_)) => Err(mistake),
        }
    }

    /// Focuses the user's window `id` through the action gate, as the outcome or the error.
    async fn restore(&self, id: u64) -> Value {
        let refusal = Box::pin(self.refusal());
        let work = Box::pin(act::refocus(self.niri(), id));
        let restored = match self.desk.act(refusal, work, std::future::ready).await {
            Ok(outcome) => serde_json::to_value(outcome),
            Err(CallError::Tool(error)) => serde_json::to_value(error),
            Err(CallError::InvalidArguments(message)) => {
                serde_json::to_value(ToolError::new(ErrorName::UpstreamError, message))
            }
        };
        restored.unwrap_or(Value::Null)
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

    /// Runs one action through the desk's gate, with a screenshot when `shoot` asks for
    /// one or its outcome is in doubt, and logs it with what was accepted and observed.
    async fn act(
        &self,
        context: &RequestContext<RoleServer>,
        asked: Asked<'_>,
        work: impl Future<Output = Result<Outcome, CallError>>,
    ) -> Result<CallToolResult, ErrorData> {
        let Asked {
            tool,
            logged,
            shoot,
        } = asked;
        self.record(context, Call::action(tool), logged, async {
            // Boxed, because the readiness report and the action's wait make large futures.
            let refusal = Box::pin(self.refusal());
            let evidence = |outcome| Box::pin(self.evidence(outcome, shoot));
            match self.desk.act(refusal, Box::pin(work), evidence).await {
                Ok(evidenced) => outcome(&evidenced),
                Err(CallError::InvalidArguments(message)) => Ok(invalid(&message)),
                Err(CallError::Tool(error)) => Ok(error.into_result()),
            }
        })
        .await
    }

    /// The outcome with the screenshot `shoot` asks for, or the one an outcome in doubt gets.
    async fn evidence(&self, outcome: Outcome, shoot: bool) -> act::Evidenced {
        act::with_evidence(outcome, shoot, |request| self.capture(request)).await
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

/// `wait_for`'s arguments for the audit log: lengths of the text to match, never the text,
/// since titles can hold anything.
fn waited_for(args: &WaitForArgs) -> Value {
    let until = match &args.until {
        UntilArg::Window(window) => serde_json::json!({"window": {
            "app_id": window.app_id,
            "title_len": window.title.as_ref().map(|title| title.chars().count()),
        }}),
        UntilArg::Closed(id) => serde_json::json!({ "closed": id }),
        UntilArg::Title(title) => serde_json::json!({"title": {
            "window_id": title.window_id,
            "contains_len": title.contains.chars().count(),
        }}),
        UntilArg::ScreenStable => serde_json::json!("screen_stable"),
    };
    let mut logged = serde_json::json!({ "until": until, "timeout_ms": args.timeout_ms });
    flag(&mut logged, "screenshot", args.screenshot);
    logged
}

/// Adds `name: true` to logged arguments when `set`; a flag left false isn't logged.
fn flag(logged: &mut Value, name: &str, set: bool) {
    if let (true, Some(fields)) = (set, logged.as_object_mut()) {
        fields.insert(name.to_owned(), Value::Bool(true));
    }
}

/// `wait_for`'s report as structured content and its text, then its screenshot, if any.
fn waited_result(
    (mut report, shot): (wait::Report, Option<observe::Screenshot>),
) -> Result<CallToolResult, ErrorData> {
    let Some(shot) = shot else {
        return structured(&report);
    };
    report.screenshot = Some(shot.metadata);
    let mut result = structured(&report)?;
    let data = base64::engine::general_purpose::STANDARD.encode(&shot.image);
    let mime = report
        .screenshot
        .as_ref()
        .map_or("image/jpeg", |m| m.mime_type);
    result.content.push(ContentBlock::image(data, mime));
    Ok(result)
}

/// A pointer tool's screenshot ref, and whether it asked for a screenshot after.
/// What `release_desktop` returns.
#[derive(Serialize)]
struct Release {
    users_window: Option<u64>,
    /// How giving focus back went, when it was asked for and there was a window.
    #[serde(skip_serializing_if = "Option::is_none")]
    restored: Option<Value>,
    released: bool,
}

/// An action call as the agent made it: the tool, its arguments as the audit log keeps
/// them, and whether it asked for a screenshot of the result.
struct Asked<'a> {
    tool: &'a str,
    logged: Value,
    shoot: bool,
}

struct Aim {
    id: String,
    shoot: bool,
}

/// A keyboard tool's call: what to type, where, and whether it asked for a screenshot
/// after.
struct Keying {
    typing: Typing,
    expect: Expect,
    shoot: bool,
}

/// The tools that exist only with Noctalia installed.
const SHELL_TOOLS: [&str; 3] = ["shell_status", "shell_open", "shell_close"];

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
    instructions = "View and act on the user's niri desktop; load the `niri-computer-use` skill first. Start with `status`. Use `desktop_state` for windows and workspaces and `outputs` for the monitor layout; take a `screenshot` only when you need to see pixels. `clipboard_read` returns the clipboard's text, and `shell_status`, when Noctalia is installed, its panel and lock state. To act, call `acquire_desktop`, then one action at a time (`focus_window`, `focus_workspace`, `launch`, `close_window`, with Noctalia `shell_open` and `shell_close`, and for input `pointer_move`, `click`, `drag` and `scroll` with a fresh `screenshot_ref`, or `key` and `type_text` with `expect`), reading `accepted` and `observed` before the next call. To see an action's result, pass it `screenshot: true` rather than sending a `screenshot` alongside it, and use `wait_for` rather than repeated screenshots to wait for a window or for the screen to settle. Never retry an action on your own, send messages with `type_text`'s `submit: true` rather than a separate Enter, and start apps only through `launch` presets. When done, call `release_desktop` with `restore_focus: true` to put focus back on the user's window. Failures carry a stable `error` name and the upstream `detail`; a mistake in the arguments comes back as a plain-text error to correct."
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
            (Server::shell_open_tool_attr(), (no, no, yes)),
            (Server::shell_close_tool_attr(), (no, no, yes)),
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
    fn the_shell_tools_exist_only_with_noctalia_installed() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = crate::test_support::fresh_dir("noctalia");
        let noctalia = dir.join("noctalia");
        std::fs::write(&noctalia, "").unwrap();
        std::fs::set_permissions(&noctalia, std::fs::Permissions::from_mode(0o755)).unwrap();
        let installed = Env {
            path: Some(dir.clone().into_os_string()),
            ..Env::default()
        };
        let installed = Server::new(installed, None, Audit::new(None));
        let absent = Server::new(Env::default(), None, Audit::new(None));
        for tool in SHELL_TOOLS {
            assert!(installed.tool_router.has_route(tool), "{tool}");
            assert!(!absent.tool_router.has_route(tool), "{tool}");
        }
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
