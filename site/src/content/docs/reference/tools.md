---
title: Tools reference
description: Every tool niri-computer-use offers, with its arguments, results and errors.
---

The perception tools are read-only and carry the `readOnlyHint` annotation. The two lease tools change only the lease, never the desktop. The four action tools change the desktop through niri's IPC and need the lease; `close_window` carries `destructiveHint`. `shell_open` and `shell_close` change which Noctalia panel is open and need the lease too. Each successful result has the data as `structuredContent` and the same JSON as text.

## Errors

A failure sets `isError` and returns `{"error": <name>, "detail": <upstream detail>}`:

| Name | Meaning |
|---|---|
| `niri_unavailable` | niri's socket is missing, refused the connection, or closed it |
| `deadline_exceeded` | niri or a program didn't answer in time: two seconds for niri and `wl-paste`, five for `grim` |
| `upstream_error` | niri, a program or Noctalia answered with an error, or with something unreadable; `detail` keeps its message, exit status and stderr. Also when the server's runtime directory was removed, which cancels a running action; the server must then be restarted |
| `noctalia_unavailable` | Noctalia is installed but didn't answer on its socket within two seconds |
| `lease_held` | another agent's server holds the lease; `detail` names its PID, label and since when |
| `lease_required` | an action tool was called without holding the lease |
| `stopped` | the stop flag is set, or was set while an action ran and cancelled it; the user clears it with `niri-computer-use resume` |
| `read_only` | this build doesn't support the running niri, niri sent events it can't parse, or the policy file is invalid |
| `screen_locked` | the screen is locked, or neither logind nor Noctalia can say whether it is |
| `recovery_required` | input may be stuck; `detail` names the marker's operation and phase, and the user runs `niri-computer-use recover` |
| `unknown_preset` | `launch` named a preset the policy file doesn't have; `detail` lists the names it has |
| `ref_invalid` | a pointer tool's `screenshot_ref` can't be used; `detail` starts with the reason: `unknown_ref` (not a screenshot of this lease, or niri's event stream reconnected since), `expired` (over 60 seconds old), `output_changed` (the output moved, resized, changed scale or transform, or is gone) or `out_of_bounds` (the pixel is outside the image) |
| `untested_output_config` | a pointer tool while niri's outputs are a setup no live test covers; `detail` lists the enabled outputs and their transforms |
| `app_denied` | input while the focused window's `app_id` is on the policy file's `deny_input_app_ids` |
| `focus_mismatch` | a keyboard tool's `expect` doesn't match the window with keyboard focus; `detail` says what has focus |
| `text_too_long` | `type_text` with over 1000 characters; `detail` gives the length and says nothing was typed |
| `panel_not_allowed` | `shell_open` or `shell_close` named a panel other than `control-center`, `wallpaper` or `tray-drawer` |

A mistake in the arguments, such as an unknown output or a value of the wrong type, comes back with `isError` and one plain-text block starting `invalid arguments:`, without `structuredContent`, so the model can correct the call.

## `status`

No arguments. The readiness report, also printed by `niri-computer-use status`:

| Field | Value |
|---|---|
| `instance` | the basename of `NIRI_SOCKET`, which names the niri instance |
| `niri.version` | niri's version string, such as `26.04 (8ed0da4)` |
| `niri.ipc_crate` | the `niri-ipc` version this build uses, `26.4.0` |
| `niri.compat` | `ok` when the major and minor versions match, `patch_warning` when only the patch differs, `read_only` otherwise |
| `niri.event_stream` | `connected`, `disconnected`, or `schema_incompatible` after niri sent two events this build can't parse; null from the `status` subcommand, which opens no stream |
| `niri.error` | why niri's version couldn't be read, or null |
| `lease.held_by_me` | whether this server holds the lease |
| `lease.holder` | the holder's `pid`, `label` (client name and server PID, such as `claude-code/4711`) and `since`, or null |
| `input_dirty` | the input-dirty marker, or null: its `operation`, `phase` (`pending` or `running`), `server_pid`, `since`, the input `child` once known, and any pointer `buttons` pressed; `{"error": …}` if the marker can't be read |
| `stop` | whether the stop flag is set for this niri instance (`niri-computer-use stop`, cleared by `niri-computer-use resume`) |
| `lock.state` | `locked`, `unlocked` or `unknown` |
| `lock.source` | `logind`, `noctalia` or `none`: the screen counts as locked when logind's `LockedHint` or Noctalia's `locked` says so |
| `lock.session` | the logind session asked: the one in niri's own `XDG_SESSION_ID`, which is where niri sets the hint |
| `lock.logind_error` | why logind couldn't answer, or null |
| `outputs.pointer_supported`, `outputs.reason` | whether niri's outputs are a setup the virtual pointer is tested on (one enabled output: a monitor at transform `Normal`, or nested niri's `winit` window), and why not |
| `noctalia` | `running`, `not_running` or `not_installed` |
| `noctalia_error` | why Noctalia counts as not running, or null |
| `policy.state` | `loaded`, `missing` (valid: no presets, no denied apps) or `invalid` |
| `policy.presets`, `policy.denied_app_ids`, `policy.error` | how many presets and denied apps the policy file has, and why it is invalid |
| `policy.preset_names` | the preset names `launch` takes |
| `audit.path`, `audit.last_error` | the audit log and the last failure to write it |
| `binaries` | whether `grim`, `wl-paste`, `wl-copy`, `wtype` and `loginctl` are on `PATH` |

