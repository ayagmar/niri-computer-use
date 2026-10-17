--!strict
-- How each desk status looks, shared by the bar, the panel and the glow so
-- they always agree: a glyph, a palette role and the words for it.

local time = require("./time.luau")

local M = {}

type Tr = (key: string, vars: { [string]: any }?) -> string

local GLYPH = {
  idle = "robot",
  driving = "robot",
  asking = "lock-question",
  stopped = "player-stop",
  offline = "plug-connected-x",
  starting = "loader",
  missing = "alert-triangle",
  error = "alert-triangle",
}

local ROLE = {
  idle = "on_surface_variant",
  driving = "primary",
  asking = "tertiary",
  stopped = "error",
  offline = "on_surface_variant",
  starting = "on_surface_variant",
  missing = "error",
  error = "error",
}

local OUTCOME_ROLE = { ok = "primary", refused = "tertiary", error = "error" }

function M.glyph(status: string): string
  return GLYPH[status] or "robot"
end

function M.role(status: string): string
  return ROLE[status] or "on_surface"
end

function M.outcomeRole(outcome: string): string
  return OUTCOME_ROLE[outcome] or "on_surface"
end

local function who(lease: any): string
  return lease and lease.client or "?"
end

-- The panel's headline for the desk.
function M.title(view: any, tr: Tr): string
  return tr("status." .. view.status .. ".title", { who = who(view.lease) })
end

-- The line under the headline: where and for how long, or what to do.
function M.detail(view: any, nowMs: number, tr: Tr): string
  local lease = view.lease
  if (view.status == "driving" or view.status == "asking") and lease then
    local since = time.parse(lease.since)
    local vars = {
      project = lease.project or "?",
      span = if since then time.span(nowMs - since, tr) else "?",
    }
    return tr(if lease.project then "status.driving.where" else "status.driving.when", vars)
  elseif view.status == "stopped" and view.inputDirty then
    return tr("status.stopped.stuck")
  elseif view.status == "error" then
    if view.problem == "format" or view.problem == "old" then
      return tr("status.error." .. view.problem)
    end
    return tostring(view.problem)
  end
  return tr("status." .. view.status .. ".detail")
end

return M
