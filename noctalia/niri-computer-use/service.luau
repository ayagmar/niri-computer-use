--!nonstrict
-- The plugin's only reader: runs `niri-computer-use watch`, folds its lines
-- into the desk and publishes it as the `desk` state the bar, the panel and
-- the glow draw. It also sends the notifications and runs Stop and Resume.

local desk = require("./lib/desk.luau")
local command = require("./lib/command.luau")
local actions = require("./lib/actions.luau")

local STATE = "desk"
-- How long a quiet or failed watch waits before it is started again.
local RESTART_MS = 15000

-- A gap this long between updates means the machine slept.
local SLEPT_MS = 5000

local model = desk.new()
local generation = 0
local startedMs = -RESTART_MS
local tickedMs = nil

local function setting(key: string, fallback)
  local value = noctalia.getConfig(key)
  if value == nil then
    return fallback
  end
  return value
end

local function tr(key: string, vars)
  return noctalia.tr(key, vars)
end

local function publish()
  noctalia.state.set(STATE, desk.view(model, noctalia.nowMs()))
end

-- Where the agent works, as the start of a notification's body.
local function where(lease): string
  return if lease.project then tr("notify.where", { project = lease.project }) else ""
end

local function notifyChange(change)
  local kind = change.kind
  if kind == "took" and setting("notify_take", true) then
    noctalia.notify(tr("notify.took", { client = change.lease.client }), where(change.lease) .. tr("notify.took_body"))
  elseif kind == "released" and setting("notify_release", false) then
    noctalia.notify(tr("notify.released", { client = change.lease.client }), where(change.lease))
  elseif kind == "stopped" and setting("notify_stop", true) then
    noctalia.notify(tr("notify.stopped"), tr("notify.stopped_body"))
  elseif kind == "stuck" then
    noctalia.notifyError(tr("notify.stuck"), tr("notify.stuck_body"))
  elseif kind == "outdated" then
    noctalia.notify(tr("notify.outdated", { n = change.count }), tr("notify.outdated_body"))
  elseif kind == "failed" and setting("notify_errors", false) then
    local call = change.call
    noctalia.notifyError(tr("notify.failed", { client = call.client, error = call.error }), call.text)
  end
end

local function onLine(line: string)
  local event = noctalia.json.decode(line)
  for _, change in desk.feed(model, event, line, noctalia.nowMs(), tr) do
    notifyChange(change)
  end
  publish()
end

-- Starts a watch. Lines from an earlier one, should it still run, are dropped.
-- There is no onConfigChanged(): Noctalia restarts the service when a setting
-- changes, which ends its watch, where runStream gives no way to end one.
local function start()
  generation += 1
  local mine = generation
  startedMs = noctalia.nowMs()
  -- Keep showing why the last watch failed until the new one says hello.
  local problem = model.problem
  model = desk.new()
  model.problem = problem
  local ok = noctalia.runStream(command.run("watch", setting("binary", "")), function(line)
    if mine == generation then
      onLine(line)
    end
  end)
  if not ok then
    model.problem = tr("error.stream")
  end
  publish()
end

-- A watch that went quiet ended; start another, no more often than RESTART_MS.
local function supervise()
  local now = noctalia.nowMs()
  -- After a sleep the watch is still there; give it time to speak again.
  if tickedMs and now - tickedMs > SLEPT_MS and model.heardMs then
    model.heardMs = now
  end
  tickedMs = now
  local status = desk.status(model, now)
  local quiet = status == "offline" or status == "error" or status == "missing"
  if (quiet or model.heardMs == nil) and now - startedMs >= RESTART_MS then
    start()
  else
    publish()
  end
end

function stop()
  actions.run("stop")
end

function resume()
  actions.run("resume")
end

function onIpc(event: string, _payload)
  if event == "stop" then
    stop()
  elseif event == "resume" then
    resume()
  elseif event == "toggle" then
    if model.stopped then
      resume()
    else
      stop()
    end
  end
end

function update()
  supervise()
end

noctalia.setUpdateInterval(1000)
start()
