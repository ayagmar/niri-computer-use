--!strict
-- Folds the lines `niri-computer-use watch` prints into one table the bar, the
-- panel and the glow draw, and says which changes are worth a notification.
-- No noctalia calls: the service decodes each line and passes the clock.

local describe = require("./describe.luau")

local M = {}

-- The watch format this plugin reads.
M.FORMAT = 1
-- How many calls the panel keeps.
M.KEEP = 50
-- `watch` sends `alive` every five seconds; this long without a line means it ended.
M.QUIET_MS = 15000

type Tr = (key: string, vars: { [string]: any }?) -> string

export type Lease = {
  session: string,
  client: string,
  project: string?,
  since: string,
  pending: boolean,
}

export type Call = {
  key: string,
  text: string,
  glyph: string,
  outcome: string,
  error: string?,
  session: string,
  client: string,
  project: string?,
  ts: string,
  durationMs: number?,
}

export type Change =
  { kind: "took", lease: Lease }
  | { kind: "asking", lease: Lease }
  | { kind: "released", lease: Lease }
  | { kind: "stopped" }
  | { kind: "resumed" }
  | { kind: "stuck" }
  | { kind: "failed", call: Call }

export type Desk = {
  lease: Lease?,
  stopped: boolean,
  inputDirty: boolean,
  calls: { Call },
  count: number,
  version: string?,
  -- `missing`, `old`, `format` or the text of a line that wasn't JSON, such as watch's error.
  problem: string?,
  heardMs: number?,
}

function M.new(): Desk
  return { lease = nil, stopped = false, inputDirty = false, calls = {}, count = 0, version = nil, problem = nil, heardMs = nil }
end

local function lease(value: any): Lease?
  if type(value) ~= "table" or type(value.session) ~= "string" then
    return nil
  end
  return {
    session = value.session,
    client = describe.client(value.session),
    project = if type(value.project) == "string" then value.project else nil,
    since = tostring(value.since or ""),
    pending = value.pending == true,
  }
end

local function leaseChanges(before: Lease?, after: Lease?, changes: { Change })
  local same = before and after and before.session == after.session
  if before and not same then
    table.insert(changes, { kind = "released", lease = before })
  end
  if after and (not same or (before :: Lease).pending ~= after.pending) then
    table.insert(changes, { kind = if after.pending then "asking" else "took", lease = after } :: Change)
  end
end

local function state(desk: Desk, event: any, changes: { Change })
  local after = lease(event.lease)
  leaseChanges(desk.lease, after, changes)
  desk.lease = after
  local stopped = event.stopped == true
  if stopped ~= desk.stopped then
    table.insert(changes, { kind = if stopped then "stopped" else "resumed" } :: Change)
  end
  desk.stopped = stopped
  local dirty = event.input_dirty == true
  if dirty and not desk.inputDirty then
    table.insert(changes, { kind = "stuck" })
  end
  desk.inputDirty = dirty
end

local function call(desk: Desk, event: any, tr: Tr, changes: { Change })
  local tool = tostring(event.tool or "?")
  local session = tostring(event.session or "?")
  desk.count += 1
  local entry: Call = {
    key = `{desk.count}`,
    text = describe.say(tool, event.args, tr),
    glyph = describe.glyph(tool),
    outcome = describe.outcome(event),
    error = if type(event.error) == "string" then event.error else nil,
    session = session,
    client = describe.client(session),
    project = if type(event.project) == "string" then event.project else nil,
    ts = tostring(event.ts or ""),
    durationMs = if type(event.duration_ms) == "number" then event.duration_ms else nil,
  }
  table.insert(desk.calls, entry)
  if #desk.calls > M.KEEP then
    table.remove(desk.calls, 1)
  end
  if entry.outcome == "error" and entry.error ~= "cancelled" then
    table.insert(changes, { kind = "failed", call = entry })
  end
end

local function hello(desk: Desk, event: any)
  if event.format ~= M.FORMAT then
    desk.problem = "format"
    return
  end
  desk.problem = nil
  desk.version = if type(event.version) == "string" then event.version else nil
end

-- Folds one line in: `event` is the decoded line, or nil when it wasn't JSON,
-- and `raw` its text. Returns what changed, oldest first.
function M.feed(desk: Desk, event: any, raw: string, nowMs: number, tr: Tr): { Change }
  desk.heardMs = nowMs
  local changes: { Change } = {}
  local kind = if type(event) == "table" then event.event else nil
  if kind == "hello" then
    hello(desk, event)
  elseif kind == "missing" then
    desk.problem = "missing"
  elseif desk.problem == "format" then
    return changes
  elseif kind == "state" then
    state(desk, event, changes)
  elseif kind == "action" then
    call(desk, event, tr, changes)
  elseif kind == nil and raw:find("usage: niri-computer-use", 1, true) then
    -- A build before `watch` prints its usage line for the unknown subcommand.
    desk.problem = "old"
  elseif kind == nil and raw ~= "" then
    desk.problem = raw
  end
  return changes
end

-- One word for the desk: `missing`, `error`, `starting`, `offline`, `stopped`,
-- `asking`, `driving` or `idle`.
function M.status(desk: Desk, nowMs: number): string
  if desk.problem == "missing" then
    return "missing"
  elseif desk.problem then
    return "error"
  elseif desk.heardMs == nil then
    return "starting"
  elseif nowMs - desk.heardMs > M.QUIET_MS then
    return "offline"
  elseif desk.stopped then
    return "stopped"
  elseif desk.lease and desk.lease.pending then
    return "asking"
  elseif desk.lease then
    return "driving"
  end
  return "idle"
end

-- The plain table the service publishes for the surfaces.
function M.view(desk: Desk, nowMs: number)
  return {
    status = M.status(desk, nowMs),
    lease = desk.lease,
    stopped = desk.stopped,
    inputDirty = desk.inputDirty,
    calls = table.clone(desk.calls),
    version = desk.version,
    problem = desk.problem,
  }
end

return M
