--!strict
-- The robot's colours from the Noctalia palette. The frames in robot/ are drawn in a few
-- template colours; each maps to a palette role, and `paint` rewrites a frame in the
-- palette's colours. Images take no tint, so robot.luau writes the painted frames to
-- files. The antenna's tip stays #4c8dff, the colour the nested overlay case finds the
-- robot by. No noctalia calls: robot.luau passes `getColor`.

local M = {}

-- Template colour -> palette role, in the order the key lists them.
M.ROLES = {
  { colour = "#e8e8ec", role = "surface_container_highest" }, -- body, and the eyes on the visor
  { colour = "#2a2a33", role = "on_surface" }, -- lines and the visor
  { colour = "#f2a541", role = "primary" }, -- the chest light and what the robot carries
  { colour = "#e5484d", role = "error" }, -- the stop sign
}

-- Colours no theme changes.
M.ANTENNA = "#4c8dff"
M.FIXED = { M.ANTENNA }

export type Palette = { [string]: string }

local function hex(value: any): string?
  if type(value) ~= "string" or not value:match("^#%x%x%x%x%x%x$") then
    return nil
  end
  local lower = value:lower()
  -- A theme colour equal to the antenna's would be found as the antenna too.
  return if lower == M.ANTENNA then "#4c8dfe" else lower
end

-- The template colour -> palette colour map, or nil when a role has no colour.
function M.palette(getColor: (string) -> any): Palette?
  local out: Palette = {}
  for _, entry in M.ROLES do
    local colour = hex(getColor(entry.role))
    if colour == nil then
      return nil
    end
    out[entry.colour] = colour
  end
  return out
end

-- A short name for `palette`, for the painted frames' file names.
function M.key(palette: Palette): string
  local parts = {}
  for _, entry in M.ROLES do
    table.insert(parts, (palette[entry.colour]:sub(2)))
  end
  return table.concat(parts)
end

-- `svg` with each template colour replaced by the palette's.
function M.paint(svg: string, palette: Palette): string
  return (svg:gsub("#%x%x%x%x%x%x", function(found: string)
    return palette[found:lower()] or found
  end))
end

-- Every colour `svg` uses that is neither a template colour nor a fixed one.
function M.stray(svg: string): { string }
  local known = {}
  for _, entry in M.ROLES do
    known[entry.colour] = true
  end
  for _, colour in M.FIXED do
    known[colour] = true
  end
  local out = {}
  for found in svg:gmatch("#%x%x%x%x%x%x") do
    if not known[found:lower()] then
      table.insert(out, found)
    end
  end
  for _, attribute in { "fill", "stroke" } do
    for name in svg:gmatch(`{attribute}="(%a+)"`) do
      if name ~= "none" then
        table.insert(out, name)
      end
    end
  end
  return out
end

return M
