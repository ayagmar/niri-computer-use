--!strict
-- Where the robot stands and what it shows, from the `live` view and the clock: the
-- output, the top-left corner of the robot and its caption in that output's logical
-- pixels, the pose, the caption, and when to draw next. Every placement and animation decision lives here; robot.luau only draws the
-- result. No noctalia calls: the layer passes the clock, its outputs and the UI scale.
--
-- While the agent looks at the screen (a call's `look` phase, until its `end`), nothing
-- the robot shows changes, so the robot never keeps the agent's own settle wait going.

local describe = require("./describe.luau")
local live = require("./live.luau")

local M = {}

-- Sizes in logical pixels at a UI scale of 1.
M.SPRITE = 32
M.CAPTION_MIN = 40
M.CAPTION_MAX = 168
-- Between the robot and its caption.
M.INNER = 6
-- Between the robot and the point or box it acts on.
M.GAP = 12
-- Kept clear of the output's edges.
M.MARGIN = 8
-- The robot's spot when it has no target: this far in from the bottom-right corner.
M.HOME = 48

M.POP_MS = 120
M.GLIDE_MS = 140
M.DRAG_MS = 300
-- How often the robot is drawn while it pops in or glides.
M.FRAME_MS = 16
M.TAP_MS = 90
M.SCROLL_MS = 120
M.SCROLL_SWAPS = 4
M.TYPE_MS = 140
M.CAPTION_MS = 2500
M.SHRUG_MS = 900
M.ASK_MS = 500
M.GOODBYE_MS = 1200
M.WAVE_MS = 300
M.DOZE_MS = 8000
-- How long the robot stays beside its last target for an action without one.
M.SPOT_MS = 30000

-- Every frame, as robot/<name>.svg.
M.POSES = {
  "idle",
  "lean-l",
  "lean-r",
  "tap",
  "type-a",
  "type-b",
  "scroll-up",
  "scroll-down",
  "grab",
  "shrug",
  "wave-a",
  "wave-b",
  "doze",
  "stop",
}

type Tr = (key: string, vars: { [string]: any }?) -> string
type Point = live.Point
type Rect = live.Rect
type Area = { width: number, height: number }

export type Env = {
  -- Noctalia's UI scale, which sizes the robot and its caption.
  scale: number,
  -- The output the robot waits on when it has no target.
  focused: string,
  -- Each output's logical size, for one no action has named yet.
  areas: { [string]: Area },
  tr: Tr,
}

-- The robot's top-left corner on an output, with its caption on `side`.
export type Spot = { output: string, x: number, y: number, side: string }

-- What the layer remembers between frames: where the robot is headed, where a glide
-- started, and when it popped in.
export type Stage = {
  at: Spot?,
  from: Spot?,
  startMs: number,
  durationMs: number,
  popMs: number?,
  -- The frame last drawn, held while the agent looks at another output.
  last: Frame?,
}

export type Frame = {
  visible: boolean,
  output: string?,
  -- The top-left corner of the robot and its caption, in whole logical pixels.
  x: number,
  y: number,
  pose: string,
  caption: string?,
  -- The caption's palette role and its width in logical pixels at a UI scale of 1.
  role: string,
  width: number,
  side: string,
  opacity: number,
  -- Popping in or gliding, so nextMs is the next frame's.
  moving: boolean,
  -- When the next change is due without a new record, if sooner than the usual tick.
  nextMs: number?,
}

type Target = {
  -- "point", "box", "drag", "home", "rest" (stay where it stands), or "window" for a
  -- window without a box (stay if it got there within SPOT_MS before the call's aim).
  kind: string,
  output: string,
  aimMs: number?,
  point: Point?,
  to: Point?,
  box: Rect?,
}

type Scene = {
  target: Target,
  pose: string,
  caption: string?,
  role: string,
  nextMs: number?,
  -- The agent is looking: snap to the spot and change nothing.
  still: boolean,
  drag: boolean,
}

function M.new(): Stage
  return { at = nil, from = nil, startMs = 0, durationMs = 0, popMs = nil, last = nil }
end

local function soonest(a: number?, b: number?): number?
  if a == nil then
    return b
  elseif b == nil then
    return a
  end
  return math.min(a, b)
end

-- Which of two frames shows `nowMs`, swapping every `everyMs` from `startMs`, and when
-- the next swap is due.
local function alternate(a: string, b: string, startMs: number, nowMs: number, everyMs: number): (string, number)
  local swaps = math.max(0, (nowMs - startMs) // everyMs)
  return if swaps % 2 == 0 then a else b, startMs + (swaps + 1) * everyMs
end

-- The caption's width in logical pixels at a UI scale of 1, estimated from its length.
function M.captionWidth(text: string): number
  local chars = utf8.len(text) or #text
  return math.clamp(7 * chars + 16, M.CAPTION_MIN, M.CAPTION_MAX)
end

-- Whether a robot at `spot`, with the widest caption on its side, keeps clear of the edges.
local function fits(spot: Spot, area: Area, scale: number): boolean
  local robot = M.SPRITE * scale
  local wide = (M.INNER + M.CAPTION_MAX) * scale
  local left = if spot.side == "left" then spot.x - wide else spot.x
  local right = if spot.side == "left" then spot.x + robot else spot.x + robot + wide
  return left >= M.MARGIN
    and spot.y >= M.MARGIN
    and right <= area.width - M.MARGIN
    and spot.y + robot <= area.height - M.MARGIN
end

-- `spot` moved as little as needed to keep clear of the edges, where the area allows.
local function clamped(spot: Spot, area: Area, scale: number): Spot
  local robot = M.SPRITE * scale
  local wide = (M.INNER + M.CAPTION_MAX) * scale
  local minX = if spot.side == "left" then M.MARGIN + wide else M.MARGIN
  local maxX = if spot.side == "left" then area.width - M.MARGIN - robot else area.width - M.MARGIN - robot - wide
  local maxY = area.height - M.MARGIN - robot
  return {
    output = spot.output,
    x = math.clamp(spot.x, minX, math.max(minX, maxX)),
    y = math.clamp(spot.y, M.MARGIN, math.max(M.MARGIN, maxY)),
    side = spot.side,
  }
end

-- Above-left first, then above-right, below-left and below-right, so menus and
-- tooltips that open below-right of the pointer stay clear.
local CORNERS = { { -1, -1 }, { 1, -1 }, { -1, 1 }, { 1, 1 } }

-- The spot beside point `p`, its nearest corner GAP away on both axes, and which corner
-- was taken. `only` takes that corner, as a drag's start does its end's.
function M.besidePoint(output: string, p: Point, area: Area, scale: number, only: number?): (Spot, number)
  local robot = M.SPRITE * scale
  local function corner(i: number): Spot
    local c = CORNERS[i]
    return {
      output = output,
      x = if c[1] < 0 then p.x - M.GAP - robot else p.x + M.GAP,
      y = if c[2] < 0 then p.y - M.GAP - robot else p.y + M.GAP,
      side = if c[1] < 0 then "left" else "right",
    }
  end
  for i = 1, #CORNERS do
    if (only == nil or only == i) and fits(corner(i), area, scale) then
      return corner(i), i
    end
  end
  local i = only or 1
  return clamped(corner(i), area, scale), i
end

-- The spot beside box `b`: right of it, left, below or above, or inside its top-right
-- corner when it fills the output.
function M.besideBox(output: string, b: Rect, area: Area, scale: number): Spot
  local robot = M.SPRITE * scale
  local cx, cy = b.x + b.width / 2 - robot / 2, b.y + b.height / 2 - robot / 2
  local options: { Spot } = {
    { output = output, x = b.x + b.width + M.GAP, y = cy, side = "right" },
    { output = output, x = b.x - M.GAP - robot, y = cy, side = "left" },
    { output = output, x = cx, y = b.y + b.height + M.GAP, side = "right" },
    { output = output, x = cx, y = b.y - M.GAP - robot, side = "right" },
  }
  for _, spot in options do
    if fits(spot, area, scale) then
      return spot
    end
  end
  local inside = { output = output, x = b.x + b.width - M.MARGIN - robot, y = b.y + M.MARGIN, side = "left" }
  return clamped(inside, area, scale)
end

function M.home(output: string, area: Area, scale: number): Spot
  local robot = M.SPRITE * scale
  local spot = { output = output, x = area.width - M.HOME - robot, y = area.height - M.HOME - robot, side = "left" }
  return clamped(spot, area, scale)
end

local function areaOf(view: any, env: Env, output: string): Area
  local call = view.call
  if call and call.aim and call.aim.output == output then
    return call.aim
  end
  local areas = view.areas or {}
  return areas[output] or env.areas[output] or { width = 1920, height = 1080 }
end

-- Where `call` acts, or where the robot rests for it.
local function callTarget(call: any, st: Stage, env: Env): Target
  local aim = call.aim
  if aim == nil then
    return { kind = "rest", output = if st.at then st.at.output else env.focused }
  end
  local points = aim.points or {}
  if call.tool == "drag" and #points >= 2 then
    return { kind = "drag", output = aim.output, point = points[1], to = points[#points] }
  elseif #points >= 1 then
    return { kind = "point", output = aim.output, point = points[1] }
  elseif aim.box then
    return { kind = "box", output = aim.output, box = aim.box }
  end
  return { kind = "window", output = aim.output, aimMs = call.aimMs or call.startMs }
end

-- A call's pose while it runs, and when the pose changes next. Poses that answer the
-- action itself start once the robot has arrived, or at `end` when the agent looked.
local function workingPose(call: any, fromMs: number, nowMs: number): (string, number?)
  local tool, args = call.tool, call.args or {}
  if tool == "click" then
    local untilMs = fromMs + M.TAP_MS * (if args.count == 2 then 2 else 1)
    if nowMs < fromMs then
      return "idle", fromMs
    end
    return if nowMs < untilMs then "tap" else "idle", if nowMs < untilMs then untilMs else nil
  elseif tool == "scroll" then
    local up = type(args.notches_y) ~= "number" or args.notches_y < 0
    local untilMs = fromMs + M.SCROLL_MS * M.SCROLL_SWAPS
    if nowMs < fromMs or nowMs >= untilMs then
      return "idle", if nowMs < fromMs then fromMs else nil
    end
    local a, b = if up then "scroll-up" else "scroll-down", if up then "scroll-down" else "scroll-up"
    return alternate(a, b, fromMs, nowMs, M.SCROLL_MS)
  elseif tool == "drag" then
    return "grab", nil
  end
  return "idle", nil
end

local function typing(call: any, nowMs: number): (string, number?)
  local fromMs = call.aimMs or call.startMs
  if call.tool == "key" and nowMs >= fromMs + 2 * M.TYPE_MS then
    return "idle", nil
  end
  return alternate("type-a", "type-b", fromMs, nowMs, M.TYPE_MS)
end

local TYPING = { type_text = true, paste = true, key = true, set_element_text = true }

local function running(call: any, target: Target, caption: string?, nowMs: number): Scene
  if call.phase == "look" then
    return { target = target, pose = "idle", caption = caption, role = "primary", still = true, drag = false }
  end
  local pose, nextMs
  if TYPING[call.tool] then
    pose, nextMs = typing(call, nowMs)
  else
    local glide = if call.tool == "drag" then M.DRAG_MS else M.GLIDE_MS
    pose, nextMs = workingPose(call, (call.aimMs or call.startMs) + glide, nowMs)
  end
  return { target = target, pose = pose, caption = caption, role = "primary", nextMs = nextMs, still = false, drag = call.tool == "drag" }
end

-- A call that ended within CAPTION_MS: its caption, a shrug for an error, and the tap
-- or scroll it held back while the agent looked.
local function ended(call: any, target: Target, caption: string?, nowMs: number, tr: Tr): Scene
  local endMs = call.endMs :: number
  local nextMs = endMs + M.CAPTION_MS
  local failed = call.error ~= nil and call.error ~= "cancelled"
  if failed then
    local shrugging = nowMs < endMs + M.SHRUG_MS
    return {
      target = target,
      pose = if shrugging then "shrug" else "idle",
      caption = describe.failure(call.error),
      role = "error",
      nextMs = if shrugging then endMs + M.SHRUG_MS else nextMs,
      still = false,
      drag = false,
    }
  end
  local pose, poseMs = "idle", nil
  if call.lookMs then
    pose, poseMs = workingPose(call, endMs, nowMs)
  end
  return { target = target, pose = pose, caption = caption, role = "primary", nextMs = soonest(poseMs, nextMs), still = false, drag = false }
end

local function resting(output: string, pose: string, caption: string?, role: string, nextMs: number?): Scene
  return {
    target = { kind = "rest", output = output },
    pose = pose,
    caption = caption,
    role = role,
    nextMs = nextMs,
    still = false,
    drag = false,
  }
end

local function waiting(view: any, nowMs: number, output: string): Scene
  local dozeMs = (view.heardMs or 0) + M.DOZE_MS
  return resting(output, if nowMs >= dozeMs then "doze" else "idle", nil, "primary", if nowMs < dozeMs then dozeMs else nil)
end

-- A session whose server writes no `live` records: the robot waits at home and shows
-- each acting call's caption from its `action` line.
local function legacy(view: any, nowMs: number, env: Env): Scene
  local call = view.legacy
  local home = { kind = "home", output = env.focused }
  if call == nil or nowMs - call.atMs >= M.CAPTION_MS then
    local scene = waiting(view, nowMs, env.focused)
    scene.target = home
    return scene
  end
  local failed = call.error ~= nil and call.error ~= "cancelled"
  local shrugging = failed and nowMs < call.atMs + M.SHRUG_MS
  local caption = if failed then describe.failure(call.error) else describe.doing(call.tool, call.args, nil, env.tr)
  return {
    target = home,
    pose = if shrugging then "shrug" else "idle",
    caption = caption,
    role = if failed then "error" else "primary",
    nextMs = if shrugging then call.atMs + M.SHRUG_MS else call.atMs + M.CAPTION_MS,
    still = false,
    drag = false,
  }
end

-- The holder's call, unless it went quiet for STALE_MS without an end.
local function openCall(view: any, nowMs: number): any
  local call = view.call
  if call == nil or (call.phase ~= "end" and nowMs - (view.heardMs or 0) >= live.STALE_MS) then
    return nil
  end
  return call
end

local function held(view: any, nowMs: number, st: Stage, env: Env): Scene
  local call = openCall(view, nowMs)
  if view.stopped then
    local target = if call then callTarget(call, st, env) else { kind = "rest", output = if st.at then st.at.output else env.focused }
    return { target = target, pose = "stop", caption = env.tr("overlay.stopped"), role = "error", still = false, drag = false }
  elseif not view.live then
    return legacy(view, nowMs, env)
  elseif call == nil then
    return waiting(view, nowMs, if st.at then st.at.output else env.focused)
  end
  local target = callTarget(call, st, env)
  local caption = describe.doing(call.tool, call.args, call.app, env.tr)
  if call.phase ~= "end" then
    return running(call, target, caption, nowMs)
  elseif nowMs < call.endMs + M.CAPTION_MS then
    return ended(call, target, caption, nowMs, env.tr)
  end
  local scene = waiting(view, nowMs, target.output)
  scene.target = target
  return scene
end

-- What the robot does now, before it is placed, or nil when it is gone.
local function scene(view: any, nowMs: number, st: Stage, env: Env): Scene?
  if type(view) ~= "table" then
    return nil
  end
  local holder = view.holder
  if holder == nil then
    local released = view.releasedMs
    if released == nil or nowMs - released >= M.GOODBYE_MS then
      return nil
    end
    local pose, nextMs = alternate("wave-a", "wave-b", released, nowMs, M.WAVE_MS)
    local output = if st.at then st.at.output else env.focused
    return resting(output, pose, nil, "primary", math.min(nextMs, released + M.GOODBYE_MS))
  elseif holder.pending then
    local pose, nextMs = alternate("wave-a", "wave-b", 0, nowMs, M.ASK_MS)
    local ask = resting(env.focused, pose, env.tr("overlay.asking"), "tertiary", nextMs)
    ask.target = { kind = "home", output = env.focused }
    return ask
  end
  return held(view, nowMs, st, env)
end

-- The spot a target puts the robot on, and for a drag where its glide starts. Only a
-- record moves the robot: resting, it stays where it stands on that output.
local function place(target: Target, view: any, st: Stage, env: Env, still: boolean): (Spot, Spot?)
  local area = areaOf(view, env, target.output)
  local scale = env.scale
  if target.kind == "point" then
    return (M.besidePoint(target.output, target.point :: Point, area, scale))
  elseif target.kind == "drag" then
    local to, corner = M.besidePoint(target.output, target.to :: Point, area, scale)
    local from = M.besidePoint(target.output, target.point :: Point, area, scale, corner)
    return to, from
  elseif target.kind == "box" then
    return M.besideBox(target.output, target.box :: Rect, area, scale)
  end
  local at = st.at
  local stays = at ~= nil and at.output == target.output and target.kind ~= "home"
  if stays and target.kind == "window" and not still then
    stays = (target.aimMs :: number) - st.startMs <= M.SPOT_MS
  end
  if stays then
    return at :: Spot
  end
  return M.home(target.output, area, scale)
end

local function sameSpot(a: Spot?, b: Spot): boolean
  return a ~= nil and a.output == b.output and a.x == b.x and a.y == b.y and a.side == b.side
end

local function ease(t: number): number
  return 1 - (1 - t) ^ 3
end

-- Where the robot is drawn now: along its glide, or at its spot.
local function position(st: Stage, nowMs: number): (number, number, boolean)
  local at = st.at :: Spot
  local from = st.from
  if from == nil or nowMs >= st.startMs + st.durationMs then
    return at.x, at.y, false
  end
  local t = ease(math.max(0, nowMs - st.startMs) / st.durationMs)
  return from.x + (at.x - from.x) * t, from.y + (at.y - from.y) * t, true
end

-- Moves the stage toward `spot`: a pop-in on a new output, a glide on the same one, and
-- a snap while the agent looks.
local function move(st: Stage, spot: Spot, from: Spot?, still: boolean, nowMs: number)
  if still then
    if not sameSpot(st.at, spot) or st.from then
      st.startMs = nowMs
    end
    st.at, st.from, st.popMs = spot, nil, nil
    return
  end
  if st.at == nil or st.at.output ~= spot.output then
    st.at, st.from, st.popMs, st.startMs = spot, nil, nowMs, nowMs
    return
  end
  if sameSpot(st.at, spot) then
    return
  end
  local x, y = position(st, nowMs)
  local start = from or { output = spot.output, x = x, y = y, side = spot.side }
  st.from, st.at, st.startMs = start, spot, nowMs
  st.durationMs = if from then M.DRAG_MS else M.GLIDE_MS
end

local HIDDEN: Frame = {
  visible = false,
  output = nil,
  x = 0,
  y = 0,
  pose = "idle",
  caption = nil,
  role = "primary",
  width = 0,
  side = "left",
  opacity = 0,
  moving = false,
  nextMs = nil,
}

-- The frame to draw at `nowMs`, advancing `st`.
function M.step(st: Stage, view: any, nowMs: number, env: Env): Frame
  local found = scene(view, nowMs, st, env)
  if found == nil then
    st.at, st.from, st.popMs, st.last = nil, nil, nil, nil
    return table.clone(HIDDEN)
  end
  local spot, from = place(found.target, view, st, env, found.still)
  -- Moving to another output makes Noctalia build the layer there, which draws its first
  -- frame some time later, in the middle of the agent's look. So while it looks, the
  -- robot stays as it was, or stays away, and moves at the call's end.
  if found.still and (st.at == nil or (st.at :: Spot).output ~= spot.output) then
    local held = if st.last then table.clone(st.last :: Frame) else table.clone(HIDDEN)
    held.moving, held.nextMs = false, nil
    return held
  end
  move(st, spot, if found.drag then from else nil, found.still, nowMs)
  local x, y, gliding = position(st, nowMs)
  local popping = st.popMs ~= nil and nowMs < st.popMs + M.POP_MS
  local pose = found.pose
  if gliding and not found.drag then
    pose = if (st.at :: Spot).x < (st.from :: Spot).x then "lean-l" else "lean-r"
  end
  -- The caption never moves: it shows once the robot has arrived.
  local caption = if gliding then nil else found.caption
  local width = if caption then M.captionWidth(caption) else 0
  local side = (st.at :: Spot).side
  if caption and side == "left" then
    x -= (M.INNER + width) * env.scale
  end
  local nextMs = found.nextMs
  -- An update interval rather than frame ticks drives a pop-in or a glide: Noctalia
  -- sends a frame tick only after a redraw, and a frame that changes nothing draws
  -- nothing, which ended the ticks halfway through a glide.
  local moving = gliding or popping
  if moving then
    nextMs = nowMs + M.FRAME_MS
  end
  if nextMs and nextMs <= nowMs then
    nextMs = nil
  end
  local frame: Frame = {
    visible = true,
    output = spot.output,
    x = math.floor(x + 0.5),
    y = math.floor(y + 0.5),
    pose = pose,
    caption = caption,
    role = found.role,
    width = width,
    side = side,
    opacity = if popping then math.floor((nowMs - (st.popMs :: number)) / M.POP_MS * 4) / 4 else 1,
    moving = moving,
    nextMs = nextMs,
  }
  st.last = frame
  return frame
end

-- What changes the drawn tree.
function M.key(frame: Frame): string
  if not frame.visible then
    return "hidden"
  end
  return `{frame.x},{frame.y}|{frame.pose}|{frame.caption or ""}|{frame.role}|{frame.side}|{frame.width}|{frame.opacity}`
end

return M
