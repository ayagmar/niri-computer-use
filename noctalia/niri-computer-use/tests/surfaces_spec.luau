--!nonstrict
local t = require("./support/t")
local env = require("./support/env")
local fx = require("./support/fixtures")

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

  t.test("its tooltip names the last call apart from its keys, and agents with their old builds in one line", function()
    local host = desktop()
    env.start(host, "bar.luau")
    host.event({ event = "hello", format = 1 })
    host.event({
      event = "state",
      stopped = false,
      input_dirty = false,
      clients = { { pid = 1, agent = "pi", replaced = true }, { pid = 2, agent = "claude", replaced = false } },
    })
    host.event({ event = "action", ts = "2026-10-17T15:06:00.000Z", session = "pi/1", tool = "key", args = { keys = { "Escape", "c" } }, accepted = true })
    local lines = string.split(host.bar.tooltip, "\n")
    t.ok(has(lines, "Connected agents: 2 · 1 on an old build"), host.bar.tooltip)
    t.ok(lines[3]:find("^Last: Pressed Escape, c · "), host.bar.tooltip)
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
  -- The button labelled `label` in the panel, or in `tree`, a part of it.
  local function button(host, label, tree)
    return host.find(tree or host.panelTree, function(n)
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

  t.test("folds an agent's run of key presses into one row that names the keys", function()
    local host = desktop()
    local panel = env.start(host, "panel.luau")
    panel.call("onOpen", {})
    drive(host)
    for i, keys in { { "Escape", "comma" }, { "Escape", "c" } } do
      host.event({ event = "action", ts = `2026-10-17T15:06:0{i}.000Z`, session = "claude-code/4711", project = "shop", tool = "key", args = { keys = keys }, accepted = true })
    end
    local texts = host.texts(host.panelTree)
    t.ok(has(texts, "Pressed 4 keys"), table.concat(texts, " | "))
    t.ok(has(texts, "Claude Code in shop · Escape, comma · Escape, c"), table.concat(texts, " | "))
    t.ok(not has(texts, "Pressed Escape, comma"), "a folded press still has its own row")
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
    t.eq(after("Pi in site"), "old build")
    t.eq(after("Claude Code in shop"), "driving")
    t.ok(has(texts, "1 on an old build, restart it"), "no summary of the old build")
  end)

  t.test("two sessions of one agent in one project share a row that counts them", function()
    local host = desktop()
    local panel = env.start(host, "panel.luau")
    panel.call("onOpen", {})
    host.event({ event = "hello", format = 1 })
    host.event({
      event = "state",
      stopped = false,
      input_dirty = false,
      clients = {
        { pid = 31, agent = "claude", project = "orange", replaced = false },
        { pid = 32, agent = "claude", project = "orange", replaced = false },
      },
    })
    local texts = host.texts(host.panelTree)
    local named = 0
    for _, text in texts do
      if text == "Claude Code in orange" then
        named += 1
      end
    end
    t.eq(named, 1, "rows for one agent in one project")
    t.ok(has(texts, "2 sessions"), table.concat(texts, " | "))
    local row = host.find(host.panelTree, function(n)
      return n.props and n.props.key == "agent-31"
    end)
    t.eq(row.props.tooltip, "PIDs 31, 32")
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

  local function fold(host)
    return host.find(host.panelTree, function(n)
      return n.props and n.props.key == "agents-fold"
    end)
  end

  t.test("with many agents, shows only the driver and folds the rest, naming old builds once", function()
    local host = crowded()
    local texts = host.texts(host.panelTree)
    t.ok(has(texts, "Claude Code in shop") and has(texts, "and 7 more"), table.concat(texts, " | "))
    t.ok(not has(texts, "Pi in p1"), "a folded agent shows")
    t.ok(has(texts, "2 on an old build, restart them"), "the old builds aren't summed up")
    t.ok(has(texts, "8"), "no count of connected agents")
    local rows, scrolls = agentRows(host)
    t.eq(rows, 1, "agent rows drawn while folded")
    t.ok(not scrolls, "a folded list scrolls")
    fold(host).props.onClick()
    -- Noctalia lets a scroll grow to its rows, so a column of fixed height holds it.
    local holder = host.find(host.panelTree, function(n)
      return n.type == "column" and n.props.height ~= nil and n.children[1] and n.children[1].props.key == "agents"
    end)
    t.ok(holder ~= nil and holder.children[1].type == "scroll", "the unfolded list doesn't scroll within a fixed height")
    local list = holder.children[1]
    for _, name in { "Claude Code in shop", "Pi in p3", "Pi in p7" } do
      t.ok(has(host.texts(list), name), `{name} is missing once unfolded`)
    end
    t.ok(has(host.texts(list), "old build"), "an old build isn't marked once unfolded")
    t.ok(has(host.texts(fold(host)), "Show fewer"), "no way to fold the list again")
    fold(host).props.onClick()
    t.ok(not has(host.texts(host.panelTree), "Pi in p3"), "Show fewer didn't fold the list")
  end)

  t.test("an unfolded agent list is folded again when the panel reopens", function()
    local host, panel = crowded()
    fold(host).props.onClick()
    panel.call("onClose")
    panel.call("onOpen", {})
    t.ok(has(host.texts(host.panelTree), "and 7 more"), "the panel reopened unfolded")
    t.eq(agentRows(host), 1)
  end)

  t.test("an unfolded agent list starts folded again once it shrank to three or none and grows back", function()
    local host = crowded()
    fold(host).props.onClick()
    crowd(host, 3)
    t.eq(agentRows(host), 3)
    t.eq(fold(host), nil, "a short list folds")
    crowd(host, 8)
    t.ok(has(host.texts(host.panelTree), "and 7 more"), "the list came back unfolded")
    t.eq(agentRows(host), 1)
    fold(host).props.onClick()
    host.event({ event = "state", stopped = false, input_dirty = false, clients = {} })
    t.eq(agentRows(host), 0)
    crowd(host, 8)
    t.ok(has(host.texts(host.panelTree), "and 7 more"), "the list came back unfolded after emptying")
    t.eq(agentRows(host), 1)
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
    t.eq(button(host, "Stop"), nil, "Stop shows without a binary to run it")
  end)

  t.test("Stop sits in the status card while an agent asks or drives, and not while idle", function()
    local host = desktop()
    local panel = env.start(host, "panel.luau")
    panel.call("onOpen", {})
    host.event({ event = "hello", format = 1 })
    host.event({ event = "state", stopped = false, input_dirty = false })
    t.eq(button(host, "Stop"), nil, "Stop shows while no agent holds the desktop")
    drive(host, { pending = true })
    local card = host.panelTree.children[1]
    t.ok(button(host, "Stop", card) ~= nil, "no Stop in the status card while an agent asks")
    drive(host)
    card = host.panelTree.children[1]
    t.ok(button(host, "Stop", card) ~= nil, "no Stop in the status card while an agent drives")
    t.eq(button(host, "Stop"), button(host, "Stop", card), "Stop isn't the card's alone")
  end)

  t.test("Stop stays for five minutes after an agent gives the desktop back, in case it takes it again", function()
    local host = desktop()
    local panel = env.start(host, "panel.luau")
    panel.call("onOpen", {})
    drive(host)
    host.event({ event = "state", stopped = false, input_dirty = false })
    t.eq(host.state.desk.status, "idle")
    t.ok(button(host, "Stop", host.panelTree.children[1]) ~= nil, "Stop left the card as the agent gave the desktop back")
    host.nowMs += 5 * 60 * 1000
    host.event({ event = "alive" })
    t.ok(button(host, "Stop") ~= nil, "Stop left before five minutes were up")
    host.nowMs += 1
    host.event({ event = "alive" })
    t.eq(button(host, "Stop"), nil, "Stop stays once the agent can no longer be taking the desktop back")
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

t.group("robot", function()
  local F = fx.live()
  local stage = env.module("lib/stage.luau")

  -- The service and the robot's layer on HDMI-A-1, with the fixtures' agent driving.
  local function overlay()
    local host = env.host()
    host.outputs = {
      { name = "HDMI-A-1", width = 2048, height = 1152, scale = 1, focused = true },
      { name = "DP-2", width = 1280, height = 720, scale = 1, focused = false },
    }
    host.focusedOutput = "HDMI-A-1"
    host.layer.output = "HDMI-A-1"
    host.nowMs = env.module("lib/time.luau").parse("2026-10-30T12:00:00.500Z")
    env.start(host, "service.luau")
    local robot = env.start(host, "robot.luau")
    host.event({ event = "hello", format = 1 })
    host.event(fx.state())
    return host, robot
  end

  local function images(tree)
    local out = {}
    local function walk(n)
      if n.type == "image" then
        table.insert(out, n)
      end
      for _, child in n.children or {} do
        walk(child)
      end
    end
    walk(tree)
    return out
  end

  local function lastConfigure(host)
    for i = #host.layerCalls, 1, -1 do
      if host.layerCalls[i].command == "configure" then
        return host.layerCalls[i].patch
      end
    end
    return nil
  end

  -- Where the drawn tree puts the robot and its caption, from its two spacers.
  local function corner(tree)
    local at = {}
    local function walk(n)
      if n.props and (n.props.key == "left" or n.props.key == "top") then
        at[n.props.key] = n.props
      end
      for _, child in n.children or {} do
        walk(child)
      end
    end
    walk(tree)
    return at.left.width, at.top.height
  end

  t.test("draws one frame of the robot beside the click, placed in the layer", function()
    local host, robot = overlay()
    local renders, calls = host.layer.renders, #host.layerCalls
    host.event(F.click_aim)
    -- The bug: drawing from the state watcher, where Noctalia gives no output name, moved
    -- the layer to the output it was on and drew nothing.
    t.eq(host.layer.renders, renders + 1, "drawn as the call arrives")
    for i = calls + 1, #host.layerCalls do
      local patch = host.layerCalls[i].patch
      t.ok(not (patch and patch.output and patch.output ~= "focused"), "kept on its output")
    end
    host.nowMs += 500
    robot.call("update")
    local frames = images(host.layer.tree)
    t.eq(#frames, #stage.POSES)
    local shown = {}
    for _, image in frames do
      t.ok(env.sources[image.props.path] ~= nil, `{image.props.path} is in the plugin`)
      if image.props.visible then
        table.insert(shown, image.props.path)
      end
    end
    t.eq(#shown, 1, "one frame shows")
    t.contains(host.texts(host.layer.tree), "click")
    local left, top = corner(host.layer.tree)
    -- Above-left of the point, with the caption to the robot's left.
    t.eq(left + (stage.INNER + stage.captionWidth("click")) + stage.SPRITE, 650 - stage.GAP)
    t.eq(top, math.floor(320.4 - stage.GAP - stage.SPRITE + 0.5))
  end)

  t.test("a target on another output moves the layer there instead of drawing here", function()
    local host = overlay()
    local other = fx.with(F.click_aim, {})
    other.aim = { output = "DP-2", area = { width = 1280, height = 720 }, points = { { x = 300, y = 300 } } }
    local renders = host.layer.renders
    host.event(other)
    local patch = lastConfigure(host)
    t.eq(patch.output, "DP-2")
    t.eq(host.layer.renders, renders, "nothing drawn on HDMI-A-1")
    local gone = fx.with(other, { call = 2 })
    gone.aim = { output = "HDMI-A-9", area = { width = 1280, height = 720 }, points = { { x = 300, y = 300 } } }
    local calls = #host.layerCalls
    host.event(gone)
    t.eq(#host.layerCalls, calls, "no move to an output Noctalia doesn't list")
  end)

  -- The bug: moving to another output while the agent looked, so the layer Noctalia built
  -- there drew its first frame in the middle of the agent's screenshot.
  t.test("waits for the call's end to move to the output the agent looks at", function()
    local host, robot = overlay()
    host.event(F.click_aim)
    host.nowMs += 500
    robot.call("update")
    local renders, calls = host.layer.renders, #host.layerCalls
    local other = fx.with(F.click_aim, { call = 2, phase = "look" })
    other.aim = { output = "DP-2", area = { width = 1280, height = 720 }, points = { { x = 300, y = 300 } } }
    host.event(other)
    host.nowMs += 100
    robot.call("update")
    t.eq(#host.layerCalls, calls, "the layer stays where it is")
    t.eq(host.layer.renders, renders, "and draws nothing new")
    host.event(fx.with(other, { phase = "end" }))
    t.eq(lastConfigure(host).output, "DP-2", "it moves once the call has ended")
  end)

  -- The bug: frame ticks drove the glide, and Noctalia stopped sending them after a
  -- frame that drew nothing new, so the robot jumped to the end of the glide.
  t.test("draws every frame while it glides, by its update interval", function()
    local host, robot = overlay()
    -- It popped in at home with the lease, and glides from there to the click.
    host.event(F.click_aim)
    t.eq(host.updateInterval, stage.FRAME_MS)
    local steps = {}
    for _ = 1, 4 do
      host.nowMs += stage.FRAME_MS
      robot.call("update")
      table.insert(steps, (corner(host.layer.tree)))
    end
    host.nowMs += stage.GLIDE_MS
    robot.call("update")
    t.ok(host.updateInterval > stage.FRAME_MS, "slower once there")
    -- The caption shows on arrival, left of the robot.
    local there = corner(host.layer.tree) + stage.INNER + stage.captionWidth("click")
    for i = 2, #steps do
      t.ok(math.abs(there - steps[i]) < math.abs(there - steps[i - 1]), "a step toward the click each frame")
    end
  end)

  -- The bug: the sizes Noctalia gives, already logical, were divided by the output's
  -- scale again, so on a scale-2 laptop the robot asked to drive mid-screen.
  t.test("asks from 48 px inside the corner of a scaled output no action has named", function()
    local host = env.host()
    host.outputs = { { name = "eDP-1", width = 1280, height = 800, scale = 2, focused = true } }
    host.focusedOutput = "eDP-1"
    host.layer.output = "eDP-1"
    host.nowMs = env.module("lib/time.luau").parse("2026-10-30T12:00:00.500Z")
    env.start(host, "service.luau")
    env.start(host, "robot.luau")
    host.event({ event = "hello", format = 1 })
    host.event(fx.state(nil, { pending = true }))
    local left, top = corner(host.layer.tree)
    t.eq(left + stage.INNER + stage.captionWidth("asking to drive") + stage.SPRITE, 1280 - stage.HOME)
    t.eq(top + stage.SPRITE, 800 - stage.HOME)
  end)

  t.test("places the robot in UI-scaled units, as Noctalia scales the tree", function()
    local host, robot = overlay()
    host.settings["accessibility.ui_scale"] = 1.25
    host.event(F.click_aim)
    host.nowMs += 500
    robot.call("update")
    local left, top = corner(host.layer.tree)
    t.near(left * 1.25 + (stage.INNER + stage.captionWidth("click") + stage.SPRITE) * 1.25, 650 - stage.GAP, 0.5)
    t.near(top * 1.25 + stage.SPRITE * 1.25, 320.4 - stage.GAP, 0.5)
  end)

  t.test("draws nothing once the agent has gone", function()
    local host, robot = overlay()
    host.event(F.click_aim)
    host.event(fx.state(false))
    host.nowMs += stage.GOODBYE_MS
    robot.call("update")
    t.eq(#images(host.layer.tree), 0)
  end)
end)

return nil
