--!nonstrict
local t = require("./support/t")
local env = require("./support/env")

local desk, host = env.module("lib/desk.luau")
local describe = env.module("lib/describe.luau")
local time = env.module("lib/time.luau")
local command = env.module("lib/command.luau")
local feed = env.module("lib/feed.luau")
local agentGroups = env.module("lib/agents.luau")
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

  t.test("watch's error before hello keeps its first lines, and not the last watch's", function()
    local d = desk.new()
    -- The service carries the last watch's error over until the new one says hello.
    d.problem = "niri-computer-use: no niri socket, so there is no instance to watch"
    local said = {
      "thread 'main' panicked at src/watch.rs:12:5:",
      "called `Result::unwrap()` on an `Err` value",
      "note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace",
      "more",
    }
    for _, line in said do
      desk.feed(d, nil, line, NOW, tr)
    end
    t.eq(desk.status(d, NOW), "error")
    t.eq(desk.view(d, NOW).problem, table.concat(said, "\n", 1, desk.ERROR_LINES))
  end)

  t.test("a line that isn't JSON after hello is a cut line, not watch's error", function()
    local d = desk.new()
    desk.feed(d, { event = "hello", format = 1 }, "", NOW, tr)
    desk.feed(d, nil, 'xxxx","tool":"type_text"}', NOW, tr)
    t.eq(d.problem, nil)
    t.eq(desk.status(d, NOW), "idle")
    desk.feed(d, { event = "alive" }, "", NOW, tr)
    t.eq(desk.view(d, NOW).lastError, nil, "a cut line's tail outlived the next line")
  end)

  t.test("watch's last words after hello keep their first lines, so a panic shows above its note", function()
    local d = desk.new()
    desk.feed(d, { event = "hello", format = 1 }, "", NOW, tr)
    local said = {
      "thread 'main' panicked at src/watch.rs:12:5:",
      "called `Result::unwrap()` on an `Err` value",
      "note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace",
    }
    for _, line in said do
      desk.feed(d, nil, line, NOW, tr)
    end
    t.eq(desk.view(d, NOW).lastError, table.concat(said, "\n"))
    for i = 1, 10 do
      desk.feed(d, nil, `more {i}`, NOW, tr)
    end
    local kept = string.split(desk.view(d, NOW).lastError, "\n")
    t.eq(#kept, desk.ERROR_LINES)
    t.eq(kept[1], said[1])
    desk.feed(d, { event = "alive" }, "", NOW, tr)
    t.eq(desk.view(d, NOW).lastError, nil, "the error outlived the next JSON line")
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

  t.test("an agent an update replaced is told to restart once per session, not per call", function()
    local d = desk.new()
    local line = stateLine(nil)
    line.clients = {
      { pid = 7, agent = "pi", replaced = true },
      { pid = 8, agent = "pi", replaced = true },
      { pid = 4711, agent = "claude", replaced = false },
    }
    desk.feed(d, line, "", NOW, tr)
    local function call(session, error, backlog)
      return { event = "action", tool = "status", session = session, args = {}, error = error, backlog = backlog }
    end
    t.eq(kinds(desk.feed(d, call("pi/8", "engine_lost", true), "", NOW, tr)), "")
    local first = desk.feed(d, call("pi/7", "engine_lost"), "", NOW, tr)
    t.eq(kinds(first), "restart")
    t.eq(first[1].call.client, "Pi")
    t.eq(kinds(desk.feed(d, call("pi/7", "engine_unavailable"), "", NOW, tr)), "")
    t.eq(kinds(desk.feed(d, call("pi/7", "engine_lost"), "", NOW, tr)), "")
    t.eq(kinds(desk.feed(d, call("pi/8", "engine_unavailable"), "", NOW, tr)), "restart")
    -- Another failure of a session already told still notifies as a failure.
    t.eq(kinds(desk.feed(d, call("pi/7", "app_denied"), "", NOW, tr)), "failed")
    t.eq(d.calls[#d.calls - 1].outcome, "error")
    -- An engine error of a server of the installed build, or of one not listed, has
    -- another cause, such as a crash or another agent holding the desktop: restarting
    -- the agent wouldn't help.
    t.eq(kinds(desk.feed(d, call("claude-code/4711", "engine_unavailable"), "", NOW, tr)), "failed")
    t.eq(kinds(desk.feed(d, call("claude-code/4711", "engine_lost"), "", NOW, tr)), "failed")
    t.eq(kinds(desk.feed(d, call("codex/9", "engine_lost"), "", NOW, tr)), "failed")
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

t.group("feed", function()
  -- The feed's rows after these calls, as watch sends them: `{ session, tool, args, error? }`.
  local function rows(calls)
    local d = desk.new()
    desk.feed(d, { event = "hello", format = 1 }, "", NOW, tr)
    for i, c in calls do
      local ts = c.ts or `2026-10-17T15:06:{string.format("%02d", i)}.000Z`
      desk.feed(d, { event = "action", ts = ts, session = c[1], tool = c[2], args = c[3], error = c[4] }, "", NOW, tr)
    end
    return feed.rows(d.calls, tr)
  end

  local function texts(list)
    local out = {}
    for _, r in list do
      table.insert(out, r.text)
    end
    return table.concat(out, " | ")
  end

  t.test("one agent's consecutive key presses fold into one row with each call's keys", function()
    local list = rows({
      { "pi/7", "click", {} },
      { "pi/7", "key", { keys = { "Escape", "comma" } } },
      { "pi/7", "key", { keys = { "Escape", "question" } } },
      { "pi/7", "key", { keys = { "Return" } } },
    })
    t.eq(texts(list), "Clicked | Pressed 5 keys")
    t.eq(list[2].detail, "Escape, comma · Escape, question · Return")
    t.eq(list[2].ts, "2026-10-17T15:06:04.000Z", "a folded row's time is its newest call's")
    -- The row keeps its first call's key, so it stays put as presses join it.
    local grown = rows({
      { "pi/7", "click", {} },
      { "pi/7", "key", { keys = { "Escape", "comma" } } },
      { "pi/7", "key", { keys = { "Escape", "question" } } },
    })
    t.eq(grown[2].key, list[2].key)
  end)

  t.test("a lone key press keeps its sentence, and another agent, a failure or another call ends a run", function()
    local list = rows({
      { "pi/7", "key", { keys = { "Escape" } } },
      { "claude-code/9", "key", { keys = { "a" } } },
      { "claude-code/9", "key", { keys = { "b" } }, "focus_mismatch" },
      { "claude-code/9", "key", { keys = { "c" } } },
      { "claude-code/9", "status", {} },
      { "claude-code/9", "key", { keys = { "d" } } },
    })
    t.eq(texts(list), "Pressed Escape | Pressed a | Pressed b | Pressed c | Checked readiness | Pressed d")
    t.eq(list[3].error, "focus_mismatch")
  end)

  t.test("key presses more than 30 seconds apart are separate steps, each with its own time", function()
    local list = rows({
      { "pi/7", "key", { keys = { "a" } }, ts = "2026-10-17T15:06:00.000Z" },
      { "pi/7", "key", { keys = { "b" } }, ts = "2026-10-17T15:06:30.000Z" },
      { "pi/7", "key", { keys = { "c" } }, ts = "2026-10-17T15:07:00.001Z" },
    })
    t.eq(texts(list), "Pressed 2 keys | Pressed c")
    t.eq(list[1].ts, "2026-10-17T15:06:30.000Z")
  end)

  t.test("key presses the log kept only a count of never fold, so no key is made up", function()
    local list = rows({
      { "pi/7", "key", { keys = { "a" } } },
      { "pi/7", "key", { keys = { category = "invalid_keys", count = 2 } } },
      { "pi/7", "key", { keys = { "b" } } },
    })
    t.eq(texts(list), "Pressed a | Pressed ? | Pressed b")
  end)
end)

t.group("agents", function()
  local function agent(pid, project, replaced)
    return { pid = pid, name = "Claude Code", project = project, replaced = replaced or false }
  end

  local function summary(groups)
    local out = {}
    for _, g in groups do
      table.insert(out, `{g.project}:{table.concat(g.pids, ",")}{if g.driving then "*" else ""}{if g.replaced then "!" else ""}`)
    end
    return table.concat(out, " ")
  end

  t.test("one agent's sessions in one project share a row, kept in watch's order", function()
    local groups = agentGroups.groups({ agent(1, "kipple"), agent(2, "orange"), agent(3, "kipple"), agent(4, "orange") }, nil)
    t.eq(summary(groups), "kipple:1,3 orange:2,4")
  end)

  t.test("the driver has a row of its own, first, and an old build never shares a row with a new one", function()
    local groups = agentGroups.groups({ agent(1, "orange"), agent(2, "orange", true), agent(3, "orange") }, 3)
    t.eq(summary(groups), "orange:3* orange:1 orange:2!")
  end)

  t.test("another agent in the same project has its own row", function()
    local pi = { pid = 2, name = "Pi", project = "orange", replaced = false }
    t.eq(#agentGroups.groups({ agent(1, "orange"), pi }, nil), 2)
  end)

  t.test("agents watch couldn't name never share a row, as they may be different agents", function()
    local d = desk.new()
    desk.feed(d, { event = "hello", format = 1 }, "", NOW, tr)
    local clients = { { pid = 1, project = "orange" }, { pid = 2, project = "orange" } }
    desk.feed(d, { event = "state", stopped = false, input_dirty = false, clients = clients }, "", NOW, tr)
    t.eq(#agentGroups.groups(d.agents, nil), 2)
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
    t.eq(describe.say("niri_action", { action = { category = "invalid_action", keys = 1, bytes = 14 } }, tr), "Ran ?")
  end)

  t.test("a Noctalia command reads as Noctalia names it, or as sent", function()
    local caffeine = { count = 1, bytes = { 15 }, command = "caffeine-toggle", description = "Toggle caffeine (idle inhibitor)" }
    t.eq(describe.say("noctalia", caffeine, tr), "Toggle caffeine (idle inhibitor)")
    t.eq(describe.say("noctalia", { count = 1, bytes = { 15 }, command = "caffeine-toggle" }, tr), "Ran Noctalia's caffeine-toggle")
    local plugin = {
      count = 4,
      bytes = { 6, 30, 3, 13 },
      command = "plugin",
      description = "Dispatch an event to a plugin entry",
      plugin = "ayagmar/obs-control",
    }
    t.eq(describe.say("noctalia", plugin, tr), "Sent an event to ayagmar/obs-control")
    t.eq(describe.say("noctalia", { count = 2, bytes = { 14, 6 } }, tr), "Sent a Noctalia command")
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
