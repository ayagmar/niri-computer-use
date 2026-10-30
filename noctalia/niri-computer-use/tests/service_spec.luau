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

  t.test("a failed action, once turned on, is a plain notification; stuck input is an error", function()
    local host = env.host()
    host.config.notify_errors = true
    env.start(host, "service.luau")
    driving(host)
    host.event({ event = "action", ts = "2026-10-17T15:06:00.000Z", session = "claude-code/4711", tool = "click", args = {}, error = "app_denied" })
    t.eq(#host.errors, 0, "a failed action is sent at critical urgency")
    t.eq(host.notifications[#host.notifications].title, "Claude Code: app_denied")
    host.event({ event = "state", stopped = true, input_dirty = true })
    t.eq(#host.errors, 1)
    t.eq(host.errors[1].title, "Input may be stuck")
  end)

  t.test("an agent that lost the engine gets one plain notification saying to restart it", function()
    local host = started()
    driving(host)
    local before = #host.notifications
    for _, error in { "engine_lost", "engine_unavailable", "engine_unavailable" } do
      host.event({ event = "action", ts = "2026-10-17T15:06:00.000Z", session = "claude-code/4711", tool = "status", args = {}, error = error })
    end
    t.eq(#host.notifications, before + 1)
    t.eq(#host.errors, 0, "sent at critical urgency")
    local told = host.notifications[#host.notifications]
    t.eq(told.title, "Restart Claude Code")
    t.ok(told.body:find("fail with engine_lost", 1, true), told.body)
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

  t.test("a settings change leaves one watch running, on the new binary", function()
    local host, service = started()
    env.changeSetting(host, service, "service.luau", "binary", "/opt/ncu")
    local running = {}
    for _, stream in host.streams do
      if not stream.ended then
        table.insert(running, stream)
      end
    end
    t.eq(#running, 1)
    t.ok(running[1].command:find("for b in '/opt/ncu';", 1, true), running[1].command)
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

  t.test("a watch of a format the plugin can't read isn't replaced while it still speaks", function()
    local host, service = started()
    host.event({ event = "hello", format = 2, version = "9.0.0" })
    for second = 1, 45 do
      host.nowMs += 1_000
      if second % 5 == 0 then
        host.event({ event = "alive" })
      end
      service.call("update")
    end
    t.eq(#host.streams, 1)
    t.eq(host.state.desk.status, "error")
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
