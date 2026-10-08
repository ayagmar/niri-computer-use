---
title: Getting started
description: Install niri-computer-use, register it in Claude Code, Pi or Codex, and run a first session.
---

## Requirements

- niri 26.04
- Rust 1.99.0 through rustup; the repository pins it in `rust-toolchain.toml`
- `grim` for screenshots, `wl-paste` from wl-clipboard for the clipboard, and `loginctl` from systemd for the lock state
- Noctalia 5.2 is optional. Without it, the `shell_status` tool isn't listed.

## Install

```sh
git clone https://github.com/ayagmar/niri-computer-use.git
cargo install --locked --path niri-computer-use
```

Check it from a shell inside your niri session:

```sh
~/.cargo/bin/niri-computer-use status
```

`niri.compat` should be `ok` and `niri.version` your niri's version. The report is described under [`status`](../../reference/tools/#status).

## Register

The server reads `NIRI_SOCKET` and the session's other variables from its environment, so register it from a shell inside your niri session.

### Claude Code

```sh
claude mcp add --scope user niri-computer-use -- ~/.cargo/bin/niri-computer-use serve
```

### Pi

```sh
pi mcp add niri-computer-use --exposure direct -- ~/.cargo/bin/niri-computer-use serve
```

`--exposure direct` gives the model the tools themselves, so a screenshot reaches it as an image.

### Codex

```sh
codex mcp add niri-computer-use -- ~/.cargo/bin/niri-computer-use serve
```

Codex passes only a short list of variables to MCP servers. Add this line to the `[mcp_servers.niri-computer-use]` section that the command wrote to `~/.codex/config.toml`:

```toml
env_vars = ["NIRI_SOCKET", "XDG_RUNTIME_DIR", "WAYLAND_DISPLAY", "XDG_SESSION_ID"]
```

Without it, `status` reports `NIRI_SOCKET is not set`.

## The skill

The repository's `skills/niri-computer-use/SKILL.md` tells an agent how to use the tools: start with `status`, prefer structured data to screenshots, and take a region screenshot for small text. Link it into your agent's skills directory:

```sh
ln -s ~/projects/niri-computer-use/skills/niri-computer-use ~/.claude/skills/niri-computer-use
ln -s ~/projects/niri-computer-use/skills/niri-computer-use ~/.agents/skills/niri-computer-use
```

Use the path where you cloned the repository.

## A first session

Open a terminal showing a word, then ask the agent:

> Use the niri-computer-use tools: call status, then take a screenshot of the focused output and tell me the word in the terminal.

The agent calls `status`, takes a screenshot, and, if the text is small, a region screenshot around it.

## The stop key and the lease

One agent at a time holds the lease on your desktop (`acquire_desktop`); the tools that act on the desktop require it. Bind `niri-computer-use stop` to a key in your niri config to cancel the running action and take the lease back at any moment:

```kdl
binds {
    Mod+Shift+Escape allow-inhibiting=false allow-when-locked=true hotkey-overlay-title="Stop the AI Agent (niri-computer-use)" { spawn "/home/you/.cargo/bin/niri-computer-use" "stop"; }
}
```

Use the path where `cargo install` put the binary. `allow-inhibiting=false` keeps the key working while an app inhibits shortcuts, and `allow-when-locked=true` on the lock screen. An agent can't press it: niri binds don't fire from virtual keyboards. `niri-computer-use resume` clears the stop; if `status` shows `input_dirty`, run `niri-computer-use recover` first, which ends any stuck input program, releases any pointer button the marker names, and asks you to confirm that no key or button is held.

The lease is also refused while the screen is locked, or while neither logind nor Noctalia can say that it isn't. Running niri with `niri --session`, which sets logind's lock hint, or running Noctalia gives the server that answer.

## Launch presets

`launch` starts only apps you list in `~/.config/niri-computer-use/policy.toml`, or `$XDG_CONFIG_HOME/niri-computer-use/policy.toml`, each with a fixed command. For example:

```toml
[[preset]]
name = "firefox"
argv = ["firefox"]
app_id = "firefox"
```

The agent passes the `name`; niri starts `argv`, and the server watches for new windows with that `app_id`. A preset can't start a shell, an interpreter or another program that runs any command it is given, nor a terminal with arguments. The server reads the file when it starts, so restart the agent's session after changing it. `status` lists the names under `policy.preset_names`.

## The audit log

Every call is appended to `~/.local/state/niri-computer-use/audit.jsonl`, or `$XDG_STATE_HOME/niri-computer-use/audit.jsonl`: the time, the client, the tool, its arguments and the outcome. The directory is `0700` and the file `0600`. Clipboard text, window titles and images are never written to it.
