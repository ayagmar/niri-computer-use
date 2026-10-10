---
title: Error reference
description: Every error name niri-computer-use returns, what causes it and what to do.
sidebar:
  order: 1
---

A failed call sets `isError` and returns `{"error": <name>, "detail": <upstream detail>}`. The names below are a stable contract; the `detail` carries niri's, Noctalia's or a program's own message, so quote it when you report a problem. This list is checked against `ErrorName` in [`src/error.rs`](https://github.com/ayagmar/niri-computer-use/blob/main/src/error.rs) when the site builds.

A mistake in the arguments isn't one of these: it comes back as plain text starting `invalid arguments:`, and nothing was done.

## Refusals that are yours to clear

An agent should stop and tell you when it gets one of these. Calling the action again won't help.

### `stopped`

**Cause:** the stop flag is set for this niri instance, by the stop key or `niri-computer-use stop`. A stop during an action cancels it with this error; anything niri had already accepted may have taken effect. A runtime directory that can't be read also counts as stopped.

**What to do:** when you want agents to act again, run `niri-computer-use resume`. If `status` shows `input_dirty`, run `niri-computer-use recover` first.

### `recovery_required`

**Cause:** the input-dirty marker exists: an earlier input call couldn't confirm that every key and button was released, for example because the server was killed mid-click. The `detail` names the marker's operation and phase.

