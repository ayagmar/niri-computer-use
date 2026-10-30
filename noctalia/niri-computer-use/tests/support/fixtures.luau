--!nonstrict
-- The `live` records the server's own tests write (tests/fixtures/live.jsonl), as
-- `watch` forwards them, by case, and the lease they were written under.

local env = require("./env")
local json = require("./json")

local M = {}

M.HOLDER = "claude-code/4711"
M.SINCE = "2026-10-30T11:59:00.000Z"

-- The fixture records by `_case`, each a fresh table as a `live` event.
function M.live(): { [string]: any }
  local out = {}
  for line in string.gmatch(env.sources["tests/fixtures/live.jsonl"], "[^\n]+") do
    local record = assert(json.decode(line))
    local case = record._case
    record._case = nil
    record.event = "live"
    out[case] = record
  end
  return out
end

-- `record` with `changes` applied, leaving `record` as it was.
function M.with(record, changes)
  local out = table.clone(record)
  for k, v in changes do
    out[k] = v
  end
  return out
end

-- A `state` event with `session` holding the lease.
function M.state(session: string?, extra)
  local lease = if session == false then nil else { session = session or M.HOLDER, since = M.SINCE, pending = false }
  local event = { event = "state", lease = lease, stopped = false, input_dirty = false }
  for k, v in extra or {} do
    if k == "pending" or k == "since" then
      if lease then
        lease[k] = v
      end
    else
      event[k] = v
    end
  end
  return event
end

return M
