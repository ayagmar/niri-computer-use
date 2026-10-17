--!nonstrict
-- Stop and Resume, for every entry that offers them: runs the subcommand and
-- says in a notification when it failed. That one is an error, at critical
-- urgency: it answers the user's own click, and after a failed Stop an agent
-- may still drive.

local command = require("./command.luau")

local M = {}

-- How long `stop` and `resume` may take.
local DEADLINE_MS = 5000

function M.run(subcommand: string)
  local binary = noctalia.getConfig("binary") or ""
  noctalia.runAsync(command.run(subcommand, binary), function(result)
    if result.exitCode == 0 then
      return
    end
    local detail = if result.timedOut
      then noctalia.tr("error.timed_out")
      elseif result.exitCode == 127 then noctalia.tr("status.missing_hint")
      else result.stdout
    noctalia.notifyError(noctalia.tr("error." .. subcommand), detail)
  end, DEADLINE_MS)
end

return M