## `desktop_state`

No arguments. One snapshot of niri's event stream:

- `windows`, by id: niri's window objects, with `id`, `title`, `app_id`, `pid`, `workspace_id`, `is_focused`, `is_floating`, `is_urgent` and `layout`
- `workspaces`, by output and index
- `focused_window`: the focused window's id, or null while keyboard focus is outside the window layout, for example on a shell panel, the lock screen or the overview
- `overview_open`
- `keyboard_layouts`: the layout names and the current index

Right after the server starts or reconnects, it waits up to two seconds for niri's initial state.

## `outputs`

No arguments. niri's outputs by connector name, such as `DP-1`, as niri reports them: make, model, modes, and under `logical` the position and size in layout coordinates, the scale and the transform. A disabled output has `logical: null`.

## `screenshot`

| Argument | Value |
|---|---|
| `target` (required) | `focused_output`, `output:<name>` with a name from `outputs`, or `region` |
| `region` | with target `region`: `{x, y, width, height}` in layout coordinates, inside one output |
| `max_width` | the widest image to return, in pixels; default 1280 |
| `format` | `jpeg` (default, quality 80) or `png` |

The capture uses the output's own scale, lowered when the captured width times that scale is wider than `max_width`. For small text, take a region around it.

While one of this server's actions is running, `screenshot` waits for it to end before capturing, so a screenshot sent alongside an action shows the screen after it.

The result is an image block, then the metadata as text and as `structuredContent`:

| Field | Value |
|---|---|
| `output`, `transform` | the captured output and its transform |
| `output_origin` | the output's top-left corner in layout coordinates |
| `captured` | the captured rectangle in layout coordinates |
| `scale` | image pixels per logical pixel |
| `width`, `height`, `mime_type` | the image, checked against its own header |
| `captured_at_unix_ms`, `capture_ms` | when the capture started and how long it took |
| `screenshot_ref` | an id such as `shot-4` for this capture while this server holds the lease, or null. The server keeps the last 64 of the current lease in memory and drops them all when the lease is taken or given up |

`grim` gets five seconds and at most 64 MiB of output.

## `clipboard_read`

No arguments. Runs `wl-paste --no-newline --type text` and returns `{"text": ..., "reason": null}`, or `text: null` with `reason` `nothing_copied` or `no_text`. Text over 1 MiB or not valid UTF-8 is an `upstream_error`.

## `shell_status`

No arguments. Listed only when `noctalia` is on `PATH`. Noctalia's own status reply: `barVisible`, `panelOpen`, `activePanelId` and `locked`. `noctalia_unavailable` when Noctalia doesn't answer; an `error:` reply from Noctalia is an `upstream_error` with its text.

## `shell_open` and `shell_close`

| Argument | Value |
|---|---|
| `panel` (required) | `control-center`, `wallpaper` or `tray-drawer` |

