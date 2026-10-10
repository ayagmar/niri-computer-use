---
title: niri-computer-use
description: An MCP server that lets AI agents see and act on a niri Wayland desktop, with a lease, a stop key and an audit log.
template: splash
hero:
  tagline: An MCP server that lets an AI agent see your niri desktop and, while it holds the lease, act on it through niri's own IPC. You keep a stop key.
  actions:
    - text: Install
      link: start/install/
      icon: right-arrow
    - text: How safety works
      link: concepts/safety/
      variant: minimal
---

## What it does

An agent such as Claude Code, Pi or Codex starts `niri-computer-use serve` and gets niri's own view of your desktop: windows, workspaces, focus, monitors, screenshots that map back to the layout, the clipboard's text, and, where apps expose them, accessible buttons and fields. Once it takes the lease, it can focus windows and workspaces, start apps you listed, close windows, click, drag, scroll, type and paste, and open three Noctalia panels.

![Noctalia's control center on a dark desktop, opened by the shell_open tool in a headless nested niri](../../assets/control-center.png)

*Noctalia's control center, opened by `shell_open` in a headless nested niri during the test suite. The server answered `observed: opened` 50 ms after the call.*

## Safety, up front

- One agent at a time holds the lease, and only the lease holder acts. Looking needs no lease.
- A key bound to `niri-computer-use stop` cancels the running action and takes the lease back. An agent can't press it.
- The server sends no input while the screen is locked or its lock state is unknown. It won't type into a window the agent didn't name, send input to apps you deny, or click at pixels of a stale screenshot, and it starts apps only from presets you write.
- Every action reports `accepted`, whether niri took the request, and `observed`, what niri saw afterwards. Nothing is retried.
- Every call goes to an audit log, without the text typed, the clipboard, window titles or images.
- None of this is a sandbox. A click or a key can do anything you could do, and screenshots show the agent whatever is on your screen. Read [Safety](concepts/safety/) before you let an agent act.

## Install in three commands

From a shell inside your niri session:

```sh
git clone https://github.com/ayagmar/niri-computer-use.git
cargo install --locked --path niri-computer-use
claude mcp add --scope user niri-computer-use -- ~/.cargo/bin/niri-computer-use serve
```

That builds the server and registers it in Claude Code. [Client setup](start/clients/) covers Pi, Codex and other MCP clients, and [First session](start/first-session/) the stop key.

## Requirements

niri 26.04, Rust 1.99.0 through rustup, and `grim`, `wl-clipboard`, `wtype` and `loginctl`. Noctalia 5.2 and an accessibility bus are optional. Details are on the [Install](start/install/) page.
