--!nonstrict
-- The panel: who drives the desktop, in which project and for how long, a Stop
-- or Resume button, and the calls agents made, newest at the bottom.

local look = require("./lib/look.luau")
local actions = require("./lib/actions.luau")
local time = require("./lib/time.luau")
local feedRows = require("./lib/feed.luau")
local agentGroups = require("./lib/agents.luau")
local desk = require("./lib/desk.luau")

local view = noctalia.state.get("desk")
local open = false
-- Bumped to jump the feed to its newest call: on every open, and from Latest.
local toLatest = 0
-- Whether the feed shows its newest call; false once the user scrolls up.
local atLatest = true
-- Whether the agent list shows every agent rather than only its first; folded
-- again on every open.
local agentsOpen = false
local render: () -> ()

local function tr(key: string, vars)
  return noctalia.tr(key, vars)
end

-- Where Stop shows: while an agent holds the desktop, and while the plugin
-- can't see whether one does. Idle, there is nothing to stop, and without a
-- binary Stop couldn't run.
local STOPPABLE = { driving = true, asking = true, starting = true, offline = true, error = true }

-- Whether Stop can do something now: also while idle just after an agent gave
-- the desktop back, as agents do before taking it again.
local function stoppable(): boolean
  return STOPPABLE[view.status] or (view.status == "idle" and view.justReleased)
end

-- Resume once stopped, Stop where it can do something, otherwise nothing.
local function controls()
  if view.stopped then
    return ui.button({
      text = tr("panel.resume"),
      glyph = "player-play",
      variant = "primary",
      controlSize = "sm",
      enabled = not view.inputDirty,
      tooltip = if view.inputDirty then tr("status.stopped.stuck") else nil,
      onClick = function()
        actions.run("resume")
      end,
    })
  elseif not stoppable() then
    return nil
  end
  return ui.button({
    text = tr("panel.stop"),
    glyph = "hand-stop",
    variant = "destructive",
    controlSize = "sm",
    -- Set although it is the default: the host may reuse a held-back Resume for it.
    enabled = true,
    tooltip = tr("panel.stop_hint"),
    onClick = function()
      actions.run("stop")
    end,
  })
end

local function settingsButton()
  return ui.button({
    glyph = "settings",
    variant = "ghost",
    controlSize = "sm",
    tooltip = tr("panel.settings"),
    onClick = function()
      noctalia.openSettings()
    end,
  })
end

-- The status card: who drives where and for how long, Stop or Resume, and the
-- plugin's settings.
local function header(now: number)
  local status = view.status
  local children = {
    ui.glyph({ name = look.glyph(status), size = 28, color = look.role(status) }),
    ui.column({ gap = 2, flexGrow = 1 }, {
      ui.label({ text = look.title(view, tr), fontSize = 15, fontWeight = "bold", maxLines = 2 }),
      ui.label({ text = look.detail(view, now, tr), color = "on_surface_variant", maxLines = 3 }),
    }),
  }
  local control = controls()
  if control then
    table.insert(children, control)
  end
  table.insert(children, settingsButton())
  return ui.row({ gap = 12, align = "center", padding = 14, radius = 12, fill = "surface_variant/0.6" }, children)
end

local function who(call): string
  if call.project then
    return tr("notify.who_in", { client = call.client, project = call.project })
  end
  return call.client
end

