--!strict
-- Says in words what one tool call did, from the audit fields `watch` forwards:
-- the tool, its argument metadata and its outcome. The audit log never holds
-- typed text or titles, so neither does anything here.

local M = {}

type Tr = (key: string, vars: { [string]: any }?) -> string

local CLIENTS = {
  ["claude-code"] = "Claude Code",
  ["claude-ai"] = "Claude",
  ["codex-mcp-client"] = "Codex",
  codex = "Codex",
  pi = "Pi",
  ["gemini-cli-mcp-client"] = "Gemini CLI",
  ["cursor-vscode"] = "Cursor",
  opencode = "opencode",
}

-- Agents by the name of their process, which `watch` reports for each server.
local PROCESSES = {
  claude = "Claude Code",
  codex = "Codex",
  pi = "Pi",
  gemini = "Gemini CLI",
  cursor = "Cursor",
  opencode = "opencode",
}

-- The agent's name for the process that started a server, such as `claude`.
function M.agent(process: string?): string
  return if process then PROCESSES[process] or process else "?"
end

-- The agent's name for a session label such as `claude-code/4711`.
function M.client(session: string?): string
  local name = if session then session:match("^(.*)/%d+$") or session else "?"
  return CLIENTS[name] or name
end

local GLYPHS = {
  acquire_desktop = "lock",
  release_desktop = "lock-open",
  screenshot = "photo",
  desktop_state = "layout",
  elements = "list-tree",
  wait_for = "hourglass",
  status = "circle-check",
  click = "pointer",
  pointer_move = "pointer",
  drag = "drag-drop",
  scroll = "arrows-vertical",
  type_text = "keyboard",
  paste = "clipboard",
  key = "command",
  focus = "focus-2",
  focus_window = "focus-2",
  focus_workspace = "layout-sidebar",
  launch = "apps",
  close_window = "x",
  niri_action = "layout",
  shell_panel = "layout-sidebar",
  noctalia = "terminal",
  activate_element = "hand-finger",
  set_element_text = "forms",
  clipboard_read = "clipboard",
}

function M.glyph(tool: string): string
  return GLYPHS[tool] or "robot"
end

local function words(name: string): string
  return (name:gsub("_", " "))
end

-- A name the server knew, such as a preset or a panel; the audit log keeps
-- only the length of one it didn't, as a table.
local function named(value: any, fallback: string): string
  return if type(value) == "string" then words(value) else fallback
end

local function clickKey(args): string
  if args.count == 2 then
    return "action.double_click"
  elseif args.button == "right" then
    return "action.right_click"
  end
  return "action.click"
end

local function typed(args, tr: Tr): string
  local key = if args.via == "paste" then "action.paste" else "action.type"
  local text = tr(key, { count = args.text_len or 0 })
  if args.submit then
    text ..= tr("action.and_enter")
  end
  return text
end

local function keys(args): string
  if type(args.keys) == "table" and type(args.keys[1]) == "string" then
    return table.concat(args.keys, ", ")
  end
  return named(args.keys or args.combo, "?")
end

local function actionName(args): string
  -- The log keeps an invalid action as its size only.
  if type(args.action) == "table" and args.action.category ~= "invalid_action" then
    for name in args.action do
      return tostring(name)
    end
  end
  return "?"
end

-- A Noctalia command as the log names it from Noctalia's own lists: the
-- plugin an event went to, Noctalia's description of the command, or its name.
local function noctalia(args, tr: Tr): string
  if type(args.plugin) == "string" then
    return tr("action.noctalia_plugin", { plugin = args.plugin })
  elseif type(args.description) == "string" then
    return args.description
  elseif type(args.command) == "string" then
    return tr("action.noctalia_command", { command = args.command })
  end
  return tr("action.noctalia")
end

local function focusKey(args): string
  if args.workspace_id then
    return "action.focus_workspace"
  end
  return "action.focus_window"
end

-- Each tool's sentence, from its arguments.
local SAY: { [string]: (args: any, tr: Tr) -> string } = {
  click = function(args, tr)
    return tr(clickKey(args))
  end,
  type_text = typed,
  paste = function(args, tr)
    return tr("action.paste", { count = args.text_len or 0 })
  end,
  key = function(args, tr)
    return tr("action.key", { keys = keys(args) })
  end,
  focus = function(args, tr)
    return tr(focusKey(args))
  end,
  focus_window = function(_, tr)
    return tr("action.focus_window")
  end,
  focus_workspace = function(_, tr)
    return tr("action.focus_workspace")
  end,
  launch = function(args, tr)
    return tr("action.launch", { preset = named(args.preset, "?") })
  end,
  niri_action = function(args, tr)
    return tr("action.niri_action", { action = actionName(args) })
  end,
  noctalia = noctalia,
  shell_panel = function(args, tr)
    local key = if args.open == false then "action.panel_close" else "action.panel_open"
    return tr(key, { panel = named(args.panel, "?") })
  end,
  activate_element = function(args, tr)
    return tr("action.activate", { role = named(args.role, "element") })
  end,
  screenshot = function(args, tr)
    if args.target ~= "region" then
      return tr("action.screenshot")
    end
    return tr(if type(args.screenshot_ref) == "string" then "action.zoom" else "action.region")
  end,
}

-- The sentence for a call, such as "Typed 12 characters".
function M.say(tool: string, args: any, tr: Tr): string
  local say = SAY[tool]
  if say then
    return say(if type(args) == "table" then args else {}, tr)
  end
  local key = "action." .. tool
  local text = tr(key)
  return if text == key then words(tool) else text
