---
title: Background desks
description: Give an agent a niri desktop of its own, beside yours, so it never moves your focus or types into your windows.
sidebar:
  order: 5
  badge:
    text: Experimental
    variant: caution
---

:::caution[Experimental]
Desks are new in this release. The commands and the files they write may change before they settle.
:::

A desk is a second niri that runs headless beside your desktop, with nothing on your screen. An agent served on a desk launches its apps there, sees only their windows, and types and clicks into them while you keep working: your focus, your workspaces and your windows don't change. Your lock screen and your stop key still apply to it.

## What it needs

A desk runs four programs besides niri, and `desk start` names any it can't find before it starts anything:

- `cage`, which gives the desk's niri a headless display to draw on. cage 0.3.1 is tested.
- `dbus-daemon`, for the desk's own session bus.
- at-spi2-core's `at-spi-bus-launcher` and `at-spi2-registryd`, for the desk's accessibility bus. They are looked for in `/usr/lib`, `/usr/libexec` and `/usr/lib/at-spi2-core`.

Only `desk start` needs them; the rest of the server doesn't.

## Start, list and stop

```sh
niri-computer-use desk start web
```

```text
niri-computer-use: desk web: config
niri-computer-use: desk web: parent
niri-computer-use: desk web: bus
niri-computer-use: desk web: niri
niri-computer-use: desk web: accessibility
niri-computer-use: desk web: round_trips
{
  "name": "web",
  "mode": "headless",
  "size": {
    "width": 1920,
    "height": 1080
  },
  "phase": "ready",
  "round_trips": {
    "count": 40,
    "p50_us": 304,
    "p95_us": 574,
    "max_us": 693
  },
  ...
}
```

It prints each step as it starts and, once the desk is ready, the desk's record, then exits 0. The desk keeps running after `desk start` returns. A name is 1 to 16 characters of `a` to `z`, `0` to `9` and `-`, not starting with `-`. `--size 1280x720` sets the desk's output size, each side 320 to 7680 pixels; the default is 1920x1080.

```sh
niri-computer-use desk list
niri-computer-use desk stop web
```

`desk list` prints every desk as JSON, with `running` and its record. `desk stop` stops every process of the desk, the apps the agent launched included, and removes its directory; it took 0.2 s here. On a desk whose processes are already gone, it removes what they left.

A desk that fails to start is stopped whole before `desk start` exits 1 with the failed step and why. Each process's log is in `~/.local/state/niri-computer-use/desks/<name>/` (or under `$XDG_STATE_HOME`), which stays after the desk stops.

The desk's processes run under one supervisor. If the supervisor is killed, the kernel kills them with it.

## Serve an agent on a desk

Register the server in your client as for your desktop ([Client setup](../../start/clients/)), with `--desk` and the desk's name after `serve`:

```sh
niri-computer-use serve --desk web
```

The server joins a desk that is ready and fails with the reason otherwise, such as `there is no desk web; start it with niri-computer-use desk start web`. It never starts a desk and never falls back to your desktop. `status`, `watch` and `recover` take `--desk <name>` the same way; `status` and `watch`'s `hello` then name the desk in `desk`.

What the server uses on a desk:

| From the desk | From your session |
|---|---|
| niri, its windows, workspaces and outputs | the lock state: logind's hint for your niri and your Noctalia |
| the Wayland display, for screenshots, input and the clipboard | the stop key, read from your niri's config |
| the session bus and the accessibility bus | notifications (`notify_on_lease`, `confirm_lease`) |
| the lease, the stop flag and the input-dirty marker | the policy file, the audit log and `systemd-run` |

The desk has no shell, so `status.noctalia` is `not_installed` and the Noctalia tools aren't listed. The Noctalia plugin shows only agents on your desktop.

## What the desk's apps get

Apps on a desk get the desk's niri, display, session bus and accessibility bus, a `TMPDIR` inside the desk, and no system bus. They don't see your niri, your display or your session bus. Accessibility is on before the first app starts, so a Chromium-based browser is on the accessibility bus from its first start, with no restart: Helium listed 35 elements here on its first `elements`.

`launch` starts your [presets](../configuration/#preset) and, with [`launch_desktop_entries`](../configuration/#launch_desktop_entries), desktop entries, inside the desk. A Chromium-based browser hands its command line to a running instance of the same profile, so a browser preset without a profile of its own opens its window on your desktop, in your browser. Give it a `--user-data-dir` inside the desk:

```toml
[[preset]]
name = "browser"
argv = ["helium-browser", "--user-data-dir=/run/user/1000/ncu-d/web/browser", "--ozone-platform=wayland", "--no-first-run", "about:blank"]
app_id = "helium"
```

A desk's directory is `$XDG_RUNTIME_DIR/ncu-d/<name>/`, and `desk stop` removes it, so that profile lasts as long as the desk.

## Lock, stop and resume

While your screen is locked, or its lock state is unknown, a desk's server refuses actions with `screen_locked`. Screenshots, accessible names and window titles follow your lock state the same way they do on your desktop. There is no setting that lets an agent act on a desk while your screen is locked.

`niri-computer-use stop`, and so your stop key, stops every desk's agent as well as your desktop's: it sets your instance's stop flag and a flag in `$XDG_RUNTIME_DIR/ncu-d/` that every desk's server checks before it admits an action and before each batch of input. `resume` clears both, your instance's first. It never clears a desk's input-dirty marker; `recover --desk <name>` does, for that desk.

## The responsiveness check

niri drawing into another compositor waits for that compositor's frames unless the GPU driver's swap interval is off, so a desk's niri always gets `__GL_SYNC_TO_VBLANK=0` (NVIDIA) and `vblank_mode=0` (Mesa). Before a desk is ready, the supervisor measures 40 round trips to its niri. If the 95th percentile is over 20 ms, the desk is stopped and `desk start` fails with the numbers; there is no fallback. Here a ready headless desk answered in 0.6 ms at the 95th percentile.

## Windowed desks

`desk start --windowed <name>` draws the desk in a window on your desktop instead, for watching what the agent does. The desk's size is then the window's, and `--size` is refused. A hidden or covered window stalls the desk's niri, which the responsiveness check refuses at the start but can't catch later.
