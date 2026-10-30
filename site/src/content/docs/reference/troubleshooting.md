---
title: Troubleshooting
description: "Fixes for common problems: the lease refused while unlocked, pointer tools refused on a rotated monitor, a server that finds no niri, and more."
sidebar:
  order: 3
---

Each problem here was seen in testing, on a real desktop or in a nested niri. For any error name, the [error reference](../errors/) has the cause and what to do.

## `status` reports `NIRI_SOCKET is not set`

The server got no `NIRI_SOCKET` and found no niri session of yours. `niri.error`'s `detail` says where it looked, and `discovery` shows what else is missing. Usually niri isn't running as your user, or the server runs as another user or in a container that can't see `/run/user/<uid>`. Clients with a reduced environment, such as Codex, need no `env_vars`: the server finds the session itself (see [Session variables](../../start/clients/#session-variables)).

## `status` reports several running niri instances

You run more than one niri session as the same user, and the server won't guess which one to drive. Start the agent from a shell inside the session you want, or make the client pass `NIRI_SOCKET` on; a set variable always wins.

## The lease is refused with `screen_locked` while the screen is unlocked

The server acts only when it knows the screen is unlocked, and here it doesn't know. `status` shows `lock.state: unknown` and why under `lock.logind_error`, for example:

```text
niri runs without --session, so it sets no logind locked hint
```

niri sets logind's lock hint only when it runs as `niri --session`, which is how display managers and `niri-session` start it. Start niri that way, or run Noctalia, which reports the lock state itself. Without either, the server won't act.

## Every action fails with `stopped`

Someone pressed the stop key or ran `niri-computer-use stop`; `status` shows `stop: true`. Run `niri-computer-use resume`. The flag is per niri instance, so it stays across agent sessions until you clear it.

## Every action fails with `recovery_required`

An earlier input call ended without confirming that every key and button was released, usually because its server was killed. `status` shows the marker under `input_dirty`. Run `niri-computer-use recover`, do the check it asks for, and type `yes`.

## `recover` says a server holds the lease

```text
niri-computer-use: a server holds the lease: PID … (…); end that agent or its server first
```

`recover` won't run while an agent holds the lease. End that agent's session, or press the stop key, which takes the lease back, then run `recover` again.

## `shell_panel` is missing, or Noctalia is unavailable

`shell_panel` is listed, and `desktop_state` has `shell`, only when the server found `noctalia` when it started; `status.desktop_programs` shows where it looked. Install Noctalia and restart the agent's session. If it is installed outside those directories, such as in a Nix user profile or `~/.local/bin`, set [`NIRI_COMPUTER_USE_DESKTOP_PATH`](../../concepts/configuration/#environment) to the same directories for every client. In shared mode, a client that sets other directories is served standalone, and its bridge says so on stderr: `refused this client (session_mismatch): the bridge looks for loginctl and noctalia in …`.

If `shell_panel` fails, or `desktop_state`'s `shell` is, `noctalia_unavailable`, Noctalia isn't running: `status` shows `noctalia: not_running` and a `noctalia_error` such as `…/noctalia-wayland-1.sock: connect: No such file or directory`. Start Noctalia. While it isn't running, the lock state depends on logind alone, so without `niri --session` the lease is refused.

## `elements` and the element actions aren't listed

The server found no accessibility bus when it started; `status` shows why under `accessibility.reason`. The server uses `DBUS_SESSION_BUS_ADDRESS`, or else the user bus at `$XDG_RUNTIME_DIR/bus`. Check that at-spi2-core is installed, then restart the agent's session.

## `elements` fails with `not_accessible` on a browser or Electron app

Chromium, Electron and CEF apps, such as a Chromium-based browser, VS Code or a Discord client, check the session's accessibility once, when they start. One that started while it was off stays off the bus, even though the server turns accessibility on afterward. Restart the app. To have it on for every app you start, run:

```sh
gsettings set org.gnome.desktop.interface toolkit-accessibility true
```

