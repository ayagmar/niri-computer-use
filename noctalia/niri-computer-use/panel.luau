--!nonstrict
-- The panel: who drives the desktop, in which project and for how long, a Stop
-- or Resume button, and the calls agents made, newest at the bottom.

local look = require("./lib/look.luau")
local actions = require("./lib/actions.luau")
local time = require("./lib/time.luau")

local view = noctalia.state.get("desk")
local open = false
-- Bumped to jump the feed to its newest call: on every open, and from Latest.
local toLatest = 0
-- Whether the feed shows its newest call; false once the user scrolls up.
local atLatest = true
-- Whether the agent list shows every agent rather than the first few; folded
-- again on every open.
local agentsOpen = false
local render: () -> ()

local function tr(key: string, vars)
  return noctalia.tr(key, vars)
end

local function header(now: number)
  local status = view.status
  return ui.row({ gap = 12, align = "center", padding = 14, radius = 12, fill = "surface_variant/0.6" }, {
    ui.glyph({ name = look.glyph(status), size = 28, color = look.role(status) }),
    ui.column({ gap = 2, flexGrow = 1 }, {
      ui.label({ text = look.title(view, tr), fontSize = 15, fontWeight = "bold", maxLines = 2 }),
      ui.label({ text = look.detail(view, now, tr), color = "on_surface_variant", maxLines = 3 }),
    }),
    ui.button({
      glyph = "settings",
      variant = "ghost",
      tooltip = tr("panel.settings"),
      onClick = function()
        noctalia.openSettings()
      end,
    }),
  })
end

local function controls()
  if view.stopped then
    return ui.button({
      text = tr("panel.resume"),
      glyph = "player-play",
      variant = "primary",
      enabled = not view.inputDirty,
      tooltip = if view.inputDirty then tr("status.stopped.stuck") else nil,
      onClick = function()
        actions.run("resume")
      end,
    })
  end
  return ui.button({
    text = tr("panel.stop"),
    glyph = "hand-stop",
    variant = "destructive",
    enabled = view.status ~= "missing",
    tooltip = tr("panel.stop_hint"),
    onClick = function()
      actions.run("stop")
    end,
  })
end

local function who(call): string
  if call.project then
    return tr("notify.who_in", { client = call.client, project = call.project })
  end
  return call.client
end

-- A call's row. Who made it shows only where the agent changes from the row
-- above, and an error always shows.
local function row(call, now: number, showWho: boolean)
  local at = time.parse(call.ts)
  local meta = if showWho then who(call) else nil
  if call.error then
    meta = if meta then `{meta} · {call.error}` else call.error
  end
  local lines = { ui.label({ text = call.text, maxLines = 1 }) }
  if meta then
    table.insert(lines, ui.label({ text = meta, fontSize = 11, color = "on_surface_variant", maxLines = 1 }))
  end
  return ui.row({ key = call.key, gap = 10, align = "center", paddingV = 5 }, {
    ui.glyph({ name = call.glyph, size = 16, color = look.outcomeRole(call.outcome) }),
    ui.column({ gap = 1, flexGrow = 1 }, lines),
    ui.label({
      text = if at then time.ago(now - at, tr) else "",
      fontSize = 11,
      color = "on_surface_variant",
    }),
  })
end

-- The server PID a session label such as `claude-code/4711` ends with.
local function serverPid(session: string?): number?
  return if session then tonumber(session:match("/(%d+)$")) else nil
end

local function agentRow(agent, driver: number?)
  local text = if agent.project then tr("panel.agent_in", { agent = agent.name, project = agent.project }) else agent.name
  local note, role = nil, "on_surface_variant"
  if agent.replaced then
    note, role = tr("panel.agent_old"), "tertiary"
  elseif agent.pid == driver then
    note, role = tr("panel.agent_driving"), "primary"
  end
  return ui.row({ key = `agent-{agent.pid}`, gap = 10, align = "center", paddingV = 3 }, {
    ui.glyph({ name = if agent.replaced then "alert-triangle" else "robot", size = 16, color = role }),
    ui.label({ text = text, maxLines = 1, flexGrow = 1 }),
    ui.label({ text = note or "", fontSize = 11, color = role }),
  })
end

-- How many agent rows show before the rest fold into "+N more", so the feed
-- keeps its room with many agents connected.
local AGENTS_SHOWN = 3
-- The unfolded list scrolls within this height, about four rows.
local AGENTS_HEIGHT = 112

-- The driver first, then the rest as `watch` lists them.
local function byDriver(agents, driver: number?)
  local sorted = {}
  for _, agent in agents do
    if agent.pid == driver then
      table.insert(sorted, agent)
    end
  end
  for _, agent in agents do
    if agent.pid ~= driver then
      table.insert(sorted, agent)
    end
  end
  return sorted
