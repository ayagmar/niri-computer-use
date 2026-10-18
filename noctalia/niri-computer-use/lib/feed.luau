--!strict
-- Turns the calls the panel keeps into the rows its feed shows: consecutive key
-- presses by one agent fold into one row, so a run of shortcuts reads as one
-- step rather than filling the feed.

local M = {}

type Tr = (key: string, vars: { [string]: any }?) -> string

-- A call as the panel draws it. A folded row is its newest call's, keyed by its
-- first so it keeps its place while it grows, with its keys in `detail`.
export type Row = {
  key: string,
  text: string,
  glyph: string,
  outcome: string,
  error: string?,
  session: string,
  client: string,
  project: string?,
  ts: string,
  -- The folded calls' combinations, each call's apart: `Escape, comma · Escape, c`.
  detail: string?,
}

-- A key call that went through, with the combinations it pressed.
local function foldable(call: any): boolean
  return call.keys ~= nil and call.outcome == "ok"
end

local function row(call: any): Row
  return {
    key = call.key,
    text = call.text,
    glyph = call.glyph,
    outcome = call.outcome,
    error = call.error,
    session = call.session,
    client = call.client,
    project = call.project,
    ts = call.ts,
    detail = nil,
  }
end

-- The row for a run of two or more key calls, oldest first.
local function folded(run: { any }, tr: Tr): Row
  local out = row(run[#run])
  out.key = run[1].key
  local count = 0
  local each = {}
  for _, call in run do
    count += #call.keys
    table.insert(each, table.concat(call.keys, ", "))
  end
  out.text = tr("action.keys", { n = count })
  out.detail = table.concat(each, " · ")
  return out
end

function M.rows(calls: { any }, tr: Tr): { Row }
  local rows: { Row } = {}
  local run: { any } = {}
  local function flush()
    if #run == 1 then
      table.insert(rows, row(run[1]))
    elseif #run > 1 then
      table.insert(rows, folded(run, tr))
    end
    run = {}
  end
  for _, call in calls do
    if not foldable(call) or (#run > 0 and run[1].session ~= call.session) then
      flush()
    end
    if foldable(call) then
      table.insert(run, call)
    else
      table.insert(rows, row(call))
    end
  end
  flush()
  return rows
end

return M
