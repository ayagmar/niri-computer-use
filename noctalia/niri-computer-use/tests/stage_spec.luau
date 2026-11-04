--!nonstrict
local t = require("./support/t")
local env = require("./support/env")
local fx = require("./support/fixtures")

local stage, host = env.module("lib/stage.luau")
local live = env.module("lib/live.luau")
local time = env.module("lib/time.luau")
local describe = env.module("lib/describe.luau")
local tr = host.globals.noctalia.tr

local F = fx.live()
local T = time.parse("2026-10-30T12:00:00.500Z")
local HDMI = { width = 2048, height = 1152 }

local function envAt(scale, size)
  return {
    scale = scale or 1,
    size = size or stage.MEDIUM,
    focused = "HDMI-A-1",
    areas = { ["HDMI-A-1"] = HDMI, ["DP-2"] = { width = 1280, height = 720 } },
    tr = tr,
  }
end

local ENV = envAt(1)
local D = stage.dims(stage.MEDIUM, 1)

-- A desk where the fixtures' agent holds the lease, and a stage drawing it.
local function desk(session)
  local model = live.new()
  live.feed(model, fx.state(session), T - 1000)
  return model, stage.new()
end

local function frame(model, st, nowMs, at)
  return stage.step(st, live.view(model), nowMs, at or ENV)
end

-- A click by the holder at `x`, `y` on HDMI-A-1, as call `n`.
local function click(n, x, y, phase)
  return fx.with(F.click_aim, {
    call = n,
    phase = phase or "aim",
    aim = { output = "HDMI-A-1", area = { width = 2048, height = 1152 }, points = { { x = x, y = y } } },
  })
end

-- The rectangle the robot and its caption cover in a frame.
local function covers(f, scale)
  local d = stage.dims(f.size, scale)
  local width = d.robot + (if f.caption or f.bubble then (d.inner + f.width) * scale else 0)
  return f.x, f.y, f.x + width, f.y + d.robot
end

-- How far point `p` is from a rectangle on each axis.
local function clearance(p, left, top, right, bottom)
  local dx = math.max(left - p.x, p.x - right, 0)
  local dy = math.max(top - p.y, p.y - bottom, 0)
  return dx, dy
end

local function same(a, b, label)
  for _, key in { "output", "x", "y", "pose", "caption", "role", "opacity", "visible", "lift", "bubble" } do
    t.eq(a[key], b[key], `{label}: {key}`)
  end
  local ra, rb = a.ring or {}, b.ring or {}
  for _, key in { "x", "y", "size", "opacity" } do
    t.eq(ra[key], rb[key], `{label}: ring {key}`)
  end
end

-- Whether a frame's ring stays on its output, clear of the robot, its room to hop and
-- its caption, and centred on `p`.
local function ringFits(f, p, area, scale, label)
  local boxes = stage.layout(f, stage.dims(f.size, scale))
  local ring, group
  for _, box in boxes do
    if box.key == "ring" then
      ring = box
    else
      group = box
    end
  end
  t.ok(ring.x >= 0 and ring.y >= 0 and ring.x + ring.width <= area.width and ring.y + ring.height <= area.height, label .. ": on the output")
  local apart = ring.x >= group.x + group.width or ring.x + ring.width <= group.x or ring.y >= group.y + group.height or ring.y + ring.height <= group.y
  t.ok(apart, label .. ": clear of the robot")
  t.near(ring.x + ring.width / 2, p.x, 0.5, label .. ": centred across")
  t.near(ring.y + ring.height / 2, p.y, 0.5, label .. ": centred down")
end