end

-- Folds or unfolds the list: "+N more" with how many of those are on an old
-- build, or "Show fewer".
local function foldRow(hidden)
  local old = 0
  for _, agent in hidden do
    if agent.replaced then
      old += 1
    end
  end
  local items = {
    ui.button({
      text = if agentsOpen then tr("panel.agents_fewer") else tr("panel.agents_more", { n = #hidden }),
      variant = "ghost",
      controlSize = "sm",
      onClick = function()
        agentsOpen = not agentsOpen
        render()
      end,
    }),
  }
  if old > 0 and not agentsOpen then
    table.insert(items, ui.label({ text = tr("panel.agents_old", { n = old }), fontSize = 11, color = "tertiary" }))
  end
  return ui.row({ key = "agents-fold", gap = 8, align = "center" }, items)
end

-- Every agent with a server on this desktop, the driver and old builds marked.
local function agentList()
  local agents = view.agents or {}
  if #agents == 0 then
    return nil
  end
  local driver = serverPid(view.lease and view.lease.session)
  local sorted = byDriver(agents, driver)
  local rows = {}
  for _, agent in sorted do
    table.insert(rows, agentRow(agent, driver))
  end
  local title = ui.label({ text = tr("panel.agents"), fontWeight = "bold" })
  -- Folding a single agent into "+1 more" would save nothing.
  if #rows <= AGENTS_SHOWN + 1 then
    -- A list that grows past it again starts folded.
    agentsOpen = false
    return ui.column({ gap = 2 }, { title, table.unpack(rows) })
  end
  local hidden = table.move(sorted, AGENTS_SHOWN + 1, #sorted, 1, {})
  if agentsOpen then
    return ui.column({ gap = 2 }, { title, ui.scroll({ key = "agents", height = AGENTS_HEIGHT, gap = 2 }, rows), foldRow(hidden) })
  end
  local shown = { title }
  for i = 1, AGENTS_SHOWN do
    table.insert(shown, rows[i])
  end
  table.insert(shown, foldRow(hidden))
  return ui.column({ gap = 2 }, shown)
end

local function feed(now: number)
  local calls = view.calls or {}
  if #calls == 0 then
    return ui.column({ flexGrow = 1, align = "center", justify = "center" }, {
      ui.label({ text = tr("panel.empty"), color = "on_surface_variant" }),
    })
  end
  local rows = {}
  local previous = nil
  for _, call in calls do
    table.insert(rows, row(call, now, call.session ~= previous))
    previous = call.session
  end
  return ui.scroll({
    key = "feed",
    flexGrow = 1,
    gap = 0,
    stickToBottom = true,
    scrollToBottomRev = toLatest,
    onScroll = "onFeedScroll",
  }, rows)
end

-- Jumps the feed back to its newest call.
local function latestButton()
  return ui.button({
    text = tr("panel.latest"),
    glyph = "arrow-down",
    variant = "secondary",
    controlSize = "sm",
    onClick = function()
      toLatest += 1
      atLatest = true
      render()
    end,
  })
end

-- The feed's heading, with a button back to the newest call while the user
-- reads older ones.
local function heading()
  local row = { ui.label({ text = tr("panel.activity"), fontWeight = "bold", flexGrow = 1 }) }
  if not atLatest then
    table.insert(row, latestButton())
  end
  table.insert(row, controls())
  return ui.row({ gap = 8, align = "center" }, row)
end

function render()
  if not open then
    return
  end
  if view == nil then
    panel.render(ui.column({ padding = 16 }, { ui.label({ text = tr("status.starting.detail") }) }))
    return
  end
  local now = noctalia.nowMs()
  local children = { header(now) }
  local list = agentList()
  if list then
    table.insert(children, list)
  end
  table.insert(children, heading())
  table.insert(children, feed(now))
  -- flexGrow makes the column the panel's height, so the feed scrolls inside it
  -- rather than running past the panel's bottom edge.
  panel.render(ui.column({ flexGrow = 1, gap = 12, padding = 16 }, children))
end

noctalia.state.watch("desk", function(value)
  view = value
  render()
end)

-- The feed scrolled: `offset` and `maxOffset` arrive as strings.
function onFeedScroll(offset: string, maxOffset: string)
  local was = atLatest
  atLatest = (tonumber(maxOffset) or 0) - (tonumber(offset) or 0) < 4
  if atLatest ~= was then
    render()
  end
end

function onOpen(_context)
  open = true
  toLatest += 1
  atLatest = true
  agentsOpen = false
  panel.setWantsSecondTicks(true)
  render()
end

function onClose()
  open = false
end

function update()
  render()
end
