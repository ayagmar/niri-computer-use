---
title: Noctalia plugin
description: See which agent drives your niri desktop, what it does, and stop it from the bar.
sidebar:
  order: 5
---

The Noctalia plugin shows what agents do on your desktop while they do it. It needs Noctalia 5.4 or newer and a niri-computer-use with the [`watch`](../../reference/cli/#watch) command. An older Noctalia lists the plugin as incompatible and doesn't load it.

- **Bar.** A robot that is dim while no agent holds the desktop. While one drives, it breathes in your accent colour, with the agent's name beside it. It takes your theme's tertiary colour while an agent asks for the desktop, and its error colour while agents are stopped. Its tooltip says who, where, the last call, and how many agents are connected and how many of them run an old build. Click it for the panel, right-click it to stop agents, or to resume them once stopped.
- **Panel.** Who drives, in which project and for how long, every agent connected to this desktop with the driver first (sessions of one agent in one project share a row that counts them, with their PIDs in its tooltip; past three rows, only the first shows and `and N more` unfolds the rest until the panel next opens), a Stop button beside them while an agent holds the desktop, for five minutes after it gives the desktop back (agents often take it again before asking you something), and while `watch` is down and the plugin can't tell, a gear that opens the plugin's settings, and the last 50 calls in words, such as `Typed 12 characters and pressed Enter` or `Pressed a toggle button`; a Noctalia command reads as Noctalia describes it, such as `Toggle caffeine (idle inhibitor)`, or names the plugin an event went to. An agent's consecutive key presses, each within 30 seconds of the last, share one row, such as `Pressed 4 keys`, with the keys under it. It opens on the newest call and follows new ones; scroll up to read older calls, and Latest takes you back. A failed call shows its error's name. Resume stays disabled while input may be stuck, and the panel says to run `niri-computer-use recover`. An agent still running a build that an update replaced is marked `old build`, and the list's heading says once how many to restart.
- **Overlay.** A small robot beside where the agent acts, with a caption such as `click` or `typing 12 chars in kitty`, while an agent holds the desktop. See [Overlay](#overlay).
- **Glow.** A desktop widget the size of your output that draws a soft frame behind your windows, so it shows in niri's gaps. It breathes while an agent drives, holds the tertiary colour while one asks, and is invisible otherwise.
- **Notifications.** When an agent takes the desktop, with its project, when agents are stopped, when input may be stuck, when an agent is left on a replaced build, and when an update replaced an agent's server and its calls fail with `engine_lost` or `engine_unavailable`. That last one comes once per agent session, at its first such failure, and names the agent and says to restart it; its later failures of that kind don't notify. The same errors from a server of the installed build, such as after an engine crash or while another agent holds the desktop, count as failed actions, since restarting wouldn't help. An agent that gives the desktop back and takes it again within five minutes, as agents do before asking you something, is announced once. What the plugin finds when it starts, such as an agent already driving, is shown but not announced, except stuck input. Each but the ones for stuck input, old builds and a replaced server's lost engine can be turned off in the plugin's settings. Two more can be turned on: giving the desktop back, and failed actions, which never include an agent's own mistakes such as invalid arguments, nor the lost engine of a replaced server. Only stuck input, and a Stop or Resume that failed, are sent at critical urgency; the rest, failed actions included, at normal urgency, so Noctalia shows them as ordinary toasts.

The project is the last part of the agent's working directory, so two Claude Code sessions in different repositories are told apart. The plugin shows only what the [audit log](../../concepts/safety/#the-audit-log) holds: never typed text, window titles or clipboard contents.

## Install it

The [install script](../install-script/) installs the plugin from the same release as the binary, so the two always speak the same `watch` format, and links it into Noctalia's plugin directory when `noctalia` is on `PATH`. Then enable it:

```sh
noctalia msg plugins enable ayagmar/niri-computer-use
```

From a clone of the repository, link the plugin yourself and enable it:

```sh
ln -s "$PWD/noctalia/niri-computer-use" ~/.local/share/noctalia/plugins/niri-computer-use
noctalia msg plugins enable ayagmar/niri-computer-use
```

Noctalia keeps the enabled plugins in `~/.local/state/noctalia/settings.toml`, not in your `config.toml`. Then add the plugin's widget to a bar and its glow to the desktop in Noctalia's settings. The panel opens from the bar, or with:

```sh
noctalia msg panel-toggle ayagmar/niri-computer-use:panel
```

## Overlay

While an agent holds or asks for the desktop, the robot stands on a layer above your windows. The layer is transparent except for the robot and its caption, and clicks and keys pass through all of it, the robot included.

- **Clicks, drags, scrolls and pointer moves.** As the input goes out, the robot sets off for the point and stops a moment later 12 pixels above-left of it, or at another corner when that one doesn't fit; the corners below-right come last, since menus and tooltips open there. Its caption shows once it has arrived. A drag carries it from the start to the end.
- **Keys, typing, element actions, focus and closing a window.** Beside the window, outside it where there is room, when the window floats. A tiled window has no place niri reports, so the robot stays where it was.
- **The agent's screenshot.** While the server captures an action's evidence, the robot holds still and nothing it shows changes, so it gives the settle wait of `screenshot: true` nothing new to wait for. A tap or scroll animation, and a move to another monitor, wait until the action has ended.
- **Other states.** It shrugs with the error's name when an action fails, shows `stopped` while agents are stopped, waves with `asking to drive` in the bottom-right corner while an agent asks, waves goodbye when the agent gives the desktop back and is gone three seconds later, and dozes after eight seconds with nothing new.
- **More than one monitor.** It moves to the monitor the action is on: it leaves the one it stood on and appears on the other a moment later. Before the first action, it stands on the focused one.

The captions use the same words as the panel: the tool, a count of characters or keys, key names only for shortcuts and keys such as Enter, and the `app_id` niri reports for the window. They never show typed text, window titles or clipboard contents.

The robot is in the agent's screenshots, as any window is. To keep it out of them and off your screen, turn off the overlay setting. With a niri-computer-use whose `watch` sends no `live` lines, the robot stands in the bottom-right corner and names each call once it has ended.

## Settings

`niri-computer-use binary` is empty by default: the plugin then runs `niri-computer-use` from `PATH`, then `~/.local/bin` and `~/.cargo/bin`, since Noctalia's `PATH` often has neither. Set it to use another build. `Show a robot where the agent acts` turns the [overlay](#overlay) on or off; it is on by default. The other settings turn each notification on or off and set the glow's width. The bar widget has two of its own: whether it shows while no agent drives, and whether it shows the agent's name.

## What it says when it can't watch

| The panel says | Why | What to do |
|---|---|---|
| niri-computer-use not found | None of the places above has it | Install it, or set its path |
| This niri-computer-use has no watch command | The binary is older than `watch` | Update it, or set the path of a build that has it |
| niri-computer-use watch failed, with an error | `watch` exited, such as without a niri socket | The error, up to its first three lines, says why; the plugin tries again every 15 seconds |
| Not watching | `watch` printed nothing for 15 seconds | It is started again within 15 seconds |
| Starting, with an error | `watch` exited after it started, such as when it couldn't write a line | The error, up to its first three lines, says why; it is started again within 15 seconds |

## How it works

The plugin's service runs `niri-computer-use watch` and reads its JSON lines: the lease holder, the stop flag, the input-dirty marker and each call the audit log records. The bar, the panel, the glow and the overlay draw what the service publishes. The overlay's layer covers the output and exists only while an agent holds or asks for the desktop. Stop and Resume run `niri-computer-use stop` and `niri-computer-use resume`, the commands your stop key runs, and a failure shows as a notification with the command's output.