**What to do:** run `niri-computer-use recover` and answer its question. See [Safety](../../concepts/safety/#input-that-may-be-stuck-and-recover).

### `screen_locked`

**Cause:** the screen is locked, or neither logind nor Noctalia can say that it isn't. `status` shows `lock.state` and, under `lock.logind_error`, why logind couldn't answer.

**What to do:** unlock the screen. If it is unlocked and `lock.state` is `unknown`, run niri as `niri --session` or run Noctalia; see [Troubleshooting](../troubleshooting/#the-lease-is-refused-with-screen_locked-while-the-screen-is-unlocked).

### `session_mismatch`

**Cause:** the Wayland display isn't served by the niri on niri's socket, so `WAYLAND_DISPLAY` and `NIRI_SOCKET` name two different compositors. The server checks this once at startup and then refuses input, screenshots and clipboard reads, because `wtype`, `grim` and `wl-paste` would reach the other compositor. `status` shows it under `display_error`.

**What to do:** pass the agent's client both variables from the same niri session, or neither, and start a new agent session.

### `lease_held`

**Cause:** another server holds the lease on this niri instance. The `detail` names its PID, its client and since when.

**What to do:** finish or close the other agent's session; its server gives the lease up when it releases or exits. The stop key also takes the lease back.

### `read_only`

**Cause:** this build doesn't support the running niri's major or minor version, niri sent events this build can't parse, or the policy file is invalid. The `detail` says which.

**What to do:** for niri's version, use a build that matches it (`status` shows `niri.version` and `niri.ipc_crate`). For the policy file, fix the error `status` shows under `policy.error`, then restart the agent's session.

### `app_denied`

**Cause:** the window with keyboard focus belongs to an app on `deny_input_app_ids`, or for `elements`, the window asked about does.

**What to do:** nothing, if you meant it. Otherwise remove the app from the [policy file](../../concepts/configuration/#deny_input_app_ids) and restart the agent's session.

### `untested_output_config`

**Cause:** a pointer tool was called while niri's outputs are a setup no live test covers: more than one enabled output, or a monitor with a transform other than `Normal`. The `detail` lists the enabled outputs and their transforms.

**What to do:** the pointer tools work with one monitor at transform `Normal`. Structured actions and the keyboard work with any setup.

### `refused`

**Cause:** the selected keyboard backend can't send the requested input safely: an unknown `NIRI_COMPUTER_USE_KEYBOARD` value, a text with more symbols missing from the layout than the native backend has spare keys, a missing control character, a `key` keysym the layout lacks under native, or held pointer `keys` without the native backend.

**What to do:** split the text, or type it with the default backend; the `detail` says which limit was hit. An agent shouldn't change the backend itself.

## Mistakes an agent can correct

### `lease_required`

**Cause:** an action tool was called without holding the lease.

**What to do:** call `acquire_desktop` first, if you asked the agent to act.

### `unknown_preset`

**Cause:** `launch` named a preset the policy file doesn't have. The `detail` lists the names it has.

**What to do:** use one of those names, or add a [preset](../../concepts/configuration/#preset) and restart the agent's session.

### `ref_invalid`

**Cause:** a pointer tool's `screenshot_ref` can't be used. The `detail` starts with the reason: `unknown_ref` (not a screenshot of this lease, or niri's event stream reconnected since), `expired` (over 60 seconds old), `output_changed` (the output moved, resized, changed scale or transform, or is gone) or `out_of_bounds` (the pixel is outside the image).

**What to do:** take a new screenshot and aim from it; for `out_of_bounds`, use a pixel inside the image.

### `focus_mismatch`

**Cause:** a keyboard tool's `expect` doesn't match the window with keyboard focus. The `detail` says what has focus. Often a dialog or notification took focus. Nothing was typed.

**What to do:** look at `desktop_state` and a screenshot, focus the intended window with `focus_window` if that is what was meant, then type again.

### `text_too_long`

**Cause:** `type_text` with over 1000 characters, or `paste` with over 1 MiB. Nothing was typed.

**What to do:** split the text into calls of at most 1000 characters, or use `paste`.

### `clipboard_unsaved`

**Cause:** `paste` couldn't save the clipboard whole before replacing it: every type it offers, over 16 MiB in all, or not read within two seconds. Or the clipboard holds what its owner marked as a secret (`x-kde-passwordManagerHint`). Nothing changed.

**What to do:** type the text with `type_text` instead, in parts of at most 1000 characters.

### `save_not_enabled`

**Cause:** `screenshot` was given `save_path`, but the policy file has no `capture_dir`, or `capture_dir` starts with `~/` and `HOME` isn't set.

**What to do:** add a [`capture_dir`](../../concepts/configuration/#capture_dir) and restart the agent's session, or take the screenshot without saving.

### `panel_not_allowed`

**Cause:** `shell_open` or `shell_close` named a panel other than `control-center`, `wallpaper` or `tray-drawer`. Nothing was sent.

**What to do:** use one of those three. The others, such as the session menu and the launcher, are deliberately out of reach.

### `not_accessible`

**Cause:** the window's app isn't on the accessibility bus, or has no accessible window that is this one.

**What to do:** aim at screenshot pixels for that window.

### `ambiguous_window`

**Cause:** the app has several accessible windows that could be the one asked about, so the server won't guess.

**What to do:** aim at screenshot pixels for that window.

### `element_stale`

**Cause:** an element ref's window, app or element is gone, the element is now something else, or the ref isn't from this lease.

**What to do:** call `elements` again and aim from its new list.

### `element_unmappable`

**Cause:** the element is still there but can't be aimed at now. The `detail` starts with `frame_size_mismatch` (the app's coordinates for this window can't be trusted), `not_showing`, `empty` or `outside_screenshot` (its centre is outside the screenshot the pointer aims through).

**What to do:** for `frame_size_mismatch`, aim at a screenshot pixel instead. For `not_showing`, bring it into view first; for `outside_screenshot`, take a screenshot that shows it.

## Failures upstream

These mean niri, a program or Noctalia didn't answer as expected. Report the name and the `detail`; repeating the same call in a loop won't help.

### `niri_unavailable`

**Cause:** `NIRI_SOCKET` isn't set and the server found no running niri of yours, or found several; or niri's socket is missing, refused the connection or closed it.

**What to do:** `status` shows the reason under `niri.error`. With several niri sessions, start the agent from inside the one you want or pass `NIRI_SOCKET` on (see [Session variables](../../start/clients/#session-variables)).

### `deadline_exceeded`

**Cause:** niri, a program or an app didn't answer in time: two seconds for niri and `wl-paste`, five for `grim`, three for an `elements` walk.

**What to do:** check whether niri or the app is stuck. A stopped app makes `elements` time out.

### `upstream_error`

**Cause:** niri, a program or Noctalia answered with an error or with something unreadable; the `detail` keeps its message, exit status and stderr. Also when the server's runtime directory was removed while it ran, which cancels a running action.

**What to do:** read the `detail`. After a removed runtime directory, restart the agent's session.

### `noctalia_unavailable`

**Cause:** Noctalia is installed but didn't answer on its socket within two seconds, usually because it isn't running.

**What to do:** start Noctalia, or ignore the shell tools. Without Noctalia running, the lock state relies on logind alone.
