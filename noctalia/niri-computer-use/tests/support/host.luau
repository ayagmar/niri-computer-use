--!nonstrict
-- A fake Noctalia host: the noctalia.*, ui.*, barWidget.*, panel.*,
-- desktopWidget.* and layer.* globals the plugin uses, with a controllable clock, scripted
-- streams and commands, and synchronous state watchers.

local json = require("./json")

local M = {}

local function lookup(tree, key: string)
  local node = tree
  for part in string.gmatch(key, "[^.]+") do
    if type(node) ~= "table" then
      return nil
    end
    node = node[part]
  end
  return if type(node) == "string" then node else nil
end

local function node(kind: string)
  return function(props, children)
    return { type = kind, props = props or {}, children = children or {} }
  end
end

function M.new(sources)
  local translations = assert(json.decode(sources["translations/en.json"]))

  local host = {
    nowMs = 1_790_000_000_000,
    config = {},
    notifications = {},
    errors = {},
    state = {},
    watchers = {},
    streams = {}, -- { command, onLine } per runStream
    commands = {}, -- { command, callback, timeout } per runAsync
    toggled = {},
    settingsOpened = 0,
    outputs = { { name = "DP-1", width = 2560, height = 1440, scale = 1.25, focused = true } },
    bar = { visible = true, tree = nil, tooltip = nil, vertical = false },
    panelTree = nil,
    desktopTree = nil,
    frameTicks = false,
    updateInterval = nil,
    -- Noctalia's own settings, as getSetting() reads them.
    settings = { ["accessibility.ui_scale"] = 1 },
    focusedOutput = "DP-1",
    -- noctalia.layer calls, in order: { command, id, patch }.
    layerCalls = {},
    -- The layer instance a layer entry runs as.
    layer = { output = "DP-1", tree = nil, renders = 0 },
  }

  local function tr(key: string, vars)
    local text = lookup(translations, key)
    if text == nil then
      return key
    end
    if vars then
      text = text:gsub("{([%w_]+)}", function(name)
        local v = vars[name]
        return if v ~= nil then tostring(v) else nil
      end)
    end
    return text
  end

  local function copy(value)
    if type(value) ~= "table" then
      return value
    end
    local out = {}
    for k, v in value do
      out[k] = copy(v)
    end
    return out
  end

  local noctalia = {
    log = function() end,
    nowMs = function()
      return host.nowMs
    end,
    getConfig = function(key)
      return host.config[key]
    end,
    getSetting = function(path)
      return host.settings[path]
    end,
    focusedOutputName = function()
      return host.focusedOutput
    end,
    layer = {
      show = function(id)
        table.insert(host.layerCalls, { command = "show", id = id })
      end,
      hide = function(id)
        table.insert(host.layerCalls, { command = "hide", id = id })
      end,
      configure = function(id, patch)
        table.insert(host.layerCalls, { command = "configure", id = id, patch = copy(patch) })
      end,
    },
    tr = tr,
    json = { decode = json.decode, encode = json.encode },
    notify = function(title, body)
      table.insert(host.notifications, { title = title, body = body })
    end,
    notifyError = function(title, body)
      table.insert(host.errors, { title = title, body = body })
    end,
    setUpdateInterval = function(ms)
      host.updateInterval = ms
    end,
    togglePanel = function(id)
      table.insert(host.toggled, id)
    end,
    openSettings = function()
      host.settingsOpened += 1
    end,
    outputs = function()
      return host.outputs
    end,
    runStream = function(command, onLine)
      table.insert(host.streams, { command = command, onLine = onLine })
      return true
    end,
    runAsync = function(command, callback, timeout)
      table.insert(host.commands, { command = command, callback = callback, timeout = timeout })
      return true
    end,
    state = {
      -- Copies, as Noctalia's state does, so no entry shares another's tables.
      set = function(key, value)
        host.state[key] = copy(value)
        host.watching = true
        for _, watcher in host.watchers[key] or {} do
          watcher(copy(value))
        end
        host.watching = false
      end,
      get = function(key)
        return copy(host.state[key])
      end,
      watch = function(key, fn)
        host.watchers[key] = host.watchers[key] or {}
        table.insert(host.watchers[key], fn)
      end,
    },
  }

  local ui = {}
  for _, kind in { "row", "column", "scroll", "label", "glyph", "box", "button", "image", "separator", "spacer" } do
    ui[kind] = node(kind)
  end

  local barWidget = {
    render = function(tree)
      host.bar.tree = tree
    end,
    setTooltip = function(text)
      host.bar.tooltip = text
    end,
    setVisible = function(visible)
      host.bar.visible = visible
    end,
    isVertical = function()
      return host.bar.vertical
    end,
  }

  local panel = {
    render = function(tree)
      host.panelTree = tree
    end,
    setWantsSecondTicks = function() end,
    close = function() end,
  }

  local desktopWidget = {
    render = function(tree)
      host.desktopTree = tree
    end,
    setNeedsFrameTick = function(on)
      host.frameTicks = on
    end,
    setWantsSecondTicks = function() end,
  }

  local layer = {
    render = function(tree)
      host.layer.tree = tree
      host.layer.renders += 1
    end,
    -- Nil in a state watcher, as in Noctalia.
    outputName = function()
      return if host.watching then nil else host.layer.output
    end,
  }

  host.globals = {
    noctalia = noctalia,
    ui = ui,
    barWidget = barWidget,
    panel = panel,
    desktopWidget = desktopWidget,
    layer = layer,
  }

  -- Sends a line to the newest stream, as the watch would print it.
  function host.line(text: string)
    local stream = assert(host.streams[#host.streams], "no stream started")
    stream.onLine(text)
  end

  function host.event(value)
    host.line(json.encode(value))
  end

  -- Every label text and glyph name in a tree, in order, for asserting what shows.
  function host.texts(tree): { string }
    local out = {}
    local function walk(n)
      if type(n) ~= "table" then
        return
      end
      if n.props then
        if n.props.text then
          table.insert(out, n.props.text)
        end
      end
      for _, child in n.children or {} do
        walk(child)
      end
    end
    walk(tree)
    return out
  end

  -- The first node in a tree for which `match(node)` holds.
  function host.find(tree, match)
    if type(tree) ~= "table" then
      return nil
    end
    if match(tree) then
      return tree
    end
    for _, child in tree.children or {} do
      local found = host.find(child, match)
      if found then
        return found
      end
    end
    return nil
  end

  return host
end

return M