Listed only when `noctalia` is on `PATH`. They need the lease and pass the same checks as the action tools below. Any other panel, such as the session menu or the launcher, is refused with `panel_not_allowed`, and nothing is sent. `noctalia_unavailable` when Noctalia doesn't answer `status`, and Noctalia's `error:` reply is an `upstream_error`.

The tool sends `panel-open` or `panel-close`, then reads Noctalia's `status` every 100 ms for up to two seconds. `observed` is `opened` once `activePanelId` is the panel, `closed` once it isn't, `timeout` if neither happens in time, or `uncertain` if Noctalia's reply or a later `status` was lost. When the panel is already open, or already isn't, nothing is sent and `accepted` is false. The result has the action tools' fields, plus `shell.active_panel`, the open panel when the observation ended, or null; it is absent when that is unknown. An open panel holds keyboard focus, so `focused_window` is null while it is open; type into it with `expect: "none"`.

## `acquire_desktop`

No arguments. Takes the lease on this niri instance and returns `{"holder": {"pid", "label", "since"}}`. One server holds it at a time; the action tools require it. Calling it again while holding the lease returns the same holder.

Refused with `lease_held` while another server holds it, `stopped` while the stop flag is set, `recovery_required` while the input-dirty marker exists, the niri error when niri's version can't be read, `read_only` when this build doesn't support the running niri, niri sent events it can't parse, or the policy file is invalid, and `screen_locked` while the screen is locked or its lock state is unknown. If the runtime directory can't be read, it fails with `upstream_error` rather than assume neither flag is set.

## `release_desktop`

No arguments. Gives the lease up and returns `{"released": true}`, or `{"released": false}` if this server didn't hold it. While an action runs, it waits for that action to end, at worst about fifteen seconds. The lease is also given up when the stop flag appears and when the server exits.

## Action tools

`focus_window`, `focus_workspace`, `launch` and `close_window` act on the desktop through niri's IPC, and the pointer and keyboard tools send input. Before each action the server checks, in this order, the stop flag (`stopped`), the input-dirty marker (`recovery_required`), the lease (`lease_required`), then niri's version, the policy file and the lock state, as `acquire_desktop` does. One action runs at a time; an action or `release_desktop` called meanwhile waits for it. A stop during an action cancels it with `stopped`, and anything niri had already accepted may have taken effect. Cancelling the MCP request cancels the action and keeps the lease.

An unknown window or workspace id is an argument mistake, and nothing is sent. Otherwise the result has these fields:

| Field | Value |
|---|---|
| `accepted` | true once niri acknowledged the request; false when nothing was sent; null when the request was sent but niri's reply was lost |
| `observed` | what niri's event stream showed afterwards; the values are listed under each tool |
| `focused_window` | the window with keyboard focus when the observation ended, or null when focus isn't on a window or the reply was lost |
| `windows` | the windows the outcome is about, when there are any |
| `detail` | why the outcome is `uncertain` |
| `screenshot` | with an outcome in doubt, the metadata of a fresh screenshot of the focused output, as `screenshot` returns it; the image follows the text in the result's content |
| `screenshot_error` | with an outcome in doubt, `{"error", "detail"}` saying why there is no screenshot |

Every action waits up to five seconds for its effect. Two outcomes can end any of them:

- `interrupted`: focus moved to a window that was neither focused before nor the expected target, so someone else is using the desktop. `focused_window` says where focus went.
- `uncertain`: niri's reply or event stream was lost, so the action may or may not have happened. `accepted` is null when the reply was lost.

None of these outcomes is an error, and the server never retries an action. `timeout`, `pending`, `none`, `interrupted` and `uncertain` are outcomes in doubt: the result comes with a fresh screenshot of the focused output, 1280 pixels wide, so the agent can see the desktop without another call.

## `focus_window`

| Argument | Value |
|---|---|
| `id` (required) | a window id from `desktop_state` |

`observed` is `focused` once the window has keyboard focus, or `timeout`. If the window already has focus, nothing is sent: `observed` is `focused` with `accepted: false`.

## `focus_workspace`

| Argument | Value |
|---|---|
| `id` (required) | a workspace id from `desktop_state`, not its index |

