--!strict
-- Folds what the robot overlay draws from `watch`'s lines: who holds the lease, the
-- holder's action in progress from `live` lines, and, for a session whose server writes
-- no `live` records (an older build), its acting calls from `action` lines. Only the
-- holder's records from since it took the lease count, so another session's record can't
-- end the holder's call. No noctalia calls: the service decodes each line and passes the
-- clock.

local describe = require("./describe.luau")
local time = require("./time.luau")

local M = {}

-- A call with no record this long after its last one is over, should its end be lost.
M.STALE_MS = 120000
-- The record `watch` finds when it starts is history when it is older than this.
M.BACKLOG_MS = 60000

local RANK = { start = 1, aim = 2, look = 3, ["end"] = 4 }

export type Point = { x: number, y: number }
export type Rect = { x: number, y: number, width: number, height: number }

-- Where on which output an action acts, in the output's logical pixels.
export type Aim = {
  output: string,
  width: number,
  height: number,
  points: { Point },
  box: Rect?,
}

export type Call = {
  pid: number,
  call: number,
  tool: string,
  args: any,
  phase: string,
  aim: Aim?,
  -- niri's app id for the window the call acts on.
  app: string?,
  -- The outcome's error name, once ended with one.
  error: string?,
  -- When each phase was folded in, in the service's clock.
  startMs: number,
  aimMs: number?,
  lookMs: number?,
  endMs: number?,
}

-- An acting call of a session without `live` records, from its `action` line.
export type Legacy = { tool: string, args: any, error: string?, atMs: number }

export type Holder = { session: string, sinceMs: number, pending: boolean }

export type Live = {
  holder: Holder?,
  stopped: boolean,
  call: Call?,
  -- Sessions that sent a `live` record while holding the lease.
  live: { [string]: boolean },
  -- The last area seen per output.
  areas: { [string]: { width: number, height: number } },
  legacy: Legacy?,
  -- When the last holder gave the desktop back, for the goodbye.
  releasedMs: number?,
  -- When the lease was taken or the holder's last record or call came.
  heardMs: number,
  -- Counts the changes, so the service publishes only when something changed.
  rev: number,
}

function M.new(): Live
  return {
    holder = nil,
    stopped = false,
    call = nil,
    live = {},
    areas = {},
    legacy = nil,
    releasedMs = nil,
    heardMs = 0,
    rev = 0,
  }
end

local function number(value: any): number?
  if type(value) == "number" and value == value and math.abs(value) < 1e7 then
    return value
  end
  return nil
end

local function point(value: any): Point?
  if type(value) ~= "table" then
    return nil
  end
  local x, y = number(value.x), number(value.y)
  return if x and y then { x = x, y = y } else nil
end

local function rect(value: any): Rect?
  if type(value) ~= "table" then
    return nil
  end
  local x, y, width, height = number(value.x), number(value.y), number(value.width), number(value.height)
  if x and y and width and height and width > 0 and height > 0 then
    return { x = x, y = y, width = width, height = height }
  end
  return nil
end

-- A record's aim, or nil for one without a usable output and area.
function M.aim(value: any): Aim?
  if type(value) ~= "table" or type(value.output) ~= "string" or type(value.area) ~= "table" then
    return nil
  end
  local width, height = number(value.area.width), number(value.area.height)
  if not (width and height and width > 0 and height > 0) then
    return nil
  end
  local points: { Point } = {}
  if type(value.points) == "table" then
    for _, raw in value.points do
      local found = point(raw)
      if found and #points < 2 then
        table.insert(points, found)
      end
    end
  end
  return { output = value.output, width = width, height = height, points = points, box = rect(value.box) }
end

local function holder(value: any): Holder?
  if type(value) ~= "table" or type(value.session) ~= "string" then
    return nil
  end
  return { session = value.session, sinceMs = time.parse(value.since) or 0, pending = value.pending == true }
end

local function sameHolder(a: Holder?, b: Holder?): boolean
  if a == nil or b == nil then
    return a == b
  end
  return a.session == b.session and a.sinceMs == b.sinceMs and a.pending == b.pending
end

local function state(model: Live, event: any, nowMs: number): boolean
  local before, after = model.holder, holder(event.lease)
  local stopped = event.stopped == true
  if sameHolder(before, after) and stopped == model.stopped then
    return false
  end
  local newLease = after ~= nil and (before == nil or before.session ~= after.session or before.sinceMs ~= after.sinceMs)
  if before and not after then
    model.releasedMs = nowMs
  end
  if newLease or not after then
    model.call = nil
    model.legacy = nil
  end
  if newLease then
    model.releasedMs = nil
    model.heardMs = nowMs
  end
  model.holder = after
  model.stopped = stopped
  return true
