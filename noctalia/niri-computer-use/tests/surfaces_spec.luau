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

  -- `count` agents, the driver listed last, and the sixth and seventh on an old build.
  local function crowd(host, count)
    local clients = {}
    for i = 1, count - 1 do
      table.insert(clients, { pid = 100 + i, agent = "pi", project = `p{i}`, replaced = i >= 6 })
    end
    table.insert(clients, { pid = 4711, agent = "claude", project = "shop", replaced = false })
    host.event({
      event = "state",
      lease = { session = "claude-code/4711", since = "2026-10-17T15:05:55.000Z", pending = false, project = "shop" },
      stopped = false,
      input_dirty = false,
      clients = clients,
    })
  end

  local function crowded()
    local host = desktop()
    local panel = env.start(host, "panel.luau")
    panel.call("onOpen", {})
    host.event({ event = "hello", format = 1 })
    crowd(host, 8)
    return host, panel
  end

  -- How many agent rows the panel draws outside a scroll, and whether it has one.
  local function agentRows(host)
    local rows, scrolls = 0, false
    local function walk(n, inScroll)
      if type(n) ~= "table" then
        return
      end
      local key = n.props and n.props.key
      if n.type == "scroll" and key == "agents" then
        scrolls = true
        inScroll = true
      elseif type(key) == "string" and key:match("^agent%-%d+$") and not inScroll then
        rows += 1
      end
      for _, child in n.children or {} do
        walk(child, inScroll)
      end
    end
    walk(host.panelTree, false)
    return rows, scrolls
  end

  t.test("with many agents, shows the driver and two more, and folds the rest so the feed keeps its room", function()
    local host = crowded()
    local texts = host.texts(host.panelTree)
    t.ok(has(texts, "Claude Code in shop") and has(texts, "Pi in p1") and has(texts, "Pi in p2"), table.concat(texts, " | "))
    t.ok(not has(texts, "Pi in p3"), "a folded agent shows")
    t.ok(has(texts, "2 on an old build"), "the folded old builds aren't counted")
    local rows, scrolls = agentRows(host)
    t.eq(rows, 3, "agent rows drawn while folded")
    t.ok(not scrolls, "a folded list scrolls")
    button(host, "+5 more").props.onClick()
    local list = host.find(host.panelTree, function(n)
      return n.type == "scroll" and n.props.key == "agents"
    end)
    t.ok(list and list.props.height ~= nil, "the unfolded list doesn't scroll within a fixed height")
    for _, name in { "Claude Code in shop", "Pi in p3", "Pi in p7" } do
      t.ok(has(host.texts(list), name), `{name} is missing once unfolded`)
    end
    t.ok(has(host.texts(list), "old build, restart it"), "an old build isn't marked once unfolded")
    button(host, "Show fewer").props.onClick()
    t.ok(not has(host.texts(host.panelTree), "Pi in p3"), "Show fewer didn't fold the list")
  end)

  t.test("an unfolded agent list is folded again when the panel reopens", function()
    local host, panel = crowded()
    button(host, "+5 more").props.onClick()
    panel.call("onClose")
    panel.call("onOpen", {})
    t.ok(button(host, "+5 more") ~= nil, "the panel reopened unfolded")
    t.eq(agentRows(host), 3)
  end)

  t.test("an unfolded agent list starts folded again once it shrank to four and grows back", function()
    local host = crowded()
    button(host, "+5 more").props.onClick()
    crowd(host, 2)
    t.eq(agentRows(host), 2)
    crowd(host, 8)
    t.ok(button(host, "+5 more") ~= nil, "the list came back unfolded")
    t.eq(agentRows(host), 3)
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

  t.test("watch's last words after hello show once it went silent, without a second watch", function()
    local host = env.host()
    local service = env.start(host, "service.luau")
    local panel = env.start(host, "panel.luau")
    panel.call("onOpen", {})
    drive(host)
    local function tick(seconds)
      for _ = 1, seconds do
        host.nowMs += 1_000
        service.call("update")
      end
    end
    tick(10)
    host.event({ event = "alive" })
    local said = "niri-computer-use: write to stdout: Input/output error (os error 5)"
    host.line(said)
    t.eq(host.state.desk.status, "driving")
    -- Past the restart delay, but not yet silent: the line didn't end the watch.
    tick(10)
    t.eq(#host.streams, 1)
    tick(6)
    t.eq(#host.streams, 2)
    t.eq(host.state.desk.status, "starting")
    t.ok(has(host.texts(host.panelTree), said), table.concat(host.texts(host.panelTree), " | "))
    host.event({ event = "hello", format = 1 })
    t.ok(not has(host.texts(host.panelTree), said), "the error outlived the next hello")
  end)

  t.test("Stop stays enabled while the plugin can't see the desk, but not without a binary", function()
    local host = env.host()
    local service = env.start(host, "service.luau")
    local panel = env.start(host, "panel.luau")
    panel.call("onOpen", {})
    drive(host)
    host.nowMs += 16_000
    service.call("update")
    t.eq(host.state.desk.status, "starting")
    t.ok(button(host, "Stop").props.enabled, "Stop is disabled while a quiet watch is started again")
    host.line("niri-computer-use: no niri socket, so there is no instance to watch")
    t.eq(host.state.desk.status, "error")
    t.ok(button(host, "Stop").props.enabled, "Stop is disabled after watch's error")
    host.event({ event = "missing" })
    t.eq(button(host, "Stop").props.enabled, false)
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