`observed` is `focused` once the workspace has focus, on whichever output it is, and keyboard focus is on the workspace's active window, or on no window when it is empty; otherwise `timeout`. While the overview, the screenshot UI or a shell surface that keeps keyboard focus is open, focus stays off every window, so a switch that happened still reports `timeout` with `focused_window` null; the screenshot shows why. Focus moving to one of the workspace's own windows is expected, not an interruption. If the workspace already has focus, nothing is sent: `observed` is `focused` with `accepted: false`. Sending it would let niri's `workspace-auto-back-and-forth` switch to the previous workspace.

## `launch`

| Argument | Value |
|---|---|
| `preset` (required) | a preset name from the policy file, listed in `status` as `policy.preset_names` |
| `reuse` | default false; with true, focus the preset's existing window instead of starting another |

niri starts the preset's fixed `argv`; the app keeps running after the server exits. `observed` counts the windows with the preset's `app_id` that weren't open before, including one that sets its `app_id` after it appears. Once the first appears the server keeps counting for half a second, then reports `one` or `ambiguous`, with the ids in `windows`. With no such window in five seconds it reports `none`. Focus moving to a new window, before it has its `app_id`, isn't an interruption. A single-instance app hands a second start to its running process, which focuses the window it already has: `observed` is then `focused`, with that window in `windows`.

With `reuse`: one existing matching window is focused, and `observed` is `focused` with its id in `windows`; several give `ambiguous` with their ids and `accepted: false`, and nothing is started; none starts the preset as usual.

## `close_window`

| Argument | Value |
|---|---|
| `id` (required) | a window id from `desktop_state` |

Asks the window to close, as its close button would. `observed` is `closed` once niri reports it gone, or `pending` if it is still open after five seconds, for example behind an unsaved-changes dialog. Nothing forces it closed. `windows` holds the id.

## Pointer tools

`pointer_move`, `click`, `drag` and `scroll` aim at pixels of a screenshot: each takes the `screenshot_ref` of a screenshot this server took under the current lease, and pixel coordinates in that image, counted from its top-left corner. A pixel targets its centre. They run through the same gate as the other action tools. Then, before sending anything, the server checks:

1. the arguments: `count` from 1 to 3, at least one scroll axis, at most 10 notches each way (otherwise an argument mistake)
2. the ref: `ref_invalid` with `unknown_ref` if it isn't a screenshot of this lease
3. the focused window: `app_denied` if its `app_id` is on the policy's deny list
4. niri's outputs, read again right then: `untested_output_config` unless there is one enabled output, a monitor at transform `Normal` or nested niri's `winit` window
5. each pixel through the ref: `ref_invalid` with `expired` past 60 seconds, `unknown_ref` if niri's event stream reconnected since the screenshot, `output_changed` if its output moved, resized, changed scale or transform, or is gone, and `out_of_bounds` for a pixel outside the image

The input goes through a virtual pointer bound to the screenshot's output, on a Wayland connection of its own to the display in `WAYLAND_DISPLAY`. The server first checks that the process serving that display is the niri at `NIRI_SOCKET`, and fails with `upstream_error` otherwise. The result has `accepted: true` and `observed: sent` once niri has handled the input, with `focused_window` as niri's event stream showed it just after. What the input did is for the next screenshot to show. If the Wayland connection fails before anything reached it, the call fails with that error. If it breaks or niri stops answering after something was sent, `observed` is `uncertain` with `accepted` null and a `detail`, and the result comes with a screenshot.

`click` and `drag` write the input-dirty marker before they send anything, and remove it once niri has handled the button's release. If the call is cancelled midway, by a stop or by the client, the server releases the button on the way out and removes the marker once niri has handled the release, so other calls may see `recovery_required` for a moment. If the release can't be sent or the marker can't be removed, the marker stays, and every action refuses with `recovery_required` until the user runs `niri-computer-use recover`.

### `pointer_move`

| Argument | Value |
|---|---|
| `screenshot_ref` (required) | from a screenshot under this lease |
| `x`, `y` (required) | the pixel in that image |

Moves the pointer there, to hover.

### `click`