end

-- The holder's session if `event` is its, from since it took the lease.
local function fromHolder(model: Live, event: any): Holder?
  local current = model.holder
  if current == nil or current.pending or event.session ~= current.session then
    return nil
  end
  local ts = time.parse(event.ts)
  if ts == nil or ts < current.sinceMs then
    return nil
  end
  return current
end

local function apply(call: Call, event: any, phase: string, nowMs: number, model: Live)
  call.phase = phase
  local aim = M.aim(event.aim)
  if aim then
    call.aim = aim
    model.areas[aim.output] = { width = aim.width, height = aim.height }
  end
  if type(event.window) == "table" and type(event.window.app_id) == "string" then
    call.app = event.window.app_id
  end
  if type(event.outcome) == "table" and type(event.outcome.error) == "string" then
    call.error = event.outcome.error
  end
  if phase == "aim" then
    call.aimMs = nowMs
  elseif phase == "look" then
    call.lookMs = nowMs
  elseif phase == "end" then
    call.endMs = nowMs
  end
end

-- Whether a record of call `n` from `pid` replaces the current call: any other key does,
-- except an earlier call of the same server, whose record came late.
local function replaces(current: Call?, pid: number, n: number): boolean
  return current == nil or current.pid ~= pid or n > current.call
end

local function record(model: Live, event: any, nowMs: number): boolean
  local found = fromHolder(model, event)
  local pid, n = number(event.pid), number(event.call)
  local rank = RANK[event.phase]
  if found == nil or pid == nil or n == nil or rank == nil then
    return false
  end
  local newly = not model.live[found.session]
  model.live[found.session] = true
  local ts = time.parse(event.ts) :: number
  if event.backlog == true and (event.phase == "end" or nowMs - ts > M.BACKLOG_MS) then
    return newly
  end
  local current = model.call
  if current and current.pid == pid and current.call == n then
    if rank <= RANK[current.phase] then
      return newly
    end
  elseif replaces(current, pid, n) then
    current = {
      pid = pid,
      call = n,
      tool = tostring(event.tool or "?"),
      args = if type(event.args) == "table" then event.args else {},
      phase = "start",
      startMs = nowMs,
    }
    model.call = current
  else
    return newly
  end
  apply(current :: Call, event, event.phase, nowMs, model)
  model.legacy = nil
  model.heardMs = nowMs
  return true
end

local function action(model: Live, event: any, nowMs: number): boolean
  local found = fromHolder(model, event)
  local tool = tostring(event.tool or "?")
  if found == nil or model.live[found.session] or event.backlog == true or not describe.acts(tool) then
    return false
  end
  model.legacy = {
    tool = tool,
    args = if type(event.args) == "table" then event.args else {},
    error = if type(event.error) == "string" then event.error else nil,
    atMs = nowMs,
  }
  model.heardMs = nowMs
  return true
end

-- Folds one decoded `watch` line in. Returns whether what the overlay draws may change.
function M.feed(model: Live, event: any, nowMs: number): boolean
  local kind = if type(event) == "table" then event.event else nil
  local changed = false
  if kind == "hello" then
    -- A new watch: its first `live` line, if any, says again what is in progress.
    changed = model.call ~= nil or model.legacy ~= nil
    model.call = nil
    model.legacy = nil
  elseif kind == "state" then
    changed = state(model, event, nowMs)
  elseif kind == "live" then
    changed = record(model, event, nowMs)
  elseif kind == "action" then
    changed = action(model, event, nowMs)
  end
  if changed then
    model.rev += 1
  end
  return changed
end

-- The plain table the service publishes as the `live` state for the overlay.
function M.view(model: Live)
  local current = model.holder
  return {
    rev = model.rev,
    holder = if current then { session = current.session, pending = current.pending } else nil,
    stopped = model.stopped,
    -- The holder's server writes `live` records; otherwise only `legacy` shows its calls.
    live = current ~= nil and model.live[current.session] == true,
    call = model.call,
    areas = model.areas,
    legacy = model.legacy,
    releasedMs = model.releasedMs,
    heardMs = model.heardMs,
  }
end

return M
