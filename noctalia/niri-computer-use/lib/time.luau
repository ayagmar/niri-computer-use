--!strict
-- Reads the UTC timestamps `watch` sends, such as `2026-10-17T15:05:55.123Z`,
-- and says how long ago they were.

local M = {}

-- Days from 1970-01-01 to the civil date, valid for any Gregorian date.
local function days(year: number, month: number, day: number): number
  local y = if month <= 2 then year - 1 else year
  local era = (if y >= 0 then y else y - 399) // 400
  local yoe = y - era * 400
  local doy = (153 * (if month > 2 then month - 3 else month + 9) + 2) // 5 + day - 1
  local doe = yoe * 365 + yoe // 4 - yoe // 100 + doy
  return era * 146097 + doe - 719468
end

-- Milliseconds since the epoch, or nil for text that isn't such a timestamp.
function M.parse(text: string?): number?
  if type(text) ~= "string" then
    return nil
  end
  local y, mo, d, h, mi, s, frac = text:match("^(%d+)-(%d+)-(%d+)T(%d+):(%d+):(%d+)%.?(%d*)Z$")
  if not y then
    return nil
  end
  local ms = if frac ~= "" then tonumber((frac .. "00"):sub(1, 3)) :: number else 0
  local seconds = days(tonumber(y) :: number, tonumber(mo) :: number, tonumber(d) :: number) * 86400
    + (tonumber(h) :: number) * 3600
    + (tonumber(mi) :: number) * 60
    + (tonumber(s) :: number)
  return seconds * 1000 + ms
end

type Tr = (string, { [string]: any }?) -> string

-- A short duration such as `12 s`, `3 min` or `2 h`.
function M.span(ms: number, tr: Tr): string
  local seconds = math.max(0, ms // 1000)
  if seconds < 60 then
    return tr("time.seconds", { n = seconds })
  elseif seconds < 3600 then
    return tr("time.minutes", { n = seconds // 60 })
  end
  return tr("time.hours", { n = seconds // 3600 })
end

-- How long ago, such as `now` or `3 min ago`.
function M.ago(ms: number, tr: Tr): string
  if ms < 5000 then
    return tr("time.now")
  end
  return tr("time.ago", { span = M.span(ms, tr) })
end

return M
