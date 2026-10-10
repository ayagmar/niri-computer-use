---
title: Client setup
description: Register niri-computer-use in Claude Code, Pi, Codex or another MCP client, and install the skill.
sidebar:
  order: 2
---

niri-computer-use is a stdio MCP server: the client starts `niri-computer-use serve` and talks to it over stdin and stdout. The server finds niri through `NIRI_SOCKET` and the rest of your session through its environment, so run these commands from a shell inside your niri session.

## Claude Code

```sh
claude mcp add --scope user niri-computer-use -- ~/.cargo/bin/niri-computer-use serve
claude mcp list
```

`claude mcp list` starts the server and should print `niri-computer-use: … - ✔ Connected`. Claude Code passes its environment on to the server. The tools appear as `mcp__niri-computer-use__<tool>`.

## Pi

```sh
pi mcp add niri-computer-use --exposure direct -- ~/.cargo/bin/niri-computer-use serve
pi mcp list
```

`pi mcp list` should print `niri-computer-use: connected, 23 tools (direct, global)` with Noctalia and an accessibility bus, fewer without. `--exposure direct` gives the model the tools themselves, so a screenshot reaches it as an image. Pi passes its environment on to the server.

## Codex

```sh
codex mcp add niri-computer-use -- ~/.cargo/bin/niri-computer-use serve
```

Codex starts MCP servers with a short list of variables, and that list doesn't include niri's. Add this line to the `[mcp_servers.niri-computer-use]` section the command wrote to `~/.codex/config.toml`:

```toml
env_vars = ["NIRI_SOCKET", "XDG_RUNTIME_DIR", "WAYLAND_DISPLAY", "DBUS_SESSION_BUS_ADDRESS"]
```

Then check that Codex forwards them:

```sh
codex mcp get niri-computer-use
```

The `env:` line should name all four. `codex mcp list` doesn't start the server, so it can't tell you whether it connects. Without `NIRI_SOCKET` the server's `status` reports `NIRI_SOCKET is not set`, and without `DBUS_SESSION_BUS_ADDRESS` the `elements` tool isn't listed.

## Other MCP clients

Any client that starts stdio servers can run it. A client that reads the common `mcpServers` format takes:

```json
{
  "mcpServers": {
    "niri-computer-use": {
      "command": "/home/you/.cargo/bin/niri-computer-use",
      "args": ["serve"]
    }
  }
}
```

Use the full path to the binary. If the client starts servers with a reduced environment, as Codex and the MCP Inspector do, it must also pass on these variables from your niri session:

| Variable | Why |
|---|---|
| `NIRI_SOCKET` | niri's IPC socket; without it nothing works |
| `XDG_RUNTIME_DIR` | where the lease, the stop flag and the input-dirty marker live |
| `WAYLAND_DISPLAY` | the pointer tools, paste, and finding Noctalia |
| `DBUS_SESSION_BUS_ADDRESS` | the accessibility bus; without it `elements` isn't listed |

`NIRI_SOCKET` names niri's process, so it changes every time niri starts. A fixed value in a config file breaks after you log in again; prefer a client setting that forwards the variable by name, as Codex's `env_vars` does.

To check a config file without an agent, list the tools through the [MCP Inspector](https://github.com/modelcontextprotocol/inspector) (needs Node.js):

```sh
npx -y @modelcontextprotocol/inspector@2.9.0 --cli --config mcp.json --server niri-computer-use --method tools/list
```

It prints the tool list as JSON. The Inspector passes the server a reduced environment and the config above sets none, so it lists 22 tools at most: `elements` is missing without `DBUS_SESSION_BUS_ADDRESS`.

## The skill

The repository's skill tells an agent how to use the tools: start with `status`, prefer structured data to screenshots, read `accepted` and `observed`, and when to stop and hand back. Link it into your agents' skill directories from the directory you cloned:

```sh
mkdir -p ~/.claude/skills ~/.agents/skills
ln -s "$PWD/skills/niri-computer-use" ~/.claude/skills/niri-computer-use
ln -s "$PWD/skills/niri-computer-use" ~/.agents/skills/niri-computer-use
```

Claude Code reads `~/.claude/skills`; Pi and Codex read `~/.agents/skills`.

Next: [bind the stop key and run a first session](../first-session/).
