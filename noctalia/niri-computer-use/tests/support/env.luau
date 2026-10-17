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

-- A plain module, such as "lib/desk.luau", with the fake host's globals.
function M.module(path: string)
  local host = M.host()
  return loader.entry(sources, host, path).require(path), host
end

return M
