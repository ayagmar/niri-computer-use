--!nonstrict
-- The last release's plugin beside this server's `watch`: users update the server and
-- the plugin at different times. `watch` may coalesce writes of `live.json`, so a reader
-- sees any of an action's records as long as it sees the last. Both versions are fed the
-- same delivered lines and must agree on everything the last record carries.

local t = require("./support/t")
local env = require("./support/env")
local fx = require("./support/fixtures")

local current = env.module("lib/live.luau")
local released = env.module("released/lib/live.luau")
local time = env.module("lib/time.luau")
local TAG = env.sources["released/tag"]:gsub("%s", "")

local F = fx.live()
local T = time.parse("2026-10-30T12:00:00.500Z")

-- An action's records as the server writes them, phase by phase.
local function phases(record, names)
  local out = {}
  for _, phase in names do
    local outcome = if phase == "end" then record.outcome or { accepted = true } else nil
    table.insert(out, fx.with(record, { phase = phase, outcome = outcome }))
  end
  return out
end

local ACTIONS = {
  click = phases(F.click_aim, { "start", "aim", "look", "end" }),
  drag = phases(F.drag_aim, { "start", "aim", "end" }),
  key = phases(F.key_aim_floating, { "start", "aim", "look", "end" }),
  failed = { F.element_end_mismatch },
}

-- What the last record carries, as the model holds it.
local function carried(model)
  local call = model.call
  if call == nil then
    return "none"
  end
  local aim = call.aim
  local points = {}
  for _, p in (aim and aim.points) or {} do
    table.insert(points, `{p.x},{p.y}`)
  end
  local box = aim and aim.box
  return table.concat({
    `{call.pid}/{call.call}`,
    call.tool,
    call.phase,
    if aim then `{aim.output} {aim.width}x{aim.height} {table.concat(points, ";")}` else "-",
    if box then `{box.x},{box.y},{box.width},{box.height}` else "-",
    call.app or "-",
    call.error or "-",
  }, " | ")
end

-- `hello` as the last release's `watch` sends it, and as this one does, naming the file
-- where the overlay publishes its area.
local OLD_HELLO = { event = "hello", format = 1, version = "0.1.7", instance = "niri.sock" }
local NEW_HELLO = fx.with(OLD_HELLO, {
  version = "0.1.8",
  overlay = "/run/user/1000/niri-computer-use/niri.sock/overlay.json",
})

-- Feeds `lines` to a fresh model of `lib` under the fixtures' lease, after an old `hello`.
local function fold(lib, lines)
  local model = lib.new()
  lib.feed(model, OLD_HELLO, T - 1100)
  lib.feed(model, fx.state(), T - 1000)
  for i, line in lines do
    lib.feed(model, line, T + i * 10)
  end
  return model
end

-- Every delivered sequence that keeps the last record: each subset of the earlier ones,
-- after either `hello`, with a `watch` restart (either `hello`) or a line neither version
-- knows before the last.
local function deliveries(records)
  local out = {}
  local earlier = #records - 1
  for mask = 0, 2 ^ earlier - 1 do
    local kept = {}
    for i = 1, earlier do
      if (mask // 2 ^ (i - 1)) % 2 == 1 then
        table.insert(kept, records[i])
      end
    end
    for _, first in { OLD_HELLO, NEW_HELLO } do
      for _, extra in { "none", "hello", "new hello", "unknown" } do
        local lines = { first }
        table.move(kept, 1, #kept, 2, lines)
        if extra == "hello" then
          table.insert(lines, OLD_HELLO)
        elseif extra == "new hello" then
          table.insert(lines, NEW_HELLO)
        elseif extra == "unknown" then
          table.insert(lines, { event = "later", pid = 1 })
        end
        table.insert(lines, records[#records])
        table.insert(out, { lines = lines, label = `{first.version} hello, subset {mask}, {extra}` })
      end
    end
  end
  return out
end

t.group(`compat with {TAG}`, function()
  -- The bug: a change to the fold that a coalesced read, a line the last release doesn't
  -- know, or this release's `hello` sends one version somewhere the other doesn't go.
  t.test("both versions agree on what an action's last record carries, however it was delivered", function()
    for name, records in ACTIONS do
      local whole = carried(fold(current, records))
      for _, delivery in deliveries(records) do
        local label = `{name}, {delivery.label}`
        local mine = carried(fold(current, delivery.lines))
        t.eq(mine, whole, `{label}: this plugin`)
        t.eq(carried(fold(released, delivery.lines)), mine, `{label}: {TAG}`)
      end
    end
  end)
end)

return nil
