---
title: First session
description: Bind the stop key, then let an agent look at your desktop and act on it.
sidebar:
  order: 3
---

## Bind the stop key

Before an agent acts on your desktop, give yourself a key that takes it back. Add a bind to your niri config (`~/.config/niri/config.kdl`):

```kdl
binds {
    Mod+Shift+Escape allow-inhibiting=false allow-when-locked=true hotkey-overlay-title="Stop the AI agent (niri-computer-use)" { spawn "/home/you/.cargo/bin/niri-computer-use" "stop"; }
}
```

Put the line inside your existing `binds` block, with the path `cargo install` printed. niri reloads its config when you save it; `niri validate` checks it first. `allow-inhibiting=false` keeps the key working while an app inhibits shortcuts, and `allow-when-locked=true` on the lock screen. niri passes `NIRI_SOCKET` to what it spawns, so the stop reaches this niri instance.

The key cancels the running action, takes the lease back and refuses further actions until you run:

```sh
niri-computer-use resume
```

See [Safety](../../concepts/safety/#stop-and-resume) for what a stop does and doesn't undo.

## Look first

Start your agent and ask it something that only needs looking, for example:

> Use the niri-computer-use tools: call status, then tell me which windows are open on my focused workspace.

The agent calls `status`, then `desktop_state`. Neither needs the lease, and neither changes anything. Ask about something on screen, and it takes a `screenshot`, then a region screenshot if the text is small.

## Then act

Ask for something that changes the desktop:

> Focus my browser window, then put focus back where it was.

The agent calls `acquire_desktop`, which notes the window you were on, then `focus_window`, then `release_desktop` with `restore_focus: true`. Each action's result says whether niri accepted it and what niri saw happen.

If `acquire_desktop` fails, its error says why; the [error reference](../../reference/errors/) says what to do. The most common first-run refusal is `screen_locked` while the screen is unlocked; see [Troubleshooting](../../reference/troubleshooting/#the-lease-is-refused-with-screen_locked-while-the-screen-is-unlocked).

## Let it start apps

`launch` starts only apps you list as presets in the [policy file](../../concepts/configuration/). Without one, the agent can focus, close and type into windows you open, but not start new ones.
