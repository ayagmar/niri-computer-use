--!nonstrict
local t = require("./support/t")
local env = require("./support/env")

local function started()
  local host = env.host()
  local service = env.start(host, "service.luau")
  return host, service
end

local function driving(host, session)
  host.event({ event = "hello", format = 1, version = "0.1.3" })
  host.event({ event = "state", stopped = false, input_dirty = false })
  host.event({
    event = "state",
    lease = { session = session or "claude-code/4711", since = "2026-10-17T15:05:55.000Z", pending = false, project = "shop" },
    stopped = false,
    input_dirty = false,
  })
end

t.group("service", function()
  t.test("runs watch and publishes who drives, with a notification naming agent and project", function()
    local host = started()
    t.eq(#host.streams, 1)
    t.ok(host.streams[1].command:find('exec "$b" watch', 1, true), host.streams[1].command)
    driving(host)
    t.eq(host.state.desk.status, "driving")
    t.eq(host.state.desk.lease.client, "Claude Code")
    t.eq(#host.notifications, 1)
    t.eq(host.notifications[1].title, "Claude Code took the desktop")
    t.eq(host.notifications[1].body, "In shop. Right-click the robot in the bar, or press your stop key, to stop it.")
  end)

  t.test("the take notification can be turned off", function()
    local host = env.host()
    host.config.notify_take = false
    env.start(host, "service.luau")
    driving(host)
    t.eq(#host.notifications, 0)
  end)

  t.test("a quiet watch is started again once, and lines from the old one are dropped", function()
    local host, service = started()
    driving(host)
    host.nowMs += 16_000
    for _ = 1, 3 do
      service.call("update")
      host.nowMs += 900
    end
    t.eq(#host.streams, 2)
    host.streams[1].onLine('{"event":"state","lease":null,"stopped":true,"input_dirty":false}')
    t.ok(not host.state.desk.stopped, "the old watch's line counted")
    driving(host)
    t.eq(host.state.desk.status, "driving")
  end)

  t.test("waking from sleep doesn't start a second watch", function()
    local host, service = started()
    driving(host)
    service.call("update")
    host.nowMs += 3_600_000
    service.call("update")
    t.eq(#host.streams, 1)
    t.eq(host.state.desk.status, "driving")
  end)

  t.test("a watch that never speaks is started again", function()
    local host, service = started()
    host.nowMs += 15_000
    service.call("update")
    t.eq(#host.streams, 2)
  end)

  t.test("watch's error stays shown while it is started again", function()
    local host, service = started()
    host.line("niri-computer-use: no niri socket, so there is no instance to watch")
    host.nowMs += 15_000
    service.call("update")
    t.eq(#host.streams, 2)
    t.eq(host.state.desk.status, "error")
  end)

  t.test("stop over IPC runs niri-computer-use stop and reports a failure with its output", function()
    local host, service = started()
    service.call("onIpc", "stop", "")
    t.eq(#host.commands, 1)
    t.ok(host.commands[1].command:find('exec "$b" stop', 1, true), host.commands[1].command)
    t.eq(host.commands[1].timeout, 5000)
    host.commands[1].callback({ exitCode = 1, stdout = "niri-computer-use: set the stop flag: denied", timedOut = false })
    t.eq(host.errors[1].title, "Couldn't stop agents")
    t.eq(host.errors[1].body, "niri-computer-use: set the stop flag: denied")
  end)

  t.test("toggle resumes when stopped, and a missing binary says how to fix it", function()
    local host, service = started()
    host.event({ event = "state", stopped = true, input_dirty = false })
    service.call("onIpc", "toggle", "")
    t.ok(host.commands[1].command:find('exec "$b" resume', 1, true), host.commands[1].command)
    host.commands[1].callback({ exitCode = 127, stdout = '{"event":"missing"}', timedOut = false })
    t.eq(host.errors[1].body, "Install it, or set its path in this plugin's settings.")
  end)

  t.test("a configured binary is the one run", function()
    local host = env.host()
    host.config.binary = "/opt/ncu/niri-computer-use"
    env.start(host, "service.luau")
    t.ok(host.streams[1].command:find("for b in '/opt/ncu/niri-computer-use'; do", 1, true), host.streams[1].command)
  end)
end)

return nil
