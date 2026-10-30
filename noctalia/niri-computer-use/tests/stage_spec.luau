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

local function envAt(scale)
  return {
    scale = scale or 1,
    focused = "HDMI-A-1",
    areas = { ["HDMI-A-1"] = HDMI, ["DP-2"] = { width = 1280, height = 720 } },
    tr = tr,
  }
end

local ENV = envAt(1)

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
  local width = stage.SPRITE * scale + (if f.caption then (stage.INNER + f.width) * scale else 0)
  return f.x, f.y, f.x + width, f.y + stage.SPRITE * scale
end

-- How far point `p` is from a rectangle on each axis.
local function clearance(p, left, top, right, bottom)
  local dx = math.max(left - p.x, p.x - right, 0)
  local dy = math.max(top - p.y, p.y - bottom, 0)
  return dx, dy
end

local function same(a, b, label)
  for _, key in { "output", "x", "y", "pose", "caption", "role", "opacity", "visible" } do
    t.eq(a[key], b[key], `{label}: {key}`)
  end
end

t.group("stage", function()
  t.group("placement", function()
    -- The bug: the robot covering the point the agent clicks, or reaching off the output.
    t.test("beside every point it stays on the output and GAP away from the point on both axes", function()
      for _, area in { { width = 1280, height = 720 }, HDMI } do
        for _, scale in { 1, 1.5 } do
          for _, fx_ in { 0, 0.5, 1 } do
            for _, fy in { 0, 0.5, 1 } do
              local p = { x = fx_ * area.width, y = fy * area.height }
              local spot = stage.besidePoint("A", p, area, scale)
              local robot = stage.SPRITE * scale
              local wide = (stage.INNER + stage.CAPTION_MAX) * scale
              local left = if spot.side == "left" then spot.x - wide else spot.x
              local right = if spot.side == "left" then spot.x + robot else spot.x + robot + wide
              local label = `{area.width}x{area.height} at {scale}, point {p.x},{p.y}`
              t.ok(left >= stage.MARGIN and right <= area.width - stage.MARGIN, label .. ": across")
              t.ok(spot.y >= stage.MARGIN and spot.y + robot <= area.height - stage.MARGIN, label .. ": down")
              local dx, dy = clearance(p, left, spot.y, right, spot.y + robot)
              t.ok(dx >= stage.GAP and dy >= stage.GAP, label .. ": clear of the point")
            end
          end
        end
      end
    end)

    -- The bug: the robot covering a menu that opens below-right of the pointer.
    t.test("with room above-left of the point, the robot sits there with its caption to its left", function()
      local spot, corner = stage.besidePoint("A", { x = 650, y = 320.4 }, HDMI, 1)
      t.eq(corner, 1)
      t.eq(spot.side, "left")
      t.eq(spot.x, 650 - stage.GAP - stage.SPRITE)
      t.near(spot.y, 320.4 - stage.GAP - stage.SPRITE, 0.001)
    end)

    t.test("beside a box it keeps off the box, and inside the corner of one that fills the output", function()
      local box = { x = 100, y = 50, width = 800, height = 600 }
      local spot = stage.besideBox("A", box, HDMI, 1)
      t.eq(spot.x, box.x + box.width + stage.GAP, "right of the box")
      local wide = { x = 0, y = 0, width = 2048, height = 1152 }
      local inside = stage.besideBox("A", wide, HDMI, 1)
      t.eq(inside.x, 2048 - stage.MARGIN - stage.SPRITE)
      t.eq(inside.y, stage.MARGIN)
      for _, b in { { x = 1500, y = 300, width = 540, height = 400 }, { x = 8, y = 8, width = 300, height = 1100 } } do
        local s = stage.besideBox("A", b, HDMI, 1.5)
        local robot = stage.SPRITE * 1.5
        local apart = s.x + robot <= b.x or s.x >= b.x + b.width or s.y + robot <= b.y or s.y >= b.y + b.height
        t.ok(apart, `box at {b.x},{b.y}`)
      end
    end)
  end)

  t.group("frames", function()
    t.test("pops in beside its first point, glides to the next, then taps", function()
      local model, st = desk()
      live.feed(model, click(1, 650, 320.4), T)
      local first = frame(model, st, T)
      t.eq(first.output, "HDMI-A-1")
      t.eq(first.opacity, 0)
      t.ok(first.moving, "moving while popping in")
      local popped = frame(model, st, T + stage.POP_MS)
      t.eq(popped.opacity, 1)
      t.ok(not popped.moving, "still once in")
      t.eq(frame(model, st, T + stage.GLIDE_MS + 10).pose, "tap")
      local still = frame(model, st, T + stage.GLIDE_MS + stage.TAP_MS + 10)
      t.eq(still.pose, "idle")
      t.ok(not still.moving)

      live.feed(model, click(2, 1500, 900), T + 1000)
      local leaving = frame(model, st, T + 1000)
      local midway = frame(model, st, T + 1070)
      local there = frame(model, st, T + 1000 + stage.GLIDE_MS)
      t.ok(midway.moving, "moving while gliding")
      t.eq(midway.pose, "lean-r")
      t.ok(midway.x > leaving.x and midway.x < there.x, "moving toward the new point")
      t.ok(not there.moving)
      t.eq(frame(model, st, T + 1000 + stage.GLIDE_MS + 10).pose, "tap")
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
      t.eq(quiet.x, looked.x + (stage.INNER + looked.width), "the caption went, the robot stayed")
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
      local from = stage.besidePoint("HDMI-A-1", { x = 80.4, y = 80.4 }, HDMI, 1, 1)
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
          live.feed(model, event, nowMs)
          quiet = true
        elseif kind == 5 then
          local choice = pick(5)
          holder = if choice == 1 then nil else fx.HOLDER
          local extra = { stopped = pick(6) == 1, pending = choice == 2 }
          live.feed(model, fx.state(if holder then holder else false, extra), nowMs)
        elseif kind == 6 then
          live.feed(model, { event = "hello", format = 1 }, nowMs)
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
        local f = stage.step(st, view, nowMs, ENV)
        local label = `seed {seed} at {nowMs - T}`
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
