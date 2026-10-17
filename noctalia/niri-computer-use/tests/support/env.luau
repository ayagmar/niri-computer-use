--!nonstrict
-- Builds a fake host and loads plugin entries into it, the way Noctalia does.

local sources = require("../_generated/sources")
local hostModule = require("./host")
local loader = require("./loader")

local M = {}

M.sources = sources

function M.host()
  return hostModule.new(sources)
end

-- Loads `entry` (such as "service.luau") against `host` and runs its top level.
function M.start(host, entry: string)
  return loader.entry(sources, host, entry).start()
end

-- Changes setting `key` the way Noctalia does: a service with onConfigChanged()
-- keeps running and hears of it; one without is restarted, which ends its
-- streams. Returns the runtime that runs after the change.
function M.changeSetting(host, runtime, entry: string, key: string, value)
  host.config[key] = value
  if rawget(runtime.env, "onConfigChanged") then
    runtime.call("onConfigChanged")
    return runtime
  end
  for _, stream in host.streams do
    stream.ended = true
  end
  return M.start(host, entry)
end

-- A plain module, such as "lib/desk.luau", with the fake host's globals.
function M.module(path: string)
  local host = M.host()
  return loader.entry(sources, host, path).require(path), host
end

return M
