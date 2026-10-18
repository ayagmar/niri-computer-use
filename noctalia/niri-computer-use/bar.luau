--!nonstrict
-- The bar capsule: a robot that is dim while no agent holds the desktop, and
-- breathes in the accent colour while one drives it, with the agent's name.
-- Click opens the panel; right-click stops agents, or resumes them.

local look = require("./lib/look.luau")
local actions = require("./lib/actions.luau")
local time = require("./lib/time.luau")
local desk = require("./lib/desk.luau")

local PANEL = "ayagmar/niri-computer-use:panel"
-- One breath, and how often the bar redraws while it breathes.
local BREATH_MS = 2400
local FRAME_MS = 120

local view = noctalia.state.get("desk")

local function setting(key: string, fallback)
  local value = noctalia.getConfig(key)
  if value == nil then
    return fallback
  end
  return value
end

local function tr(key: string, vars)
  return noctalia.tr(key, vars)
end

-- The accent at an alpha that rises and falls once per breath.
local function breathing(role: string): string
  local phase = (noctalia.nowMs() % BREATH_MS) / BREATH_MS
  local alpha = 0.55 + 0.45 * (0.5 + 0.5 * math.cos(phase * 2 * math.pi))
  return `{role}/{string.format("%.2f", alpha)}`
end

local function tooltip(status: string): string
  local lines = { look.title(view, tr), look.detail(view, noctalia.nowMs(), tr) }
  local last = view.calls and view.calls[#view.calls]
  if last then
    local at = time.parse(last.ts)
    local ago = if at then time.ago(noctalia.nowMs() - at, tr) else "?"
    table.insert(lines, tr("bar.last", { text = last.text, ago = ago }))
  end
  local agents = view.agents or {}
  local outdated = desk.outdated(agents)
  if outdated > 0 then
    table.insert(lines, tr("bar.agents_outdated", { n = #agents, old = outdated }))
  elseif #agents > 0 then
    table.insert(lines, tr("bar.agents", { n = #agents }))
  end
  table.insert(lines, tr(if status == "stopped" then "bar.hint_resume" else "bar.hint_stop"))
  return table.concat(lines, "\n")
end

local function render()
  local status = if view then view.status else "starting"
  local live = status == "driving"
  noctalia.setUpdateInterval(if live then FRAME_MS else 1000)
  if view == nil then
    barWidget.setVisible(false)
    return
  end
  barWidget.setVisible(status ~= "idle" or setting("show_when_idle", true))
  local role = look.role(status)
  local color = if live then breathing(role) else role
  local children = { ui.glyph({ name = look.glyph(status), color = color }) }
  if (live or status == "asking") and view.lease and setting("show_client", true) then
    table.insert(children, ui.label({ text = view.lease.client, color = color }))
  end
  local container = if barWidget.isVertical() then ui.column else ui.row
  barWidget.render(container({ gap = 6, align = "center" }, children))
  barWidget.setTooltip(tooltip(status))
end

noctalia.state.watch("desk", function(value)
  view = value
  render()
end)

function update()
  render()
end

function onClick()
  noctalia.togglePanel(PANEL)
end

function onRightClick()
  actions.run(if view and view.stopped then "resume" else "stop")
end

render()