t.group("stage", function()
  t.group("placement", function()
    -- The bug: the robot covering the point the agent clicks, or reaching off the output.
    t.test("beside every point it stays on the output and GAP away from the point on both axes", function()
      for _, area in { { width = 1280, height = 720 }, HDMI } do
        for _, scale in { 1, 1.25, 1.5 } do
          for _, size in { 40, 56, 72 } do
            for _, fx_ in { 0, 0.5, 1 } do
              for _, fy in { 0, 0.5, 1 } do
                local p = { x = fx_ * area.width, y = fy * area.height }
                local d = stage.dims(size, scale)
                local spot = stage.besidePoint("A", p, area, d)
                local robot, wide = d.robot, d.wide
                local left = if spot.side == "left" then spot.x - wide else spot.x
                local right = if spot.side == "left" then spot.x + robot else spot.x + robot + wide
                local label = `{area.width}x{area.height} at {scale}, size {size}, point {p.x},{p.y}`
                t.ok(left >= stage.MARGIN and right <= area.width - stage.MARGIN, label .. ": across")
                t.ok(spot.y >= stage.MARGIN and spot.y + robot <= area.height - stage.MARGIN, label .. ": down")
                local dx, dy = clearance(p, left, spot.y, right, spot.y + robot)
                t.ok(dx >= stage.GAP and dy >= stage.GAP, label .. ": clear of the point")
              end
            end
          end
        end
      end
    end)

    -- The bug: the robot covering a menu that opens below-right of the pointer.
    t.test("with room above-left of the point, the robot sits there with its caption to its left", function()
      local spot, corner = stage.besidePoint("A", { x = 650, y = 320.4 }, HDMI, D)
      t.eq(corner, 1)
      t.eq(spot.side, "left")
      t.eq(spot.x, 650 - stage.GAP - D.robot)
      t.near(spot.y, 320.4 - stage.GAP - D.robot, 0.001)
    end)

    t.test("beside a box it keeps off the box, and inside the corner of one that fills the output", function()
      local box = { x = 100, y = 50, width = 800, height = 600 }
      local spot = stage.besideBox("A", box, HDMI, D)
      t.eq(spot.x, box.x + box.width + stage.GAP, "right of the box")
      local wide = { x = 0, y = 0, width = 2048, height = 1152 }
      local inside = stage.besideBox("A", wide, HDMI, D)
      t.eq(inside.x, 2048 - stage.MARGIN - D.robot)
      t.eq(inside.y, stage.MARGIN + D.room, "with its room to hop")
      for _, b in { { x = 1500, y = 300, width = 540, height = 400 }, { x = 8, y = 8, width = 300, height = 1100 } } do
        local s = stage.besideBox("A", b, HDMI, stage.dims(stage.MEDIUM, 1.5))
        local robot = stage.MEDIUM * 1.5
        local apart = s.x + robot <= b.x or s.x >= b.x + b.width or s.y + robot <= b.y or s.y >= b.y + b.height
        t.ok(apart, `box at {b.x},{b.y}`)
      end
    end)
  end)

  t.group("frames", function()
    t.test("pops in beside its first point, gets ready, glides to the next, then taps and hops", function()
      local model, st = desk()
      live.feed(model, click(1, 650, 320.4), T)
      local first = frame(model, st, T)
      t.eq(first.output, "HDMI-A-1")
      t.eq(first.opacity, 0)
      t.ok(first.moving, "moving while popping in")
      local popped = frame(model, st, T + stage.POP_MS)
      t.eq(popped.opacity, 1)
      t.ok(not popped.moving, "still once in")
      local TRAVEL = stage.READY_MS + stage.GLIDE_MS
      local tapping = frame(model, st, T + TRAVEL + 10)
      t.eq(tapping.pose, "tap")
      t.eq(tapping.lift, D.lift, "hops as it taps")
      local still = frame(model, st, T + TRAVEL + stage.TAP_MS + 10)
      t.eq(still.pose, "idle")
      t.eq(still.lift, 0)
      t.ok(not still.moving)

      live.feed(model, click(2, 1500, 900), T + 1000)
      local leaving = frame(model, st, T + 1000)
      t.eq(leaving.pose, "ready-r", "looks toward where it goes")
      t.eq(leaving.nextMs, T + 1000 + stage.READY_MS, "and changes nothing until it sets off")
      t.eq(leaving.caption, nil)
      t.eq(frame(model, st, T + 1000 + stage.READY_MS - 1).x, leaving.x, "standing where it was")
      local midway = frame(model, st, T + 1000 + stage.READY_MS + 70)
      local there = frame(model, st, T + 1000 + TRAVEL)
      t.ok(midway.moving, "moving while gliding")
      t.eq(midway.pose, "lean-r")
      t.ok(midway.x > leaving.x and midway.x < there.x, "moving toward the new point")
      t.ok(not there.moving)
      t.eq(frame(model, st, T + 1000 + TRAVEL + 10).pose, "tap")
    end)

    -- The bug: the robot moving while the agent's own settle wait samples the screen,
    -- so the wait never sees the screen still.
    t.test("while the agent looks nothing changes, and the tap waits for the end", function()
      local model, st = desk()
      live.feed(model, click(1, 650, 320.4), T)
      frame(model, st, T + 500)
      live.feed(model, click(2, 1500, 900), T + 1000)
      frame(model, st, T + 1000)
      live.feed(model, click(2, 1500, 900, "look"), T + 1030)
      local looked = frame(model, st, T + 1030)
      t.ok(not looked.moving)
      t.eq(looked.pose, "idle")
      for at = T + 1031, T + 1700, 37 do
        same(frame(model, st, at), looked, `at {at - T}`)
      end
      live.feed(model, click(2, 1500, 900, "end"), T + 1700)
      t.eq(frame(model, st, T + 1710).pose, "tap")
      t.eq(frame(model, st, T + 1710).caption, "click")
      local quiet = frame(model, st, T + 1700 + stage.CAPTION_MS)
      t.eq(quiet.caption, nil)
      t.eq(quiet.x, looked.x + (D.inner + looked.width), "the caption went, the robot stayed")
    end)

    t.test("at a larger UI scale the robot keeps the same gap to the point", function()
      local model, st = desk()
      live.feed(model, click(1, 650, 320.4), T)
      local f = frame(model, st, T + 500, envAt(1.5))
      local _, top, right, bottom = covers(f, 1.5)
      t.near(right, 650 - stage.GAP, 1)
      t.near(bottom, 320.4 - stage.GAP, 1)
      t.ok(top >= stage.MARGIN)
    end)

    t.test("typing alternates while the call runs; a key call alternates once", function()
      local model, st = desk("pi/9")
      live.feed(model, F.type_aim, T)
      local seen = {}
      for at = T, T + 600, 70 do
        seen[frame(model, st, at).pose] = true
      end
      t.ok(seen["type-a"] and seen["type-b"], "both typing frames")
      t.eq(frame(model, st, T + 600).caption, "typing 24 chars in kitty")
      live.feed(model, fx.with(F.type_aim, { phase = "end", outcome = { accepted = true } }), T + 700)
      t.eq(frame(model, st, T + 710).pose, "idle")

      local keys, kst = desk()
      live.feed(keys, F.key_aim_floating, T)
      t.eq(frame(keys, kst, T + 10).pose, "type-a")
      t.eq(frame(keys, kst, T + stage.TYPE_MS + 10).pose, "type-b")
      t.eq(frame(keys, kst, T + 2 * stage.TYPE_MS + 10).pose, "idle")
      t.eq(frame(keys, kst, T + 300).caption, "keys ctrl+s in org.gnome.TextEditor")
    end)

    t.test("an error shrugs and names it; a cancelled call doesn't", function()
      local model, st = desk()
      live.feed(model, F.element_end_mismatch, T)
      local f = frame(model, st, T)
      t.eq(f.pose, "shrug")
      t.eq(f.caption, "focus mismatch")
      t.eq(f.role, "error")
      t.eq(frame(model, st, T + stage.SHRUG_MS).pose, "idle")
      t.eq(frame(model, st, T + stage.CAPTION_MS).caption, nil)

      local dragged, dst = desk()
      live.feed(dragged, F.drag_aim, T)
      live.feed(dragged, F.drag_end_cancelled, T + 100)
      local cancelled = frame(dragged, dst, T + 100)
      t.eq(cancelled.pose, "idle")
      t.eq(cancelled.caption, "drag")
    end)

    t.test("stopped agents show the stop sign where the robot stood", function()
      local model, st = desk()
      live.feed(model, click(1, 650, 320.4), T)
      local before = frame(model, st, T + 500)
      live.feed(model, fx.state(nil, { stopped = true }), T + 600)
      local stopped = frame(model, st, T + 600)
      t.eq(stopped.pose, "stop")
      t.eq(stopped.caption, "stopped")
      t.eq(stopped.output, before.output)
    end)

    t.test("an agent that asks waves from the focused output's corner", function()
      local model, st = desk()
      live.feed(model, fx.state(nil, { pending = true }), T)
      local f = frame(model, st, T)
      t.eq(f.caption, "asking to drive")
      t.eq(f.role, "tertiary")
      t.eq(f.output, "HDMI-A-1")
      local _, _, right, bottom = covers(f, 1)
      t.eq(right, HDMI.width - stage.HOME)
      t.eq(bottom, HDMI.height - stage.HOME)
      t.ok(frame(model, st, T + stage.ASK_MS).pose ~= f.pose, "waves")
    end)

    t.test("a release waves goodbye, then the robot is gone", function()
      local model, st = desk()
      frame(model, st, T)
      live.feed(model, fx.state(false), T + 100)
      t.ok(frame(model, st, T + 100).pose:find("wave") ~= nil)
      local gone = frame(model, st, T + 100 + stage.GOODBYE_MS)
      t.ok(not gone.visible)
      t.ok(not gone.moving)
    end)

    t.test("dozes after DOZE_MS with no call open, never during one", function()
      local model, st = desk()
      t.eq(frame(model, st, T).pose, "idle")
      t.eq(frame(model, st, T - 1000 + stage.DOZE_MS).pose, "doze")
      live.feed(model, fx.with(F.launch_start, { ts = "2026-10-30T12:00:20.000Z" }), T + 20000)
      t.eq(frame(model, st, T + 20000 + stage.DOZE_MS + 1000).pose, "idle")
    end)

    t.test("a call without an end is over after STALE_MS, and nothing is scheduled after", function()
      local model, st = desk()
      live.feed(model, click(1, 650, 320.4), T)
      local before = frame(model, st, T + 500)
      local f = frame(model, st, T + live.STALE_MS)
      t.eq(f.y, before.y, "stays where it stood")
      t.eq(f.caption, nil)
      t.eq(f.pose, "doze")
      t.ok(not f.moving)
      t.eq(f.nextMs, nil)
    end)

    t.test("an older server's calls show at home with their caption", function()
      local model, st = desk()
      live.feed(model, { event = "action", ts = "2026-10-30T12:00:00.400Z", session = fx.HOLDER, tool = "scroll", args = {} }, T)
      local f = frame(model, st, T)
      t.eq(f.caption, "scroll")
      local _, _, right = covers(f, 1)
      t.eq(right, HDMI.width - stage.HOME)
      t.eq(frame(model, st, T + stage.CAPTION_MS).caption, nil)
    end)

    t.test("a target on another output pops in there instead of gliding across", function()
      local model, st = desk()
      live.feed(model, click(1, 650, 320.4), T)
      frame(model, st, T + 500)
      local other = fx.with(click(2, 300, 300), {})
      other.aim = { output = "DP-2", area = { width = 1280, height = 720 }, points = { { x = 300, y = 300 } } }
      live.feed(model, other, T + 600)
      local f = frame(model, st, T + 600)
      t.eq(f.output, "DP-2")
      t.eq(f.opacity, 0)
    end)

    -- The bug: a ring sized in the UI scale's units beside a robot placed in logical
    -- pixels, overlapping the robot at 1.5, or running off the output by an edge click.
    t.test("a click's ring grows and fades on the point, clear of the robot, at any size and edge", function()
      local area = { width = 1280, height = 720 }
      local points = {
        { x = 640, y = 360 },
        { x = 1, y = 360 },
        { x = 6, y = 6 },
        { x = 1279, y = 700 },
        { x = 640, y = 3 },
        { x = 1270, y = 715 },
      }
      for _, scale in { 1, 1.25, 1.5 } do
        for _, size in { 40, 56, 72 } do
          for _, p in points do
            local model, st = desk()
            local at = envAt(scale, size)
            at.areas = { ["HDMI-A-1"] = area }
            local aim = { output = "HDMI-A-1", area = area, points = { p } }
            live.feed(model, fx.with(F.click_aim, { aim = aim }), T)
            local label = `size {size} at {scale}, click {p.x},{p.y}`
            local sizes, opacities, steps = {}, {}, {}
            local f = frame(model, st, T, at)
            local seen = 0
            while f.nextMs and f.nextMs < T + 2000 do
              f = frame(model, st, f.nextMs, at)
              local step = f.ring and `{f.ring.size}/{f.ring.opacity}`
              if step and not steps[step] then
                steps[step] = true
                seen += 1
                ringFits(f, p, area, scale, label)
                table.insert(sizes, f.ring.size)
                table.insert(opacities, f.ring.opacity)
              end
            end
            local edge = math.min(p.x, p.y, area.width - p.x, area.height - p.y)
            if edge >= stage.RIPPLE_MIN then
              t.eq(seen, stage.RIPPLE_STEPS, label .. ": every step drawn")
              t.ok(sizes[#sizes] > sizes[1] and opacities[#opacities] < opacities[1], label .. ": grows and fades")
              t.ok(sizes[#sizes] / 2 <= math.min(stage.RIPPLE_RADIUS * scale, edge), label .. ": no bigger than it may be")
            else
              t.eq(seen, 0, label .. ": no ring where none fits")
            end
          end
        end
      end
    end)

    t.test("the ring starts with the tap: after the glide, or at the end when the agent looked", function()
      local TRAVEL = stage.READY_MS + stage.GLIDE_MS
      local model, st = desk()
      live.feed(model, click(1, 650, 320.4), T)
      t.eq(frame(model, st, T + TRAVEL - 1).ring, nil)
      t.ok(frame(model, st, T + TRAVEL).ring ~= nil)
      t.eq(frame(model, st, T + TRAVEL + stage.RIPPLE_MS).ring, nil, "gone after RIPPLE_MS")

      local looked, lst = desk()
      live.feed(looked, click(1, 650, 320.4), T)
      live.feed(looked, click(1, 650, 320.4, "look"), T + 20)
      t.eq(frame(looked, lst, T + TRAVEL + 10).ring, nil, "not while the agent looks")
      live.feed(looked, click(1, 650, 320.4, "end"), T + 900)
      local f = frame(looked, lst, T + 900)
      t.ok(f.ring ~= nil and f.pose == "tap", "tap and ring at the end")
    end)

    -- The bug: a click whose end came before the robot arrived lost its tap and ring.
    t.test("a click that ends before the robot arrives still taps there", function()
      local model, st = desk()
      live.feed(model, click(1, 650, 320.4), T)
      live.feed(model, click(1, 650, 320.4, "end"), T + 20)
      local f = frame(model, st, T + stage.READY_MS + stage.GLIDE_MS + 10)
      t.eq(f.pose, "tap")
      t.ok(f.ring ~= nil)
    end)

    t.test("a drag glides in its grab from beside the start to beside the end", function()
      local model, st = desk()
      live.feed(model, click(1, 1800, 1000), T)
      frame(model, st, T + 500)
      live.feed(model, F.drag_aim, T + 1000)
      local start = frame(model, st, T + 1000)
      local done = frame(model, st, T + 1000 + stage.DRAG_MS)
      t.eq(start.pose, "grab")
      t.ok(start.moving)
      t.ok(not done.moving)
      local from = stage.besidePoint("HDMI-A-1", { x = 80.4, y = 80.4 }, HDMI, D, 1)
      t.eq(start.side, "left")
      t.eq(start.caption, nil, "the caption waits for the robot to arrive")
      t.near(start.x, from.x, 1)
      local _, _, right, bottom = covers(done, 1)
      local dx, dy = clearance({ x = 400.4, y = 240.4 }, done.x, done.y, right, bottom)
      t.ok(dx >= stage.GAP and dy >= stage.GAP, "clear of where the drag ends")
    end)
  end)

  -- Seeded so a failure repeats: a linear congruential generator.
  local function random(seed)
    local state = seed
    return function(n)
      state = (state * 1103515245 + 12345) % 2147483648
      -- The low bits of such a generator repeat with a short period; use the high ones.
      return (state // 65536) % n + 1
    end
  end

  local TOOLS = { "click", "drag", "scroll", "type_text", "key", "focus", "launch", "activate_element" }
  local OUTPUTS = { { name = "HDMI-A-1", width = 2048, height = 1152 }, { name = "DP-2", width = 1280, height = 720 } }

  local function record(pick, nowMs, holder)
    local output = OUTPUTS[pick(2)]
    local tool = TOOLS[pick(#TOOLS)]
    local points = {}
    for _ = 1, if tool == "drag" then 2 elseif pick(3) == 1 then 0 else 1 do
      table.insert(points, { x = pick(output.width) - 1, y = pick(output.height) - 1 })
    end
    local box = if pick(4) == 1 then { x = pick(1000), y = pick(500), width = pick(800), height = pick(600) } else nil
    local phases = { "start", "aim", "look", "end" }
    local errors = { nil, nil, "cancelled", "focus_mismatch" }
    local ts = math.max(time.parse(fx.SINCE) :: number, nowMs - pick(20000))
    return {
      event = "live",
      -- A non-holder's records come from another server, pid 9, to tell them apart.
      pid = if holder then pick(2) else 9,
      call = pick(5),
      session = if holder then fx.HOLDER else "pi/9",
      tool = tool,
      tsMs = ts,
      phase = phases[pick(4)],
      args = { count = pick(2), text_len = pick(40), keys = { "ctrl+s" } },
      aim = { output = output.name, area = { width = output.width, height = output.height }, points = points, box = box },
      window = if pick(2) == 1 then { id = 3, app_id = "kitty" } else nil,
      outcome = { error = errors[pick(4)] },
      backlog = pick(10) == 1,
    }
  end

  -- An epoch time in ms as `watch` writes it.
  local function stamp(ms)
    local seconds = ms // 1000
    local day = seconds % 86400
    local base = time.parse("2026-10-30T00:00:00.000Z") :: number
    assert(ms - base < 86400000 and ms >= base, "the generated clock stays within the day")
    return string.format("2026-10-30T%02d:%02d:%02d.%03dZ", day // 3600, day % 3600 // 60, day % 60, ms % 1000)
  end

  t.group("thinking", function()
    local OVERLAY = "/run/user/1000/niri-computer-use/niri.sock/overlay.json"
    local THINK = table.clone(ENV)
    THINK.think = true

    -- The fixtures' agent clicked at (x, 320.4), 650 by default, and the call ended at T,
    -- with `watch` naming the area file unless `overlay` is false, and the robot drawn
    -- then, by `at`.
    local function thinker(overlay, at, x)
      local model, st = desk()
      live.feed(model, { event = "hello", format = 1, overlay = if overlay == false then nil else OVERLAY }, T - 900)
      live.feed(model, fx.with(click(1, x or 650, 320.4, "end"), { outcome = { accepted = true } }), T)
      frame(model, st, T, at or THINK)
      return model, st
    end

    -- Whether `inner` lies within `outer`.
    local function within(inner, outer)
      return inner.x >= outer.x
        and inner.y >= outer.y
        and inner.x + inner.width <= outer.x + outer.width
        and inner.y + inner.height <= outer.y + outer.height
    end

    -- The bug: an animation that leaves the area the server leaves out, or one fast
    -- enough to keep a settle wait's run from starting, breaks the agent's screenshots.
    t.test("between actions it thinks in place: a change every beat or so, all inside its area", function()
      for _, size in { 40, 56, 72 } do
        for _, scale in { 1, 1.5, 2.5 } do
          local at = envAt(scale, size)
          at.think = true
          local d = stage.dims(size, scale)
          local model, st = thinker(nil, at)
          local start = T + stage.CAPTION_MS + 1000
          local first = frame(model, st, start, at)
          local label = `size {size} at {scale}`
          t.ok(first.area ~= nil, label .. ": no area")
          t.ok(stage.fits({ width = first.area.width, height = first.area.height }), label .. ": too large")
          local poses, lifts, changes = {}, {}, {}
          local lastKey = stage.key(first)
          for nowMs = start, start + 10000, 10 do
            local f = frame(model, st, nowMs, at)
            for _, key in { "output", "x", "y", "side", "width", "caption" } do
              t.eq(f[key], first[key], `{label} at {nowMs - start}: {key}`)
            end
            t.eq(`{f.area.x},{f.area.y},{f.area.width},{f.area.height}`, `{first.area.x},{first.area.y},{first.area.width},{first.area.height}`, label .. ": the area moved")
            for _, box in stage.layout(f, d) do
              t.ok(within(box, f.area), `{label}: {box.key} outside the area`)
            end
            t.ok(f.lift <= d.lift, label .. ": bobs out of its room")
            t.ok(f.nextMs > nowMs and f.nextMs <= nowMs + stage.BEAT_MS, label .. ": the next beat")
            poses[f.pose], lifts[f.lift] = true, true
            local key = stage.key(f)
            if key ~= lastKey then
              table.insert(changes, nowMs)
              lastKey = key
            end
          end
          t.ok(poses.idle and poses.blink and poses.pulse, label .. ": blinks and pulses")
          t.ok(lifts[0] and lifts[1] and lifts[2], label .. ": bobs")
          for i = 2, #changes do
            local apart = changes[i] - changes[i - 1]
            t.ok(apart >= 100 and apart <= 1000, `{label}: changes {apart} ms apart`)
          end
        end
      end
    end)

    -- The bug: the hop, the ring, an error's caption or the caption going away drawn
    -- outside the published area, or the area changing under them, so a settle wait
    -- that starts as the action ends never sees two samples alike.
    t.test("from the moment it arrives, one area covers the hop, the ring and the caption going", function()
      local outcomes = {
        { name = "accepted", outcome = { accepted = true } },
        { name = "error", outcome = { accepted = nil, error = "focus_mismatch" } },
      }
      for _, setting in { { 72, 2.5 }, { 40, 0.5 }, { 56, 1 } } do
        local size, scale = setting[1], setting[2]
        local at = envAt(scale, size)
        at.think = true
        local d = stage.dims(size, scale)
        for _, x in { 120, 1000, 1900 } do
          for _, looked in { false, true } do
            for _, ending in outcomes do
              local label = `size {size} at {scale}, x {x}, look {looked}, {ending.name}`
              local model, st = desk()
              live.feed(model, { event = "hello", format = 1, overlay = OVERLAY }, T - 900)
              live.feed(model, click(1, x, 320.4), T)
              local arrived = T + stage.READY_MS + stage.GLIDE_MS
              frame(model, st, T, at)
              if looked then
                live.feed(model, click(1, x, 320.4, "look"), arrived + 10)
              end
              local endMs = arrived + 50
              live.feed(model, fx.with(click(1, x, 320.4, "end"), { outcome = ending.outcome }), endMs)
              local first = frame(model, st, endMs, at)
              t.ok(first.area ~= nil, label .. ": no area as the call ends")
              local function rect(a)
                return if a then `{a.x},{a.y},{a.width},{a.height}` else "none"
              end
              local captions, rings, hopped = {}, false, false
              for nowMs = endMs, endMs + stage.CAPTION_MS + 3000, 100 do
                local f = frame(model, st, nowMs, at)
                t.eq(rect(f.area), rect(first.area), `{label} at {nowMs - endMs}: the area`)
                for _, box in stage.layout(f, d) do
                  t.ok(f.area and within(box, f.area), `{label} at {nowMs - endMs}: {box.key} outside the area`)
                  rings = rings or box.key == "ring"
                end
                captions[f.caption or (if f.bubble then "bubble" else "none")] = true
                hopped = hopped or f.lift > 0
              end
              t.ok(captions.bubble, label .. ": thinks once the caption is gone")
              t.ok(hopped, label .. ": hops")
              t.eq(rings, ending.name == "accepted", label .. ": rings only for an accepted click")
            end
          end
        end
      end
    end)

    -- The bug: a cap below what the robot can cover beside some click, so it publishes
    -- nothing there and its hop and ring go uncovered.
    t.test("beside a click anywhere at the largest size and UI scale, its area fits the cap", function()
      local at = envAt(2.5, 72)
      at.think = true
      local widest, tallest = 0, 0
      for x = 0, 2048, 64 do
        for y = 0, 1152, 48 do
          local model, st = desk()
          live.feed(model, { event = "hello", format = 1, overlay = OVERLAY }, T - 900)
          live.feed(model, fx.with(click(1, x, y, "end"), { outcome = { accepted = true } }), T)
          frame(model, st, T, at)
          local f = frame(model, st, T + 1000, at)
          t.ok(f.area ~= nil, `click at {x},{y}: no area`)
          widest, tallest = math.max(widest, f.area.width), math.max(tallest, f.area.height)
        end
      end
      t.eq(`{math.ceil(widest)}x{math.ceil(tallest)}`, `{stage.AREA_MAX.width}x{stage.AREA_MAX.height}`, "the cap is the largest area")
    end)

    -- The bug: thinking where the server compares the robot's pixels keeps every settle
    -- wait going for as long as it thinks.
    t.test("it doesn't think where the server wouldn't leave its area out or it can't publish it", function()
      local after = T + stage.CAPTION_MS + 1000
      local function still(model, st, at, label)
        local f = frame(model, st, after, at)
        t.eq(f.area, nil, label)
        t.eq(f.bubble, nil, label)
        t.eq(f.pose, "idle", label)
        t.eq(f.nextMs, T + stage.DOZE_MS, label .. ": dozes as before")
      end
      local model, st = thinker(false)
      still(model, st, THINK, "a watch that names no area file")
      model, st = thinker(nil, ENV)
      still(model, st, ENV, "the layer can't publish")
      live.feed(model, fx.state(nil, { clients = { { pid = 4, replaced = false }, { pid = 5, replaced = true } } }), T + 1)
      still(model, st, THINK, "a server an update replaced")
      live.feed(model, fx.state(nil, { clients = { { pid = 4, replaced = false } } }), T + 2)
      t.ok(frame(model, st, after, THINK).area ~= nil, "thinks once it is gone")
      live.feed(model, { event = "hello", format = 1 }, T + 3)
      still(model, st, THINK, "a new watch that names no area file")
      -- A server without `live` records.
      model, st = desk()
      live.feed(model, { event = "hello", format = 1, overlay = OVERLAY }, T - 900)
      live.feed(model, { event = "action", ts = "2026-10-30T12:00:00.400Z", session = fx.HOLDER, tool = "scroll", args = {} }, T)
      frame(model, st, T, THINK)
      t.eq(frame(model, st, after, THINK).bubble, nil, "an older server's session")
      -- Too large for the server: the largest robot past Noctalia's largest UI scale.
      local huge = envAt(3, 72)
      huge.think = true
      model, st = thinker(nil, huge)
      t.eq(frame(model, st, after, huge).area, nil, "an area the server would refuse")
    end)

    t.test("it thinks only while no call runs, and stops publishing before the robot moves", function()
      local model, st = thinker()
      local shown = frame(model, st, T + 500, THINK)
      t.eq(shown.bubble, nil, "the call's caption shows")
      local thought = frame(model, st, T + stage.CAPTION_MS, THINK)
      t.ok(thought.bubble ~= nil, "thinks once the caption is gone")
      t.eq(stage.key({ visible = true, x = thought.area.x, y = thought.area.y, pose = "", role = "", side = "", width = thought.area.width, size = thought.area.height, lift = 0, opacity = 0 }), stage.key({ visible = true, x = shown.area.x, y = shown.area.y, pose = "", role = "", side = "", width = shown.area.width, size = shown.area.height, lift = 0, opacity = 0 }), "the same area")
      local start = click(2, 1200, 700, "start")
      start.aim = nil
      live.feed(model, start, T + 4000)
      local started = frame(model, st, T + 4000, THINK)
      t.ok(started.bubble == nil and started.area ~= nil, "a call starts where it stands")
      live.feed(model, click(2, 1200, 700), T + 4010)
      local f = frame(model, st, T + 4010, THINK)
      t.eq(f.area, nil, "gets ready to glide")
      t.eq(f.x, thought.x + (D.inner + D.captionMin), "where it stood, its bubble gone")
    end)

    t.test("dozes after THINK_MS without a record, with nothing scheduled", function()
      local model, st = thinker()
      t.ok(frame(model, st, T + stage.DOZE_MS, THINK).area ~= nil, "thinks past DOZE_MS")
      local f = frame(model, st, T + stage.THINK_MS, THINK)
      t.eq(f.pose, "doze")
      t.eq(f.bubble, nil)
      t.eq(f.nextMs, nil)
    end)

    t.test("a release while it thinks waves goodbye, then it is gone", function()
      local model, st = thinker()
      t.ok(frame(model, st, T + 3000, THINK).bubble ~= nil)
      live.feed(model, fx.state(false), T + 3100)
      local bye = frame(model, st, T + 3100, THINK)
      t.ok(bye.visible and bye.bubble == nil and bye.pose:match("^wave"), bye.pose)
      t.eq(frame(model, st, T + 3100 + stage.GOODBYE_MS, THINK).visible, false)
    end)

    -- The bug: an area the plugin publishes that the server refuses as too large, so a
    -- setting the plugin offers thinks with every pixel compared.
    t.test("every size, UI scale and side gives the areas the server's tests read", function()
      local lines = { `\{"max":\{"width":{stage.AREA_MAX.width},"height":{stage.AREA_MAX.height}}}` }
      local n = function(value)
        return string.format("%.4f", value)
      end
      for _, size in { 40, 56, 72 } do
        for step = 10, 50 do
          local scale = step / 20
          local at = envAt(scale, size)
          at.think = true
          for _, x in { 120, 1900 } do
            local model, st = thinker(nil, at, x)
            local f = frame(model, st, T + stage.CAPTION_MS + 1000, at)
            local a = f.area
            t.ok(a ~= nil, `size {size} at {scale}: no area`)
            local rect = `\{"x":{n(a.x)},"y":{n(a.y)},"width":{n(a.width)},"height":{n(a.height)}}`
            table.insert(lines, `\{"size":{size},"scale":{n(scale)},"side":"{f.side}","rect":{rect}}`)
          end
        end
      end
      local generated = table.concat(lines, "\n") .. "\n"
      local shared = env.sources["tests/fixtures/overlay-areas.jsonl"]
      if shared ~= generated then
        print(generated)
      end
      t.eq(shared, generated, "tests/fixtures/overlay-areas.jsonl differs; the generated lines are above")
    end)
  end)

  local THINKING = table.clone(ENV)
  THINKING.think = true

  t.test("generated sequences keep the overlay's invariants", function()
    local captions = { [tr("overlay.stopped")] = true, [tr("overlay.asking")] = true }
    for seed = 1, 300 do
      local pick = random(seed)
      local model, st = live.new(), stage.new()
      local nowMs = T
      local holder = fx.HOLDER
      live.feed(model, fx.state(), nowMs)
      local lastFedMs = nowMs
      local movedSince = nil
      local previous, previousKey, quiet = nil, nil, false
      local drawn = nil
      for _ = 1, 60 do
        local kind = pick(10)
        quiet = false
        if kind <= 4 then
          local event = record(pick, nowMs, holder ~= nil and pick(5) > 1)
          event.ts = stamp(event.tsMs)
          event.tsMs = nil
          if event.outcome.error then
            captions[describe.failure(event.outcome.error)] = true
          end
          captions[describe.doing(event.tool, event.args, event.window and event.window.app_id, tr) or ""] = true
          captions[describe.doing(event.tool, event.args, nil, tr) or ""] = true
          -- A call keeps the app an earlier record of it named.
          captions[describe.doing(event.tool, event.args, "kitty", tr) or ""] = true
          live.feed(model, event, nowMs)
          quiet = true
        elseif kind == 5 then
          local choice = pick(5)
          holder = if choice == 1 then nil else fx.HOLDER
          local extra = { stopped = pick(6) == 1, pending = choice == 2, clients = { { pid = 4, replaced = pick(4) == 1 } } }
          live.feed(model, fx.state(if holder then holder else false, extra), nowMs)
        elseif kind == 6 then
          live.feed(model, { event = "hello", format = 1, overlay = if pick(3) > 1 then "/run/overlay.json" else nil }, nowMs)
        elseif kind == 7 then
          -- As the layer Noctalia builds on another output, or the plugin restarting.
          st = stage.new()
          drawn = nil
        elseif kind == 8 then
          live.feed(model, { event = "later", pid = 1 }, nowMs)
          quiet = true
        else
          nowMs += if pick(10) == 1 then pick(130000) else pick(400)
          quiet = true
        end
        if kind <= 8 then
          lastFedMs = nowMs
          movedSince = nil
        end
        local view = live.view(model)
        local f = stage.step(st, view, nowMs, THINKING)
        local label = `seed {seed} at {nowMs - T}`
        -- Thinking only between actions of a server that leaves its area out.
        if f.bubble then
          local call = view.call
          local idle = call == nil or (call.phase == "end" and nowMs >= call.endMs + stage.CAPTION_MS) or nowMs - view.heardMs >= live.STALE_MS
          -- A frame held while the agent looks at another output is the last one drawn.
          local held = drawn ~= nil and stage.key(drawn) == stage.key(f) and f.area == nil
          t.ok(held or (f.area and view.overlay and not view.replaced and view.live and not view.stopped and idle), label .. ": thinks during an action or unpublished")
          if drawn and drawn.bubble then
            for _, key in { "output", "x", "y", "side", "width", "caption" } do
              t.eq(f[key], drawn[key], `{label}: {key} changed while it thought`)
            end
          end
        end
        -- Everything a frame with an area draws is inside it, and the area stays the same
        -- while the robot stands still, so whatever it changes there is left out.
        if f.area then
          -- Popping in happens where it stands; a glide doesn't.
          local gliding = f.moving and f.opacity == 1
          t.ok(view.overlay and not view.replaced and not gliding, label .. ": an area while gliding or unpublished")
          local a = f.area
          for _, box in stage.layout(f, stage.dims(f.size, 1)) do
            t.ok(box.x >= a.x and box.y >= a.y and box.x + box.width <= a.x + a.width and box.y + box.height <= a.y + a.height, label .. `: {box.key} outside the area`)
          end
          local was = drawn and drawn.area
          if was and drawn.output == f.output and not drawn.moving and drawn.visible then
            local same = was.x == a.x and was.y == a.y and was.width == a.width and was.height == a.height
            t.ok(same or drawn.x ~= f.x or drawn.y ~= f.y or drawn.side ~= f.side, label .. ": the area changed where the robot stood")
          end
        end
        local key = view.call and `{view.call.pid}/{view.call.call}`
        -- Visible only while an agent holds or asks, or for the goodbye.
        if f.visible and view.holder == nil then
          t.ok(view.releasedMs and nowMs - view.releasedMs < stage.GOODBYE_MS, label .. ": shown with no lease")
        end
        t.ok(view.call == nil or view.call.pid ~= 9, label .. ": shows another session's call")
        if f.visible then
          local area = ENV.areas[f.output]
          local left, top, right, bottom = covers(f, 1)
          t.ok(left >= 0 and top >= 0 and right <= area.width and bottom <= area.height, label .. `: off the output {f.output} {left},{top},{right},{bottom} {f.pose} {f.caption} {view.call and view.call.tool}`)
          local call = view.call
          local points = call and call.aim and call.aim.output == f.output and call.aim.points or {}
          local current = call and (call.phase == "end" or nowMs - view.heardMs < live.STALE_MS)
          if current and #points > 0 and not f.moving and view.holder and not view.holder.pending and view.live then
            local dx, dy = clearance(points[#points], left, top, right, bottom)
            t.ok(dx > 0 or dy > 0, label .. ": covers its point")
          end
          if f.caption then
            t.ok(captions[f.caption], label .. `: a caption from nowhere: {f.caption}`)
          end
          -- A frame held while the agent looks at another output keeps its ring.
          local held = call and call.phase == "look" and call.aim and call.aim.output ~= f.output
          if f.ring and not held then
            local point = call and call.aim and call.aim.points[1]
            t.ok(call and call.tool == "click" and point ~= nil, label .. ": a ring without a click")
            ringFits(f, point, area, 1, label)
          end
        end
        if f.moving then
          movedSince = movedSince or nowMs
          t.ok(nowMs - movedSince <= stage.DRAG_MS, label .. ": moving for too long")
        else
          movedSince = nil
        end
        if nowMs - lastFedMs >= live.STALE_MS and not (view.holder and view.holder.pending) then
          t.ok(not f.moving and f.nextMs == nil, label .. ": not still long after the last line")
        end
        -- While the agent looks, nothing the robot shows changes: not while it is stopped
        -- or asking, which the user and the lease decide.
        local looking = view.call
          and view.call.phase == "look"
          and nowMs - view.heardMs < live.STALE_MS
          and view.holder
          and not view.holder.pending
          and not view.stopped
        if looking and quiet and previous and previousKey == key then
          same(f, previous, label .. ": changed while the agent looked")
        end
        -- Nor does it appear: not from a new stage, nor on the output the agent looks at
        -- while it stood on another.
        if looking and kind == 7 then
          t.ok(not f.visible, label .. ": appeared while the agent looked")
        end
        local aimed = looking and view.call.aim and view.call.aim.output
        if aimed and drawn and drawn.visible and drawn.output ~= aimed then
          same(f, drawn, label .. ": moved to another output while the agent looked")
        end
        previous = if looking then f else nil
        previousKey = key
        drawn = f
      end
    end
  end)

  t.test("ten thousand frames take well under the 25 ms a call may use each", function()
    local model, st = desk()
    live.feed(model, click(1, 650, 320.4), T)
    local view = live.view(model)
    local started = os.clock()
    for i = 1, 10000 do
      stage.step(st, view, T + i, ENV)
    end
    t.ok(os.clock() - started < 2, "10000 frames in under 2 s")
  end)
end)

return nil
