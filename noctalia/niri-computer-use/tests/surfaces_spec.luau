--!nonstrict
local t = require("./support/t")
local env = require("./support/env")

local function desktop()
  local host = env.host()
  env.start(host, "service.luau")
  return host
end

local function drive(host, extra)
  host.event({ event = "hello", format = 1 })
  local lease = { session = "claude-code/4711", since = "2026-10-17T15:05:55.000Z", pending = false, project = "shop" }
  for k, v in extra or {} do
    lease[k] = v
  end
  host.event({ event = "state", lease = lease, stopped = false, input_dirty = false })
end

local function has(list, wanted)
  for _, v in list do
    if v == wanted then
      return true
    end
  end
  return false
end

t.group("bar", function()
  t.test("hidden while idle when asked, then shows the driving agent's name", function()
    local host = desktop()
    host.config.show_when_idle = false
    env.start(host, "bar.luau")
    host.event({ event = "hello", format = 1 })
    host.event({ event = "state", stopped = false, input_dirty = false })
    t.eq(host.bar.visible, false)
    drive(host)
    t.eq(host.bar.visible, true)
    t.ok(has(host.texts(host.bar.tree), "Claude Code"), "no agent name in the bar")
    t.ok(host.bar.tooltip:find("Claude Code is driving", 1, true), host.bar.tooltip)
  end)

  t.test("breathes while an agent drives and rests otherwise", function()
    local host = desktop()
    env.start(host, "bar.luau")
    drive(host)
    t.eq(host.updateInterval, 120)
    host.event({ event = "state", stopped = false, input_dirty = false })
    t.eq(host.updateInterval, 1000)
  end)

  t.test("right-click stops agents, or resumes them once stopped", function()
    local host = desktop()
    local bar = env.start(host, "bar.luau")
    drive(host)
    bar.call("onRightClick")
    t.ok(host.commands[1].command:find('exec "$b" stop', 1, true), host.commands[1].command)
    host.event({ event = "state", stopped = true, input_dirty = false })
    bar.call("onRightClick")
    t.ok(host.commands[2].command:find('exec "$b" resume', 1, true), host.commands[2].command)
    bar.call("onClick")
    t.eq(host.toggled[1], "ayagmar/niri-computer-use:panel")
  end)
end)

