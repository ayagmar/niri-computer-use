--!nonstrict
-- The overlay: a small robot and a caption beside where the agent acts, on a layer
-- above windows that clicks and keys pass through. It draws what lib/stage.luau
-- decides from the `live` state the service publishes. The layer covers the output
-- and the robot is placed inside it; a target on another output moves the layer there.

local art = require("./lib/art.luau")
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

-- The robot's size from the `overlay_size` setting, medium for one it doesn't know.
local function size(): number
  local named = noctalia.getConfig("overlay_size")
  return if type(named) == "string" and stage.SIZES[named] then stage.SIZES[named] else stage.MEDIUM
end

-- The frame of each pose in the theme's colours: written once per palette into the
-- plugin's data directory, through a file of this instance's own, and every other file
-- there removed: other palettes' frames, and files an instance left half written. The
-- frames as drawn when a colour or a write is missing.
local function themed(): { [string]: string }
  local bundled = {}
  for _, pose in stage.POSES do
    bundled[pose] = `robot/{pose}.svg`
  end
  local palette = art.palette(noctalia.getColor)
  local data = noctalia.pluginDataDir()
  if palette == nil or data == nil then
    return bundled
  end
  local dir = `{data}/robot`
  if not noctalia.mkdirAll(dir) then
    return bundled
  end
  local key = art.key(palette)
  local mine = `.{noctalia.nowMs()}.tmp`
  local paths = {}
  for _, pose in stage.POSES do
    local path = `{dir}/{pose}-{key}.svg`
    if not noctalia.fileExists(path) then
      local template = noctalia.readFile(bundled[pose])
      local written = template ~= nil
        and noctalia.writeFile(path .. mine, art.paint(template, palette))
        and noctalia.renameFile(path .. mine, path)
      if not written then
        noctalia.removeFile(path .. mine)
        return bundled
      end
    end
    paths[pose] = path
  end
  local kept = `-{key}.svg`
  for _, name in noctalia.listDir(dir) or {} do
    if name:sub(-#kept) ~= kept then
      noctalia.removeFile(`{dir}/{name}`)
    end
  end
  return paths
end

local frames = themed()

-- A spacer, `width` by `height` in the tree's units.
local function gap(width: number, height: number)
  return ui.row({ width = width, height = height }, {})
end

-- The robot, with its room to hop above it, and its caption beside it.
local function group(frame, d, box)
  local sprite = {}
  -- Every frame stays mounted, so each image is read once, and only one shows.
  for _, pose in stage.POSES do
    table.insert(sprite, ui.image({
      key = pose,
      path = frames[pose],
      width = d.size,
      height = d.size,
      visible = pose == frame.pose,
    }))
  end
  local parts = {
    ui.column({ key = "sprite", width = d.size, height = d.size + d.lift, align = "start" }, {
      gap(1, d.lift - frame.lift),
      ui.row({}, sprite),
    }),
  }
  if frame.caption then
    local caption = ui.row({
      key = "caption",
      width = frame.width,
      height = d.captionHeight,
      paddingH = d.pad,
      radius = d.captionHeight / 2,
      align = "center",
      fill = frame.role,
    }, {
      ui.label({
        text = frame.caption,
        fontSize = d.font,
        color = `on_{frame.role}`,
        maxLines = 1,
        maxWidth = frame.width - 2 * d.pad,
      }),
    })
    -- Level with the robot below its room to hop.
    local beside = ui.column({ align = "start" }, { gap(1, d.lift), caption })
    table.insert(parts, if frame.side == "left" then 1 else #parts + 1, beside)
  end
  return ui.row({
    key = "group",
    width = box.width / d.scale,
    height = box.height / d.scale,
    gap = d.inner,
    align = "center",
    opacity = frame.opacity,
  }, parts)
end

-- A click's ring: a circle's outline in the accent colour.
local function ring(frame, d, box)
  local size = box.width / d.scale
  return ui.box({
    key = "ring",
    width = size,
    height = size,
    radius = size / 2,
    border = "primary",
    borderWidth = 2,
    opacity = frame.ring.opacity,
  })
end

-- The robot and a click's ring placed on the layer, side by side along the axis that
-- keeps them apart: the tree has no way to overlap them.
local function tree(frame, scale: number)
  local d = stage.dims(frame.size, scale)
  local boxes, axis = stage.layout(frame, d)
  local nodes = {}
  for _, box in boxes do
    nodes[box.key] = if box.key == "ring" then ring(frame, d, box) else group(frame, d, box)
  end
  -- Noctalia multiplies every size in the tree by the UI scale, the spacers' too.
  local placed = {}
  if axis == "x" then
    local top, at = math.huge, 0
    for _, box in boxes do
      top = math.min(top, box.y)
    end
    for _, box in boxes do
      table.insert(placed, gap((box.x - at) / scale, 1))
      table.insert(placed, ui.column({ align = "start" }, { gap(1, (box.y - top) / scale), nodes[box.key] }))
      at = box.x + box.width
    end
    return ui.column({ align = "start" }, { gap(1, top / scale), ui.row({ align = "start" }, placed) })
  end
  local at = 0
  for _, box in boxes do
    table.insert(placed, gap(1, (box.y - at) / scale))
    table.insert(placed, ui.row({ align = "start" }, { gap(box.x / scale, 1), nodes[box.key] }))
    at = box.y + box.height
  end
  return ui.column({ align = "start" }, placed)
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
    size = size(),
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
