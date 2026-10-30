--!nonstrict
-- The glow: a soft frame the size of the output, on the desktop layer behind
-- windows, so it shows in niri's gaps. It breathes in the accent colour while
-- an agent drives, holds the tertiary colour while one asks, and is gone
-- otherwise.

local look = require("./lib/look.luau")

local BREATH_MS = 2400
-- Alpha changes smaller than this aren't drawn, so a breath is about 25 frames.
local STEP = 0.04

local view = noctalia.state.get("desk")
local phaseMs = 0
local drawn = nil

local function setting(key: string, fallback)
  local value = noctalia.getConfig(key)
  if value == nil then
    return fallback
  end
  return value
end

-- The focused output in logical pixels, as Noctalia gives it, or a common size before
-- outputs are known. Its `scale` is wl_output's whole number, 2 for 1.25 on niri.
local function size(): (number, number)
  for _, output in noctalia.outputs() or {} do
    if output.focused then
      return output.width, output.height
    end
  end
  return 1920, 1080
end

local function alpha(status: string): number
  if status == "driving" then
    local wave = 0.5 + 0.5 * math.cos(phaseMs / BREATH_MS * 2 * math.pi)
    return math.floor((0.35 + 0.65 * wave) / STEP + 0.5) * STEP
  elseif status == "asking" then
    return 0.8
  end
  return 0
end

local function render()
  local status = if view then view.status else "idle"
  desktopWidget.setNeedsFrameTick(status == "driving")
  local a = alpha(status)
  local key = `{status}/{a}`
  if key == drawn then
    return
  end
  drawn = key
  local width, height = size()
  desktopWidget.render(ui.box({
    width = width,
    height = height,
    radius = 0,
    border = if a > 0 then `{look.role(status)}/{string.format("%.2f", a)}` else "#00000000",
    borderWidth = setting("glow_width", 6),
    softness = setting("glow_width", 6) * 2,
  }))
end

noctalia.state.watch("desk", function(value)
  view = value
  render()
end)

function onFrameTick(deltaMs)
  phaseMs = (phaseMs + (tonumber(deltaMs) or 16)) % BREATH_MS
  render()
end

function update()
  drawn = nil
  render()
end

render()
