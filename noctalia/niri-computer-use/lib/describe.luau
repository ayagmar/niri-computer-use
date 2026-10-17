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
  if type(args.action) == "table" then
    for name in args.action do
      return tostring(name)
    end
  end
  return "?"
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
