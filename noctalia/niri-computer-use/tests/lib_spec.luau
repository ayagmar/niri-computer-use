--!nonstrict
local t = require("./support/t")
local env = require("./support/env")

local desk, host = env.module("lib/desk.luau")
local describe = env.module("lib/describe.luau")
local time = env.module("lib/time.luau")
local command = env.module("lib/command.luau")
local tr = host.globals.noctalia.tr

local NOW = 1_790_000_000_000

local function lease(session, extra)
  local value = { session = session, since = "2026-10-17T15:05:55.000Z", pending = false, project = "shop" }
  for k, v in extra or {} do
    value[k] = v
  end
  return value
end

local function stateLine(leaseValue, stopped, dirty)
  return { event = "state", lease = leaseValue, stopped = stopped or false, input_dirty = dirty or false }
end

local function kinds(changes)
  local out = {}
  for _, change in changes do
    table.insert(out, change.kind)
  end
  return table.concat(out, ",")
end

t.group("desk", function()
  t.test("a lease that changes hands is a release, then a take, naming agent and project", function()
    local d = desk.new()
    desk.feed(d, { event = "hello", format = 1, version = "0.1.3" }, "", NOW, tr)
    desk.feed(d, stateLine(nil), "", NOW, tr)
    t.eq(kinds(desk.feed(d, stateLine(lease("claude-code/4711")), "", NOW, tr)), "took")
    local changes = desk.feed(d, stateLine(lease("codex-mcp-client/9", { project = "site" })), "", NOW, tr)
    t.eq(kinds(changes), "released,took")
    t.eq(changes[1].lease.client, "Claude Code")
    t.eq(changes[2].lease.client, "Codex")
    t.eq(changes[2].lease.project, "site")
    t.eq(kinds(desk.feed(d, stateLine(nil), "", NOW, tr)), "released")
    t.eq(desk.status(d, NOW), "idle")
  end)

  t.test("what watch shows at its start is how the desk was, not news, except stuck input", function()
    local d = desk.new()
    desk.feed(d, { event = "hello", format = 1 }, "", NOW, tr)
    t.eq(kinds(desk.feed(d, stateLine(lease("pi/7"), true, true), "", NOW, tr)), "stuck")
    local old = { event = "action", tool = "click", session = "pi/7", error = "app_denied", backlog = true }
    t.eq(kinds(desk.feed(d, old, "", NOW, tr)), "")
    t.eq(kinds(desk.feed(d, stateLine(nil, false, false), "", NOW, tr)), "released,resumed")
    -- A restarted watch says hello again: its first state is again how the desk was.
    desk.feed(d, { event = "hello", format = 1 }, "", NOW, tr)
    t.eq(kinds(desk.feed(d, stateLine(lease("pi/7")), "", NOW, tr)), "")
  end)

  t.test("an agent taking the desktop back within five minutes retook it, and later took it", function()
    local d = desk.new()
    desk.feed(d, stateLine(lease("claude-code/4711")), "", NOW, tr)
    t.eq(kinds(desk.feed(d, stateLine(nil), "", NOW, tr)), "released")
    t.eq(kinds(desk.feed(d, stateLine(lease("claude-code/4711")), "", NOW + 60_000, tr)), "retook")
    t.eq(kinds(desk.feed(d, stateLine(lease("pi/7")), "", NOW + 61_000, tr)), "released,took")
    desk.feed(d, stateLine(nil), "", NOW + 62_000, tr)
    t.eq(kinds(desk.feed(d, stateLine(lease("pi/7")), "", NOW + 62_000 + desk.RETAKE_MS + 1, tr)), "took")
  end)

  t.test("an agent's own mistake is never a failure worth telling the user about", function()
    local d = desk.new()
    local function call(error)
      return { event = "action", tool = "scroll", session = "claude-code/4711", error = error }
    end
    t.eq(kinds(desk.feed(d, call("invalid_arguments"), "", NOW, tr)), "")
    t.eq(kinds(desk.feed(d, call("ref_invalid"), "", NOW, tr)), "")
    t.eq(kinds(desk.feed(d, call("app_denied"), "", NOW, tr)), "failed")
  end)

  t.test("an agent asking first is asking, then took once allowed", function()
    local d = desk.new()
    desk.feed(d, stateLine(nil), "", NOW, tr)
    t.eq(kinds(desk.feed(d, stateLine(lease("pi/7", { pending = true })), "", NOW, tr)), "asking")
    t.eq(desk.status(d, NOW), "asking")
    t.eq(kinds(desk.feed(d, stateLine(lease("pi/7")), "", NOW, tr)), "took")
    t.eq(desk.status(d, NOW), "driving")
  end)

  t.test("the same state again changes nothing", function()
    local d = desk.new()
    desk.feed(d, stateLine(lease("pi/7")), "", NOW, tr)
    t.eq(kinds(desk.feed(d, stateLine(lease("pi/7")), "", NOW, tr)), "")
  end)

  t.test("stopped wins over a holder, and stuck input is reported once", function()
    local d = desk.new()
    desk.feed(d, stateLine(lease("pi/7")), "", NOW, tr)
    t.eq(kinds(desk.feed(d, stateLine(lease("pi/7"), true, true), "", NOW, tr)), "stopped,stuck")
    t.eq(desk.status(d, NOW), "stopped")
    t.eq(kinds(desk.feed(d, stateLine(nil, true, true), "", NOW, tr)), "released")
    t.eq(kinds(desk.feed(d, stateLine(nil, false, false), "", NOW, tr)), "resumed")
  end)

  t.test("an agent left on a replaced build is reported once, and again only when another joins it", function()
    local d = desk.new()
    local function clients(...)
      local line = stateLine(nil)
      line.clients = { ... }
      return line
    end
    local fresh = { pid = 5585, agent = "claude", project = "shop", replaced = false }
    local old = { pid = 4205, agent = "pi", project = "site", replaced = true }
    local older = { pid = 4300, agent = "codex", replaced = true }
    t.eq(kinds(desk.feed(d, clients(fresh), "", NOW, tr)), "")
    local changes = desk.feed(d, clients(fresh, old), "", NOW, tr)
    t.eq(kinds(changes), "outdated")
    t.eq(changes[1].count, 1)
    t.eq(kinds(desk.feed(d, clients(old, fresh), "", NOW, tr)), "")
    t.eq(kinds(desk.feed(d, clients(old, older), "", NOW, tr)), "outdated")
    t.eq(d.agents[1].name, "Pi")
    t.eq(d.agents[2].name, "Codex")
    t.eq(kinds(desk.feed(d, clients(fresh), "", NOW, tr)), "")
  end)

  t.test("a watch quiet for longer than its heartbeat allows is offline", function()
    local d = desk.new()
    t.eq(desk.status(d, NOW), "starting")
    desk.feed(d, { event = "alive" }, "", NOW, tr)
    t.eq(desk.status(d, NOW + desk.QUIET_MS), "idle")
    t.eq(desk.status(d, NOW + desk.QUIET_MS + 1), "offline")
  end)

  t.test("a line that isn't JSON is watch's error, shown until the next hello", function()
    local d = desk.new()
    local raw = "niri-computer-use: no niri socket, so there is no instance to watch"
    desk.feed(d, nil, raw, NOW, tr)
    t.eq(desk.status(d, NOW), "error")
    t.eq(desk.view(d, NOW).problem, raw)
    desk.feed(d, { event = "hello", format = 1 }, "", NOW, tr)
    t.eq(desk.status(d, NOW), "idle")
  end)

  t.test("a build without watch says to update rather than show its usage line", function()
    local d = desk.new()
    desk.feed(d, nil, "niri-computer-use: usage: niri-computer-use serve | status | stop", NOW, tr)
    t.eq(desk.status(d, NOW), "error")
    t.eq(d.problem, "old")
  end)

  t.test("a newer watch format is refused, and its later lines ignored", function()
    local d = desk.new()
    desk.feed(d, { event = "hello", format = 2 }, "", NOW, tr)
    desk.feed(d, stateLine(lease("pi/7")), "", NOW, tr)
    t.eq(desk.status(d, NOW), "error")
    t.eq(d.lease, nil)
  end)

  t.test("a missing binary says so", function()
    local d = desk.new()
    desk.feed(d, { event = "missing" }, "", NOW, tr)
    t.eq(desk.status(d, NOW), "missing")
  end)

  t.test("keeps the newest calls only", function()
    local d = desk.new()
    for i = 1, desk.KEEP + 5 do
      desk.feed(d, { event = "action", tool = "status", session = "pi/7", ts = `t{i}` }, "", NOW, tr)
    end
    t.eq(#d.calls, desk.KEEP)
    t.eq(d.calls[1].ts, "t6")
    t.eq(d.calls[desk.KEEP].ts, `t{desk.KEEP + 5}`)
  end)

  t.test("a failed call is worth a notification, a cancelled one isn't", function()
    local d = desk.new()
    local failed = desk.feed(d, { event = "action", tool = "click", session = "pi/7", error = "element_unmappable" }, "", NOW, tr)
    t.eq(kinds(failed), "failed")
    t.eq(failed[1].call.outcome, "error")
    t.eq(kinds(desk.feed(d, { event = "action", tool = "key", session = "pi/7", error = "cancelled" }, "", NOW, tr)), "")
    t.eq(kinds(desk.feed(d, { event = "action", tool = "key", session = "pi/7", accepted = false }, "", NOW, tr)), "")
    t.eq(d.calls[3].outcome, "refused")
  end)
end)

t.group("describe", function()
  t.test("typed and pasted text is told by its length only", function()
    t.eq(describe.say("type_text", { text_len = 12, expect = { window_id = 3 } }, tr), "Typed 12 characters")
    t.eq(describe.say("type_text", { text_len = 5, via = "paste", submit = true }, tr), "Pasted 5 characters and pressed Enter")
    t.eq(describe.say("set_element_text", { text_len = 9, role = "text" }, tr), "Filled in a field")
  end)

  t.test("names the server didn't know are never shown, as the log keeps only their length", function()
    t.eq(describe.say("launch", { preset = { category = "unknown_preset", bytes = 9 } }, tr), "Launched ?")
    t.eq(describe.say("launch", { preset = "firefox" }, tr), "Launched firefox")
    t.eq(describe.say("key", { keys = { category = "invalid_keys", count = 2 } }, tr), "Pressed ?")
    t.eq(describe.say("key", { keys = { "ctrl+s", "Return" } }, tr), "Pressed ctrl+s, Return")
    t.eq(describe.say("shell_panel", { panel = "control-center", open = false }, tr), "Closed control-center")
    t.eq(describe.say("activate_element", { role = "toggle_button" }, tr), "Pressed a toggle button")
    t.eq(describe.say("niri_action", { action = { MaximizeColumn = {} } }, tr), "Ran MaximizeColumn")
  end)

  t.test("every call reads as a sentence, a new tool as its name", function()
    t.eq(describe.say("click", { count = 2 }, tr), "Double-clicked")
    t.eq(describe.say("focus", { workspace_id = 3 }, tr), "Switched workspace")
    t.eq(describe.say("screenshot", { target = "region" }, tr), "Looked at part of the screen")
    t.eq(describe.say("screenshot", { target = "region", screenshot_ref = "shot-1" }, tr), "Zoomed into part of the screen")
    t.eq(describe.say("release_desktop", nil, tr), "Gave the desktop back")
    t.eq(describe.say("brand_new_tool", {}, tr), "brand new tool")
  end)

  t.test("agents go by their names", function()
    t.eq(describe.client("claude-code/4711"), "Claude Code")
    t.eq(describe.client("codex-mcp-client/12"), "Codex")
    t.eq(describe.client("my agent/3"), "my agent")
  end)
end)

t.group("time", function()
  t.test("reads watch's UTC timestamps", function()
    t.eq(time.parse("1970-01-01T00:00:00Z"), 0)
    t.eq(time.parse("2000-03-01T00:00:00.5Z"), 951868800500)
    t.eq(time.parse("2026-10-17T15:05:55.123Z"), 1792249555123)
    t.eq(time.parse("yesterday"), nil)
  end)

  t.test("says how long ago in short words", function()
    t.eq(time.ago(1000, tr), "now")
    t.eq(time.ago(42_000, tr), "42 s ago")
    t.eq(time.ago(3 * 60_000 + 5000, tr), "3 min ago")
    t.eq(time.span(2 * 3_600_000, tr), "2 h")
  end)
end)

t.group("command", function()
  t.test("a configured binary is quoted for the shell", function()
    local run = command.run("watch", "/opt/it's here/niri-computer-use")
    t.ok(run:find("for b in '/opt/it'\\''s here/niri-computer-use'; do", 1, true), run)
    t.ok(run:find('exec "$b" watch 2>&1', 1, true), run)
  end)

  t.test("without one it tries PATH and both install places, and fails with 127 when none is there", function()
    local run = command.run("stop", "")
    t.ok(run:find('niri-computer-use "$HOME/.local/bin/niri-computer-use" "$HOME/.cargo/bin/niri-computer-use"', 1, true), run)
    t.ok(run:find(`echo '\{"event":"missing"}'; exit 127`, 1, true), run)
  end)
end)

return nil
