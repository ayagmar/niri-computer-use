--!nonstrict
-- Entry point: scripts/test-plugin.sh generates _generated/sources.luau, then runs this.

local t = require("./support/t")

-- Each spec returns nil. The Luau CLI's require returns a spec that doesn't
-- parse as its error message rather than raising it, which would otherwise
-- drop that spec's tests without a word.
local broken = 0
local specs = {
  lib_spec = require("./lib_spec"),
  service_spec = require("./service_spec"),
  surfaces_spec = require("./surfaces_spec"),
}
for name, loaded in specs do
  if loaded ~= nil then
    broken += 1
    print(`  ✗ {name} didn't load\n      {loaded}`)
  end
end

local failed = t.run() + broken
if failed > 0 then
  error(`{failed} test(s) or spec(s) failed`, 0)
end
