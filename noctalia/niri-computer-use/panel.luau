--!nonstrict
-- The panel: who drives the desktop, in which project and for how long, a Stop
-- or Resume button, and the calls agents made, newest at the bottom.

local look = require("./lib/look.luau")
local actions = require("./lib/actions.luau")
local time = require("./lib/time.luau")

local view = noctalia.state.get("desk")
local open = false

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
    enabled = view.status == "driving" or view.status == "asking" or view.status == "idle",
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
  return ui.scroll({ flexGrow = 1, gap = 0, stickToBottom = true }, rows)
end

local function render()
  if not open then
    return
  end
  if view == nil then
    panel.render(ui.column({ padding = 16 }, { ui.label({ text = tr("status.starting.detail") }) }))
    return
  end
  local now = noctalia.nowMs()
  panel.render(ui.column({ gap = 12, padding = 16 }, {
    header(now),
    ui.row({ gap = 8, align = "center" }, {
      ui.label({ text = tr("panel.activity"), fontWeight = "bold", flexGrow = 1 }),
      controls(),
    }),
    feed(now),
  }))
end

noctalia.state.watch("desk", function(value)
  view = value
  render()
end)

function onOpen(_context)
  open = true
  panel.setWantsSecondTicks(true)
  render()
end

function onClose()
  open = false
end

function update()
  render()
end