t.group("panel", function()
  local function button(host, label)
    return host.find(host.panelTree, function(n)
      return n.type == "button" and n.props.text == label
    end)
  end

  t.test("shows who drives where, each call in words, and the agent only where it changes", function()
    local host = desktop()
    local panel = env.start(host, "panel.luau")
    panel.call("onOpen", {})
    drive(host)
    host.event({ event = "action", ts = "2026-10-17T15:06:00.000Z", session = "claude-code/4711", project = "shop", tool = "type_text", args = { text_len = 12 }, accepted = true })
    local texts = host.texts(host.panelTree)
    t.ok(has(texts, "Claude Code is driving"), table.concat(texts, " | "))
    t.ok(has(texts, "Typed 12 characters"), table.concat(texts, " | "))
    t.ok(has(texts, "Claude Code in shop"), table.concat(texts, " | "))
    host.event({ event = "action", ts = "2026-10-17T15:06:01.000Z", session = "claude-code/4711", project = "shop", tool = "key", args = { keys = { "Return" } }, accepted = true })
    host.event({ event = "action", ts = "2026-10-17T15:06:02.000Z", session = "pi/9", tool = "click", args = {}, error = "element_unmappable" })
    texts = host.texts(host.panelTree)
    local named = 0
    for _, text in texts do
      if text == "Claude Code in shop" then
        named += 1
      end
    end
    t.eq(named, 1, "the agent under consecutive rows of one session")
    t.ok(has(texts, "Pi · element_unmappable"), table.concat(texts, " | "))
    t.ok(button(host, "Stop").props.enabled, "Stop is disabled while driving")
  end)

  t.test("lists every connected agent, marking the driver and one left on an old build", function()
    local host = desktop()
    local panel = env.start(host, "panel.luau")
    panel.call("onOpen", {})
    host.event({ event = "hello", format = 1 })
    host.event({
      event = "state",
      lease = { session = "claude-code/4711", since = "2026-10-17T15:05:55.000Z", pending = false, project = "shop" },
      stopped = false,
      input_dirty = false,
      clients = {
        { pid = 4205, agent = "pi", project = "site", replaced = true },
        { pid = 4711, agent = "claude", project = "shop", replaced = false },
      },
    })
    local texts = host.texts(host.panelTree)
    local function after(wanted)
      for i, text in texts do
        if text == wanted then
          return texts[i + 1]
        end
      end
      return nil
    end
    t.eq(after("Pi in site"), "old build, restart it")
    t.eq(after("Claude Code in shop"), "driving")
  end)

  t.test("opens on the newest call, and offers Latest only while the user reads older ones", function()
    local host = desktop()
    local panel = env.start(host, "panel.luau")
    local function feed()
      return host.find(host.panelTree, function(n)
        return n.type == "scroll"
      end)
    end
    panel.call("onOpen", {})
    drive(host)
    host.event({ event = "action", ts = "2026-10-17T15:06:00.000Z", session = "claude-code/4711", tool = "status", args = {} })
    local first = feed().props.scrollToBottomRev
    t.ok(button(host, "Latest") == nil, "Latest shows at the newest call")
    panel.call("onFeedScroll", "100", "400")
    local latest = button(host, "Latest")
    t.ok(latest ~= nil, "no Latest while reading older calls")
    latest.props.onClick()
    t.ok(feed().props.scrollToBottomRev ~= first, "Latest didn't jump to the newest call")
    t.ok(button(host, "Latest") == nil, "Latest stays after jumping")
    panel.call("onFeedScroll", "100", "400")
    panel.call("onClose")
    panel.call("onOpen", {})
    t.ok(button(host, "Latest") == nil, "a reopened panel doesn't start on older calls")
  end)

  t.test("its settings button opens the plugin's settings", function()
    local host = desktop()
    local panel = env.start(host, "panel.luau")
    panel.call("onOpen", {})
    drive(host)
    local settings = host.find(host.panelTree, function(n)
      return n.type == "button" and n.props.glyph == "settings"
    end)
    settings.props.onClick()
    t.eq(host.settingsOpened, 1)
  end)

  t.test("Resume is offered once stopped, and held back while input may be stuck", function()
    local host = desktop()
    local panel = env.start(host, "panel.luau")
    panel.call("onOpen", {})
    host.event({ event = "hello", format = 1 })
    host.event({ event = "state", stopped = true, input_dirty = true })
    local resume = button(host, "Resume")
    t.eq(resume.props.enabled, false)
    t.ok(has(host.texts(host.panelTree), "Input may be stuck. Run niri-computer-use recover in a terminal, then resume."), "no recover hint")
    host.event({ event = "state", stopped = true, input_dirty = false })
    resume = button(host, "Resume")
    t.eq(resume.props.enabled, true)
    resume.props.onClick()
    t.ok(host.commands[1].command:find('exec "$b" resume', 1, true), host.commands[1].command)
  end)
end)

t.group("glow", function()
  local function border(host)
    return host.desktopTree.props.border
  end

  t.test("is invisible while idle and breathes in the accent while an agent drives", function()
    local host = desktop()
    local glow = env.start(host, "glow.luau")
    host.event({ event = "hello", format = 1 })
    host.event({ event = "state", stopped = false, input_dirty = false })
    t.eq(border(host), "#00000000")
    t.eq(host.frameTicks, false)
    drive(host)
    t.eq(host.frameTicks, true)
    local first = border(host)
    t.ok(first:sub(1, 8) == "primary/", first)
    glow.call("onFrameTick", 1200)
    t.ok(border(host) ~= first, "the glow didn't breathe")
  end)

  t.test("holds the tertiary colour while an agent asks, and covers the focused output in logical pixels", function()
    local host = desktop()
    env.start(host, "glow.luau")
    drive(host, { pending = true })
    t.eq(border(host), "tertiary/0.80")
    t.eq(host.desktopTree.props.width, 2048)
    t.eq(host.desktopTree.props.height, 1152)
  end)
end)

return nil
