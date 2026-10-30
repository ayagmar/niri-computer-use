--!nonstrict
-- The plugin's only reader: runs `niri-computer-use watch`, folds its lines
-- into the desk and publishes it as the `desk` state the bar, the panel and
-- the glow draw, and the action in progress as the `live` state the robot
-- overlay draws. It also sends the notifications, shows and hides the overlay's
-- layer, and runs Stop and Resume.

local desk = require("./lib/desk.luau")
local command = require("./lib/command.luau")
local actions = require("./lib/actions.luau")
local live = require("./lib/live.luau")

local STATE = "desk"
local LIVE = "live"
local LAYER = "robot"
-- How long the overlay's layer stays after the lease is gone, for the goodbye.
local HIDE_MS = 3000
-- How long a quiet or failed watch waits before it is started again.
local RESTART_MS = 15000

-- A gap this long between updates means the machine slept.
local SLEPT_MS = 5000

local model = desk.new()
local overlay = live.new()
-- Whether this service showed the layer, and when it hides it.
local shown = false
local hideAtMs = nil
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

local function publishLive()
  noctalia.state.set(LIVE, live.view(overlay))
end

-- Shows the layer while an agent holds or asks for the desktop and the overlay is on,
-- and hides it HIDE_MS after, so no surface or runtime exists while no agent drives.
local function showOverlay(nowMs: number)
  if setting("overlay", true) and model.lease ~= nil then
    hideAtMs = nil
    if not shown then
      shown = true
      -- The last lease may have left the layer on an output that is gone since.
      noctalia.layer.configure(LAYER, { output = "focused" })
      noctalia.layer.show(LAYER)
    end
  elseif shown then
    hideAtMs = hideAtMs or nowMs + HIDE_MS
    if nowMs >= hideAtMs then
      shown = false
      hideAtMs = nil
      noctalia.layer.hide(LAYER)
    end
  end
end

-- Where the agent works, as the start of a notification's body.
local function where(lease): string
  return if lease.project then tr("notify.where", { project = lease.project }) else ""
end

-- Only stuck input, which needs the user to run `recover`, is an error:
-- notifyError sends at critical urgency, too loud for anything an agent retries.
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
  elseif kind == "restart" then
    local call = change.call
    noctalia.notify(tr("notify.restart", { client = call.client }), tr("notify.restart_body", { error = call.error }))
  elseif kind == "failed" and setting("notify_errors", false) then
    local call = change.call
    noctalia.notify(tr("notify.failed", { client = call.client, error = call.error }), call.text)
  end
end

local function onLine(line: string)
  local event = noctalia.json.decode(line)
  local nowMs = noctalia.nowMs()
  for _, change in desk.feed(model, event, line, nowMs, tr) do
    notifyChange(change)
  end
  if live.feed(overlay, event, nowMs) then
    publishLive()
  end
  showOverlay(nowMs)
  -- A `live` line changes nothing the bar, the panel or the glow draw.
  if type(event) ~= "table" or event.event ~= "live" then
    publish()
  end
end

-- Starts a watch. Lines from an earlier one, should it still run, are dropped.
-- There is no onConfigChanged(): Noctalia restarts the service when a setting
-- changes, which ends its watch, where runStream gives no way to end one.
local function start()
  generation += 1
  local mine = generation
  startedMs = noctalia.nowMs()
  -- Keep showing why the last watch failed until the new one says hello.
  local problem, lastError = model.problem, model.lastError
  model = desk.new()
  model.problem = problem
  model.lastError = lastError
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
  local silent = model.heardMs == nil or now - model.heardMs > desk.QUIET_MS
  -- A watch that printed its error has exited; one whose format this plugin
  -- can't read still runs, and is replaced only once it goes silent.
  local ended = model.problem ~= nil and model.problem ~= "format"
  if (silent or ended) and now - startedMs >= RESTART_MS then
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
  showOverlay(noctalia.nowMs())
end

noctalia.setUpdateInterval(1000)
-- A service that runs again, as after the overlay setting changed, starts hidden.
noctalia.layer.hide(LAYER)
publishLive()
start()