| Argument | Value |
|---|---|
| `screenshot_ref`, `x`, `y` (required) | as for `pointer_move` |
| `button` | `left` (default), `right` or `middle` |
| `count` | 1 (default) to 3; 2 is a double click |

Moves to the pixel, then presses and releases the button `count` times.

### `drag`

| Argument | Value |
|---|---|
| `screenshot_ref` (required) | from a screenshot under this lease |
| `from`, `to` (required) | `{"x", "y"}` pixels in that image |
| `button` | `left` (default), `right` or `middle` |

Moves to `from`, waits 50 ms, presses the button, waits 50 ms, moves to `to` in ten even steps 20 ms apart, and releases the button there.

### `scroll`

| Argument | Value |
|---|---|
| `screenshot_ref`, `x`, `y` (required) | as for `pointer_move` |
| `notches_y` | wheel notches down, negative for up; default 0 |
| `notches_x` | wheel notches right, negative for left; default 0 |

Moves to the pixel and turns the wheel by whole notches, the vertical axis first: for each axis, one frame of `axis_discrete` with 15 per notch, as niri's own wheel uses, then `axis_source` wheel. Apps scroll by their own amount per notch.

## Keyboard tools

`key` types into the app with keyboard focus through one `wtype` call, and `type_text` through one `wtype` call per part of 100 characters. The keys go to the app, not to niri: niri's own keybinds don't fire from them. They run through the same gate as the other action tools, then check, before anything is typed:

1. the arguments: `type_text` refuses over 1000 characters, counted as Unicode scalar values, with `text_too_long`, and an empty text or a combination it can't read is an argument mistake
2. `expect` against the window with keyboard focus: `focus_mismatch` when it doesn't match. A lock screen, the overview or a shell panel leaves no window focused, so only `"none"` types there
3. the focused window: `app_denied` if its `app_id` is on the policy's deny list

`expect` is required, so the agent says where it means to type:

| `expect` | Means |
|---|---|
| `{"window_id": <id>}` | that window, from `desktop_state`, must have keyboard focus |
| `{"app_id": "<app_id>"}` | the focused window must have that `app_id` |
| `"none"` | no check, for example to type into a shell panel or a dialog that holds focus outside the windows |

The result has `accepted: true`, `focus` (`matched`, or `unchecked` for `"none"`), and `observed`: `sent` once `wtype` has exited, or `interrupted` if keyboard focus moved off the window that had it at any time during the call; an interrupted result comes with a screenshot. If niri's event stream was lost meanwhile, `observed` is `uncertain` with `accepted: true`: the keys went out, but where focus went is unknown. What the keys did is for the next screenshot to show.

Before `wtype` starts, the server writes the input-dirty marker (`pending`), and once `wtype` runs it adds its PID and start time (`running`). `wtype` starts with `-`, so it waits at its stdin until the server has recorded it. The server removes the marker when `wtype` exits by itself. A stop or a cancelled request ends the call with `stopped` or a cancellation, but a `wtype` already running keeps typing and then removes the marker. A `wtype` still running three seconds after it started is killed with its process group, and one killed by a signal, the same way: the marker stays and every action refuses with `recovery_required` until the user runs `niri-computer-use recover`. The text is never logged: the audit log has `text_len`.

### `key`

| Argument | Value |
|---|---|
| `combo` (required) | modifiers and one key joined by `+`, such as `ctrl+s`, `ctrl+shift+t`, `alt+F4` or `Return`. The key is an XKB keysym name (`a`, `Return`, `Escape`, `F5`, `slash`, `Page_Down`); the modifiers are `shift`, `ctrl`, `alt`, `altgr` and `super` |
| `expect` (required) | see above |

Presses the modifiers, presses and releases the key, then releases the modifiers in reverse order.

### `type_text`

| Argument | Value |
|---|---|
| `text` (required) | 1 to 1000 characters |
| `expect` (required) | see above |

The text goes out in parts of 100 characters, one `wtype` call each, and after each part the server checks that keyboard focus is still on the window it started on. If focus moved, the rest isn't typed: `observed` is `interrupted` and `typed` gives the number of characters sent. A part that fails ends the call with that error, its `detail` starting with how many characters were typed before it. A result without `typed` means the whole text went out.