and restart the app. For a single app, start it with `--force-renderer-accessibility`. The agent can still work on screenshots meanwhile, but not by element name. [Chromium and Electron apps](../../tools/elements/#chromium-and-electron-apps) explains what the server's `turn_on_accessibility` does and doesn't do.

## `elements` lists elements without a `layout_box`

With `unmappable: frame_size_mismatch`, the app draws its own title bar and its coordinates can't be matched to niri's window. GTK 3, Qt, Chromium and Electron apps do this; with server-side decorations, `prefer-no-csd` in niri's config, GTK 3, Qt and Chromium work. Aim at screenshot pixels for that window, or use `activate_element` and `set_element_text`, which don't need a box; Chromium's text fields refuse `set_element_text`, so focus them with `activate_element` and use `type_text`. A click on such an element is refused with `element_unmappable` rather than land in the wrong place.

With `unmappable: tiled`, the window is tiled. niri 26.04 gives only floating windows a place in the workspace's view, which the box is measured from. `activate_element` and `set_element_text` work on tiled windows; to click an element by its ref when changing the user's layout is fine, float the window first with `niri_action` `ToggleWindowFloating`, then call `elements` again. Floating doesn't help a `frame_size_mismatch`.

With `unmappable: page_scale_unknown`, the element is in a web page, as in Chromium, at an output scale other than 1, whose coordinates the server can't tell to be in logical pixels or the output's. Aim at screenshot pixels, or use `activate_element`, which doesn't need a box.

## `elements` fails with `deadline_exceeded`

The app didn't answer one call over the accessibility bus within a second, or the three seconds ran out before the walk began. A stopped or hung app does this; in testing, a stopped Qt app failed after about a second while other apps kept answering. A large tree that is slow to read doesn't fail: it gives what was read in three seconds, with `capped_reason: "budget_exhausted"`.

## The pointer tools fail with `untested_output_config`

They run on monitors at transform `Normal`, and in nested niri's window when it is the only output. The screenshot they aimed through was of a rotated monitor, or of nested niri's window next to another output. Aim through a screenshot of another monitor, or turn that one back to `Normal`. `status` lists the monitors they run on in `outputs.pointer_outputs`. Focus, launch, close, the keyboard tools and screenshots still work on any monitor.

## A click fails with `ref_invalid` `out_of_bounds`

The pixel is outside the screenshot. In testing this happened with a tiled window taller than the monitor: the control the agent wanted was below the screen's edge. Bring it into view first, by scrolling or resizing the window, then take a new screenshot.

## Typing fails with `focus_mismatch`

The window the agent named in `expect` lost keyboard focus before typing started, often to a dialog or a notification. Nothing was typed. The agent should look at what took focus, focus its window again with `focus`, and type again.

## A paste fails with `clipboard_unsaved`

The clipboard couldn't be saved whole: it held over 16 MiB, its owner didn't answer within two seconds, a password manager marked its contents as secret, or something else was copied while it was being saved. Nothing changed. Type the text with `type_text` and `via: "keys"` instead. A paste's own text, left on the clipboard by a `kept` outcome, doesn't cause this: the next paste replaces it.

## Actions fail after the runtime directory was cleaned

If something removes the runtime directory, `niri-computer-use/<instance>/` beside niri's socket with every symlink resolved, while a server runs, the server gives up the lease and refuses it until it restarts, and an action in progress fails with `upstream_error`. Restart the agent's session. A shared engine exits instead, within about a second: a call in flight fails with `engine_lost`, or, with none in flight, the session's next call does, and the call after that reaches a new engine.

## Shared mode serves the client standalone

With [`shared`](../../concepts/configuration/#shared) on, the default, `status.engine.mode` says `standalone` and `engine.fallback` says why; the server's stderr has the same line. If the engine runs another build, you reinstalled while it ran. An engine newer than 0.1.5 hands over to the first client of the new build that connects while no agent holds the desktop. One that refused this client was held, and the fallback says so: it hands over once that agent gives the desktop up. An engine of 0.1.5 or older keeps serving until its clients have ended. Either way, the fallback names the old engine's PID and says to restart the clients it serves, and those clients see `engine.replaced: true`; restarting them ends the wait. If neither build was replaced, as with one install by `cargo install` and another by the install script, the fallback names both files instead: point every MCP client at one of them, since restarting a client runs its own build again. If the engine's socket path is too long, niri's socket directory is: the socket is `niri-computer-use/<instance>/engine.sock` in the directory holding niri's socket, with every symlink resolved, where `<instance>` is niri's socket name without `.sock`, and it must fit in 108 bytes. `/run/user/<uid>` fits; a long test or container directory may not. If no engine answered, read `engine.log` next to that socket.

## An action comes back `timeout`, `pending` or `interrupted`

These aren't errors. `pending` from `close_window` usually means an unsaved-changes dialog. `interrupted` means focus went to a window the action wasn't about, so someone else is using the desktop. The result carries a screenshot; look at it before deciding what to do. Don't repeat a `launch` or `close_window` blindly: a second launch opens a second window, and a second close can answer the app's dialog.
