---
title: Client setup
description: Register niri-computer-use in Claude Code, Pi, Codex or another MCP client, and install the skill.
sidebar:
  order: 2
---

niri-computer-use is a stdio MCP server: the client starts `niri-computer-use serve` and talks to it over stdin and stdout. No client needs more than the command. The server finds your niri session itself when its environment lacks `XDG_RUNTIME_DIR`, `NIRI_SOCKET` or `WAYLAND_DISPLAY`, as it does under Codex; see [Session variables](#session-variables).

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

`pi mcp list` should show `niri-computer-use` as connected. How many tools it counts depends on the session, from 20 to 25: see the [tools overview](../../tools/overview/). `--exposure direct` gives the model the tools themselves, so a screenshot reaches it as an image. Pi passes its environment on to the server.

## Codex

```sh
codex mcp add niri-computer-use -- ~/.cargo/bin/niri-computer-use serve
```

Codex starts MCP servers with a short list of variables that leaves out niri's, so the server finds the session itself; `env_vars` isn't needed. `codex mcp list` doesn't start the server, so ask the agent to call `status`: `discovery` shows `discovered` for the three session variables, and `niri.error` is null.

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

Use the full path to the binary. A client that starts servers with a reduced environment, as Codex and the MCP Inspector do, needs nothing more.

To check a config file without an agent, list the tools through the [MCP Inspector](https://github.com/modelcontextprotocol/inspector) (needs Node.js):

```sh
npx -y @modelcontextprotocol/inspector@2.9.0 --cli --config mcp.json --server niri-computer-use --method tools/list
```

It prints the tool list as JSON.

## Session variables

When one of these is unset or empty, the server finds it at startup:

| Variable | Found as |
|---|---|
| `XDG_RUNTIME_DIR` | the directory holding `NIRI_SOCKET` when that is set, otherwise `/run/user/<your uid>`; either must be your directory with mode `0700`, as logind creates it |
| `NIRI_SOCKET` | the one socket in the runtime directory named `niri.<display>.<pid>.sock`, as niri names it, that is yours, whose process is a running `niri`, and that this `niri` answers on; when `WAYLAND_DISPLAY` is set, only a socket for that display counts |
| `WAYLAND_DISPLAY` | the `<display>` in that socket's name, if the runtime directory has a socket by that name |

The session bus for `elements` is `$XDG_RUNTIME_DIR/bus` unless `DBUS_SESSION_BUS_ADDRESS` is set. `status` reports under `discovery` whether each came from the environment or was discovered, and why it is missing when neither.

A variable you set always wins, and the server finds the rest to match it. That matters when you run more than one niri session as the same user: the server never picks one of several, and `status` reports `niri_unavailable` naming the sockets it found. Start the agent from a shell inside the session you want, or pass `NIRI_SOCKET` on. `NIRI_SOCKET` names niri's process and changes every time niri starts, so forward it by name, as Codex's `env_vars = ["NIRI_SOCKET"]` does, rather than writing a fixed path into a config file.

Forwarding `NIRI_SOCKET` alone is enough when niri's runtime directory is yours with mode `0700`, as `/run/user/<your uid>` is. For a niri whose runtime directory isn't, forward `XDG_RUNTIME_DIR` too; otherwise `status` shows the runtime directory missing, and the lease, input and screenshots are refused.

Forward the variables from one niri session. When `WAYLAND_DISPLAY` belongs to another compositor than `NIRI_SOCKET`, the server refuses input, screenshots and clipboard reads with `session_mismatch`, and `status` says why under `display_error`.

Without the variables, a server started over SSH, from a TTY or as a systemd service still finds your niri session when it is the only one, and an agent using it acts on your desktop. The lease, the stop key and the lock check still apply, but nothing asks whether you meant it.

## The skill

The repository's skill tells an agent how to use the tools: start with `status`, prefer structured data to screenshots, read `accepted` and `observed`, and when to stop and hand back. Link it into your agents' skill directories from the directory you cloned:

```sh
mkdir -p ~/.claude/skills ~/.agents/skills
ln -s "$PWD/skills/niri-computer-use" ~/.claude/skills/niri-computer-use
ln -s "$PWD/skills/niri-computer-use" ~/.agents/skills/niri-computer-use
```

Claude Code reads `~/.claude/skills`; Pi and Codex read `~/.agents/skills`. The same files are on this site under [For agents](../../reference/agents/).

Next: [bind the stop key and run a first session](../first-session/).
