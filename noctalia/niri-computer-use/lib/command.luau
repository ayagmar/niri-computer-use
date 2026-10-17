--!strict
-- The shell commands the plugin runs. Noctalia's environment often lacks
-- ~/.local/bin and ~/.cargo/bin on PATH, so without a `binary` setting the
-- command tries PATH and then both places the installers use.

local M = {}

local function quote(text: string): string
  return "'" .. (text:gsub("'", "'\\''")) .. "'"
end

local function candidates(binary: string?): string
  if binary and binary ~= "" then
    return quote(binary)
  end
  return 'niri-computer-use "$HOME/.local/bin/niri-computer-use" "$HOME/.cargo/bin/niri-computer-use"'
end

-- Runs `niri-computer-use <subcommand>` from the first candidate found, with
-- stderr on stdout so the plugin sees why it failed, or prints a `missing`
-- event and exits 127 when there is none.
function M.run(subcommand: string, binary: string?): string
  return `for b in {candidates(binary)}; do command -v "$b" >/dev/null 2>&1 && exec "$b" {subcommand} 2>&1; done; echo '\{"event":"missing"}'; exit 127`
end

return M