end

-- Keys a caption names: ones that type no character, so naming them shows nothing typed.
local NAMED_KEYS = {
  Return = true,
  Tab = true,
  Escape = true,
  BackSpace = true,
  Delete = true,
  Insert = true,
  Home = true,
  End = true,
  Page_Up = true,
  Page_Down = true,
  Left = true,
  Right = true,
  Up = true,
  Down = true,
  space = true,
}

-- Modifiers that still type a character with a letter: `shift+h` is a capital H.
local TYPING_MODIFIERS = { shift = true, altgr = true }

-- Whether `combo` types no character: its key is one of NAMED_KEYS or an F-key, or it
-- holds a modifier that makes it a shortcut.
local function namedKey(combo: string): boolean
  local parts = string.split(combo, "+")
  local key = parts[#parts]
  if NAMED_KEYS[key] or key:match("^F%d%d?$") then
    return true
  end
  for i = 1, #parts - 1 do
    if not TYPING_MODIFIERS[parts[i]:lower()] then
      return true
    end
  end
  return false
end

-- `keys ctrl+s, Return` when every combination holds a modifier or is a key that types
-- nothing, otherwise only how many, so a caption never spells out typed letters.
local function keysDoing(args, tr: Tr): string
  local list = args.keys
  local function counted(n: number): string
    return if n == 1 then tr("overlay.one_key") else tr("overlay.keys", { n = n })
  end
  if type(list) ~= "table" then
    return counted(1)
  end
  local count = if type(list.count) == "number" then list.count else #list
  for _, combo in list do
    if type(combo) ~= "string" or not namedKey(combo) then
      return counted(count)
    end
  end
  if #list == 0 or #list > 2 then
    return counted(count)
  end
  return tr("overlay.key", { keys = table.concat(list, ", ") })
end

local function inApp(text: string, app: string?, tr: Tr): string
  return if app then tr("overlay.in", { doing = text, app = app }) else text
end

local function clickDoing(args, tr: Tr): string
  if args.count == 2 then
    return tr("overlay.double_click")
  elseif args.button == "right" then
    return tr("overlay.right_click")
  end
  return tr("overlay.click")
end

-- Each acting tool's caption, from its logged arguments and the app of the window it acts on.
local DOING: { [string]: (args: any, app: string?, tr: Tr) -> string } = {
  click = function(args, _, tr)
    return clickDoing(args, tr)
  end,
  pointer_move = function(_, _, tr)
    return tr("overlay.move")
  end,
  drag = function(_, _, tr)
    return tr("overlay.drag")
  end,
  scroll = function(_, _, tr)
    return tr("overlay.scroll")
  end,
  type_text = function(args, app, tr)
    local key = if args.via == "paste" then "overlay.paste" else "overlay.type"
    return inApp(tr(key, { count = args.text_len or 0 }), app, tr)
  end,
  paste = function(args, app, tr)
    return inApp(tr("overlay.paste", { count = args.text_len or 0 }), app, tr)
  end,
  key = function(args, app, tr)
    return inApp(keysDoing(args, tr), app, tr)
  end,
  focus = function(args, app, tr)
    if args.workspace_id then
      return tr("overlay.workspace")
    end
    return if app then tr("overlay.focus_app", { app = app }) else tr("overlay.focus")
  end,
  focus_window = function(_, app, tr)
    return if app then tr("overlay.focus_app", { app = app }) else tr("overlay.focus")
  end,
  focus_workspace = function(_, _, tr)
    return tr("overlay.workspace")
  end,
  launch = function(args, _, tr)
    return tr("overlay.launch", { preset = named(args.preset, "?") })
  end,
  close_window = function(_, app, tr)
    return if app then tr("overlay.close_app", { app = app }) else tr("overlay.close")
  end,
  niri_action = function(args, _, tr)
    return tr("overlay.niri_action", { action = actionName(args) })
  end,
  shell_panel = function(args, _, tr)
    return tr("overlay.panel", { panel = named(args.panel, "?") })
  end,
  noctalia = function(_, _, tr)
    return tr("overlay.noctalia")
  end,
  activate_element = function(args, app, tr)
    return inApp(tr("overlay.activate", { role = named(args.role, "control") }), app, tr)
  end,
  set_element_text = function(_, app, tr)
    return inApp(tr("overlay.set_text"), app, tr)
  end,
}

-- Whether `tool` acts on the desktop, so the overlay shows it; reading tools never do.
function M.acts(tool: string): boolean
  return DOING[tool] ~= nil
end

-- The overlay's short caption for a call in progress, such as `typing 24 chars in kitty`,
-- from the same metadata as `say` and niri's app id for the window acted on.
function M.doing(tool: string, args: any, app: string?, tr: Tr): string?
  local doing = DOING[tool]
  if doing == nil then
    return nil
  end
  return doing(if type(args) == "table" then args else {}, if type(app) == "string" then app else nil, tr)
end

-- An error name in words, such as `focus mismatch`, for the overlay's caption.
function M.failure(name: string): string
  return words(name)
end

-- `ok`, `refused` or `error`, from the call's audit fields.
function M.outcome(call: any): string
  if type(call.error) == "string" then
    return "error"
  elseif call.accepted == false then
    return "refused"
  end
  return "ok"
end

return M