-- A feed row. Who made it shows only where the agent changes from the row
-- above; an error, and the keys a folded row pressed, always show.
local function row(call, now: number, showWho: boolean)
  local at = time.parse(call.ts)
  local meta = {}
  if showWho then
    table.insert(meta, who(call))
  end
  if call.detail then
    table.insert(meta, call.detail)
  end
  if call.error then
    table.insert(meta, call.error)
  end
  local lines = { ui.label({ text = call.text, maxLines = 1 }) }
  if #meta > 0 then
    local text = table.concat(meta, " · ")
    table.insert(lines, ui.label({ text = text, fontSize = 11, color = "on_surface_variant", maxLines = 1 }))
  end
  return ui.row({ key = call.key, gap = 10, align = "center", paddingV = 5, tooltip = call.detail }, {
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

local function agentRow(group)
  local text = if group.project then tr("panel.agent_in", { agent = group.name, project = group.project }) else group.name
  local notes, role = {}, "on_surface_variant"
  if #group.pids > 1 then
    table.insert(notes, tr("panel.agent_sessions", { n = #group.pids }))
  end
  if group.driving then
    table.insert(notes, tr("panel.agent_driving"))
    role = "primary"
  end
  if group.replaced then
    table.insert(notes, tr("panel.agent_old"))
    role = "tertiary"
  end
  local pids = table.concat(group.pids, ", ")
  return ui.row({
    key = group.key,
    gap = 10,
    align = "center",
    paddingV = 3,
    tooltip = tr(if #group.pids > 1 then "panel.agent_pids" else "panel.agent_pid", { pids = pids }),
  }, {
    ui.glyph({ name = if group.replaced then "alert-triangle" else "robot", size = 16, color = role }),
    ui.label({ text = text, maxLines = 1, flexGrow = 1 }),
    ui.label({ text = table.concat(notes, " · "), fontSize = 11, color = role }),
  })
end

-- A list of up to this many rows shows whole; a longer one folds to its first
-- row, the driver while one drives, so the feed keeps its room.
local FOLD_PAST = 3
-- The unfolded list scrolls within this height, three rows and part of a fourth.
local AGENTS_HEIGHT = 112

-- "Connected agents", how many, and once how many of them run an old build.
local function agentsTitle(count: number, old: number)
  local items = {
    ui.label({ text = tr("panel.agents"), fontWeight = "bold" }),
    ui.label({ text = tostring(count), color = "on_surface_variant", flexGrow = 1 }),
  }
  if old > 0 then
    local key = if old == 1 then "panel.agents_old_one" else "panel.agents_old"
    table.insert(items, ui.label({ text = tr(key, { n = old }), fontSize = 11, color = "tertiary" }))
  end
  return ui.row({ gap = 6, align = "center" }, items)
end

-- Folds or unfolds the list: "and N more" agents, or "Show fewer".
local function foldRow(hidden: number)
  return ui.row({
    key = "agents-fold",
    gap = 10,
    align = "center",
    paddingV = 3,
    onClick = function()
      agentsOpen = not agentsOpen
      render()
    end,
  }, {
    ui.glyph({ name = if agentsOpen then "chevron-up" else "chevron-down", size = 16, color = "primary" }),
    ui.label({ text = if agentsOpen then tr("panel.agents_fewer") else tr("panel.agents_more", { n = hidden }), color = "primary" }),
  })
end

-- Every agent with a server on this desktop, the driver first.
local function agentList()
  local agents = view.agents or {}
  local groups = agentGroups.groups(agents, serverPid(view.lease and view.lease.session))
  -- A list that grows long again, even from none, starts folded.
  if #groups <= FOLD_PAST then
    agentsOpen = false
  end
  if #groups == 0 then
    return nil
  end
  local rows = {}
  for _, group in groups do
    table.insert(rows, agentRow(group))
  end
  local title = agentsTitle(#agents, desk.outdated(agents))
  if #groups <= FOLD_PAST then
    return ui.column({ gap = 2 }, { title, table.unpack(rows) })
  end
  local hidden = #agents - #groups[1].pids
  if agentsOpen then
    -- Noctalia sizes a scroll to its rows whatever its own height, so a column
    -- of that height holds it.
    local list = ui.column({ height = AGENTS_HEIGHT }, { ui.scroll({ key = "agents", flexGrow = 1, gap = 2 }, rows) })
    return ui.column({ gap = 2 }, { title, list, foldRow(hidden) })
  end
  return ui.column({ gap = 2 }, { title, rows[1], foldRow(hidden) })
end

local function feed(now: number)
  local calls = feedRows.rows(view.calls or {}, tr)
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
-- reads older ones. It keeps the button's height, so the feed doesn't jump
-- when it shows.
local function heading()
  local row = { ui.label({ text = tr("panel.activity"), fontWeight = "bold", flexGrow = 1 }) }
  if not atLatest then
    table.insert(row, latestButton())
  end
  return ui.row({ gap = 8, align = "center", minHeight = 32 }, row)
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
