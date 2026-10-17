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
-- An agent that gives the desktop back and takes it again within this long, as
-- agents do before asking the user something, took it once.
M.RETAKE_MS = 5 * 60 * 1000

-- Errors that are an agent's own mistake, which it corrects itself: never a failure
-- worth a notification.
local MISTAKES = {
  cancelled = true,
  invalid_arguments = true,
  ref_invalid = true,
  element_stale = true,
}

type Tr = (key: string, vars: { [string]: any }?) -> string

export type Lease = {
  session: string,
  client: string,
  project: string?,
  since: string,
  pending: boolean,
}

-- One agent's server on this desktop.
export type Agent = {
  pid: number,
  name: string,
  project: string?,
  -- An update replaced its binary: it runs the old build until restarted.
  replaced: boolean,
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
  | { kind: "retook", lease: Lease }
  | { kind: "asking", lease: Lease }
  | { kind: "released", lease: Lease }
  | { kind: "stopped" }
  | { kind: "resumed" }
  | { kind: "stuck" }
  | { kind: "outdated", count: number }
  | { kind: "failed", call: Call }

export type Desk = {
  lease: Lease?,
  stopped: boolean,
  inputDirty: boolean,
  calls: { Call },
  agents: { Agent },
  count: number,
  version: string?,
  -- `missing`, `old`, `format` or the text of a line that wasn't JSON, such as watch's error.
  problem: string?,
  heardMs: number?,
  -- The last agent to give the desktop back, and when.
  released: { session: string, atMs: number }?,
  -- A state was read since `hello`. The first one shows how the desk already
  -- was, not a change, so it notifies only of stuck input.
  settled: boolean,
  -- This watch said hello. A later line that isn't JSON is its last words as
  -- it exits, or the tail of a line Noctalia cut, which a JSON line follows.
  greeted: boolean,
  -- That line, until a JSON line follows it: shown once the watch is silent.
  lastError: string?,
}

function M.new(): Desk
  return {
    lease = nil,
    stopped = false,
    inputDirty = false,
    calls = {},
    agents = {},
    count = 0,
    version = nil,
    problem = nil,
    heardMs = nil,
    released = nil,
    settled = false,
    greeted = false,
    lastError = nil,
  }
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

-- Whether `lease` is its agent taking the desktop back soon after giving it up.
local function retaking(desk: Desk, lease: Lease, nowMs: number): boolean
  local released = desk.released
  return released ~= nil and released.session == lease.session and nowMs - released.atMs <= M.RETAKE_MS
end

local function leaseChanges(desk: Desk, after: Lease?, nowMs: number, changes: { Change })
  local before = desk.lease
  local same = before and after and before.session == after.session
  if before and not same then
    table.insert(changes, { kind = "released", lease = before })
    desk.released = { session = before.session, atMs = nowMs }
  end
  if after and (not same or (before :: Lease).pending ~= after.pending) then
    local kind = if after.pending then "asking" elseif retaking(desk, after, nowMs) then "retook" else "took"
    table.insert(changes, { kind = kind, lease = after } :: Change)
  end
end

local function agents(value: any): { Agent }
  local out: { Agent } = {}
  if type(value) ~= "table" then
    return out
  end
  for _, client in value do
    if type(client) == "table" and type(client.pid) == "number" then
      table.insert(out, {
        pid = client.pid,
        name = describe.agent(if type(client.agent) == "string" then client.agent else nil),
        project = if type(client.project) == "string" then client.project else nil,
        replaced = client.replaced == true,
      })
    end
  end
  return out
end

-- How many agents run a build an update replaced.
function M.outdated(list: { Agent }): number
  local count = 0
  for _, agent in list do
    if agent.replaced then
      count += 1
    end
  end
  return count
end

local function state(desk: Desk, event: any, nowMs: number, found: { Change })
  local changes: { Change } = {}
  local after = lease(event.lease)
  leaseChanges(desk, after, nowMs, changes)
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
  local servers = agents(event.clients)
  local outdated = M.outdated(servers)
  if outdated > M.outdated(desk.agents) then
    table.insert(changes, { kind = "outdated", count = outdated } :: Change)
  end
  desk.agents = servers
  for _, change in changes do
    if desk.settled or change.kind == "stuck" then
      table.insert(found, change)
    end
  end
  desk.settled = true
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
  -- Calls `watch` sends from before it started are history, not news.
  if entry.outcome == "error" and not MISTAKES[entry.error or ""] and event.backlog ~= true then
    table.insert(changes, { kind = "failed", call = entry })
  end
end

local function hello(desk: Desk, event: any)
  if event.format ~= M.FORMAT then
    desk.problem = "format"
    return
  end
  desk.problem = nil
  desk.settled = false
  desk.greeted = true
  desk.version = if type(event.version) == "string" then event.version else nil
end

-- Folds one line in: `event` is the decoded line, or nil when it wasn't JSON,
-- and `raw` its text. Returns what changed, oldest first.
function M.feed(desk: Desk, event: any, raw: string, nowMs: number, tr: Tr): { Change }
  desk.heardMs = nowMs
  local changes: { Change } = {}
  local kind = if type(event) == "table" then event.event else nil
  if kind ~= nil then
    desk.lastError = nil
  end
  if kind == "hello" then
    hello(desk, event)
  elseif kind == "missing" then
    desk.problem = "missing"
  elseif desk.problem == "format" then
    return changes
  elseif kind == "state" then
    state(desk, event, nowMs, changes)
  elseif kind == "action" then
    call(desk, event, tr, changes)
  elseif kind == nil and raw:find("usage: niri-computer-use", 1, true) then
    -- A build before `watch` prints its usage line for the unknown subcommand.
    desk.problem = "old"
  elseif kind == nil and raw ~= "" and not desk.greeted then
    desk.problem = raw
  elseif kind == nil and raw ~= "" then
    desk.lastError = raw
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
    agents = table.clone(desk.agents),
    version = desk.version,
    problem = desk.problem,
    lastError = desk.lastError,
  }
end

return M
