--!nonstrict
-- The overlay: a small robot and a caption beside where the agent acts, on a layer
-- above windows that clicks and keys pass through. It draws what lib/stage.luau
-- decides from the `live` state the service publishes. The layer covers the output
-- and the robot is placed inside it; a target on another output moves the layer there.

local stage = require("./lib/stage.luau")

local ENTRY = "robot"

local view = noctalia.state.get("live")
local st = stage.new()
local drawn = nil
local interval = 1000
-- The output this instance is on. `layer.outputName()` is nil in a state watcher.
local here = layer.outputName()

-- Each output's logical size, for one no action has named yet. Noctalia gives logical
-- sizes; its `scale` is wl_output's whole number, 2 for 1.25 or 1.5 on niri.
local function areas()
  local out = {}
  for _, output in noctalia.outputs() or {} do
    out[output.name] = { width = output.width, height = output.height }
  end
  return out
end

local function tree(frame, scale: number)
  local sprite = {}
  -- Every frame stays mounted, so each image is read once, and only one shows.
  for _, pose in stage.POSES do
    table.insert(sprite, ui.image({
      key = pose,
      path = `robot/{pose}.svg`,
      width = stage.SPRITE,
      height = stage.SPRITE,
      visible = pose == frame.pose,
    }))
  end
  local group = { ui.row({ key = "sprite" }, sprite) }
  if frame.caption then
    local caption = ui.row({
      key = "caption",
      width = frame.width,
      height = 20,
      paddingH = 8,
      radius = 10,
      align = "center",
      fill = frame.role,
    }, {
      ui.label({ text = frame.caption, fontSize = 11, color = `on_{frame.role}`, maxLines = 1, maxWidth = frame.width - 16 }),
    })
    table.insert(group, if frame.side == "left" then 1 else #group + 1, caption)
  end
  -- Noctalia multiplies every size in the tree by the UI scale, the spacers' too.
  return ui.column({ align = "start" }, {
    ui.row({ key = "top", width = 1, height = frame.y / scale }, {}),
    ui.row({ align = "center" }, {
      ui.row({ key = "left", width = frame.x / scale, height = 1 }, {}),
      ui.row({ key = "group", gap = stage.INNER, align = "center", opacity = frame.opacity }, group),
    }),
  })
end

local function schedule(frame, nowMs: number)
  local wanted = if frame.nextMs then math.clamp(frame.nextMs - nowMs, 16, 1000) else 1000
  if wanted ~= interval then
    interval = wanted
    noctalia.setUpdateInterval(wanted)
  end
end

local function draw()
  here = layer.outputName() or here
  local nowMs = noctalia.nowMs()
  local known = areas()
  local scale = tonumber(noctalia.getSetting("accessibility.ui_scale")) or 1
  local frame = stage.step(st, view, nowMs, {
    scale = scale,
    focused = noctalia.focusedOutputName() or here,
    areas = known,
    tr = noctalia.tr,
  })
  schedule(frame, nowMs)
  if frame.visible and here and frame.output ~= here then
    -- niri and Noctalia name outputs alike. This instance ends, and the one Noctalia
    -- starts on the target's output draws the robot there.
    if known[frame.output] then
      noctalia.layer.configure(ENTRY, { output = frame.output })
    end
    return
  end
  local key = stage.key(frame)
  if key ~= drawn then
    drawn = key
    layer.render(if frame.visible then tree(frame, scale) else ui.row({}, {}))
  end
end

noctalia.state.watch("live", function(value)
  view = value
  draw()
end)

function update()
  draw()
end

draw()
