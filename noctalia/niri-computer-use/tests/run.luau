--!nonstrict
-- Entry point: scripts/test-plugin.sh generates _generated/sources.luau, then runs this.

local t = require("./support/t")

require("./lib_spec")
require("./service_spec")
require("./surfaces_spec")

local failed = t.run()
if failed > 0 then
  error(`{failed} test(s) failed`, 0)
end
