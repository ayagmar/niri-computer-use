---
title: Tools reference
description: Every tool niri-computer-use offers, with its arguments, results and errors.
---

The perception tools are read-only and carry the `readOnlyHint` annotation. The two lease tools change only the lease, never the desktop. The four action tools change the desktop through niri's IPC and need the lease; `close_window` carries `destructiveHint`. Each successful result has the data as `structuredContent` and the same JSON as text.

## Errors

A failure sets `isError` and returns `{"error": <name>, "detail": <upstream detail>}`:

| Name | Meaning |
|---|---|
| `niri_unavailable` | niri's socket is missing, refused the connection, or closed it |
| `deadline_exceeded` | niri or a program didn't answer in time: two seconds for niri and `wl-paste`, five for `grim` |
| `upstream_error` | niri, a program or Noctalia answered with an error, or with something unreadable; `detail` keeps its message, exit status and stderr |
| `noctalia_unavailable` | Noctalia is installed but didn't answer on its socket within two seconds |
| `lease_held` | another agent's server holds the lease; `detail` names its PID, label and since when |
| `lease_required` | an action tool was called without holding the lease |
| `stopped` | the stop flag is set, or was set while an action ran and cancelled it; the user clears it with `niri-computer-use resume` |
| `read_only` | this build doesn't support the running niri, niri sent events it can't parse, or the policy file is invalid |
| `screen_locked` | the screen is locked, or neither logind nor Noctalia can say whether it is |
| `recovery_required` | input may be stuck; `detail` names the marker's operation and phase, and the user runs `niri-computer-use recover` |
| `unknown_preset` | `launch` named a preset the policy file doesn't have; `detail` lists the names it has |

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

The result is an image block, then the metadata as text and as `structuredContent`:

| Field | Value |
|---|---|
| `output`, `transform` | the captured output and its transform |
| `output_origin` | the output's top-left corner in layout coordinates |
| `captured` | the captured rectangle in layout coordinates |
| `scale` | image pixels per logical pixel |
| `width`, `height`, `mime_type` | the image, checked against its own header |
| `captured_at_unix_ms`, `capture_ms` | when the capture started and how long it took |

`grim` gets five seconds and at most 64 MiB of output.

## `clipboard_read`

No arguments. Runs `wl-paste --no-newline --type text` and returns `{"text": ..., "reason": null}`, or `text: null` with `reason` `nothing_copied` or `no_text`. Text over 1 MiB or not valid UTF-8 is an `upstream_error`.

## `shell_status`

No arguments. Listed only when `noctalia` is on `PATH`. Noctalia's own status reply: `barVisible`, `panelOpen`, `activePanelId` and `locked`. `noctalia_unavailable` when Noctalia doesn't answer; an `error:` reply from Noctalia is an `upstream_error` with its text.

## `acquire_desktop`

No arguments. Takes the lease on this niri instance and returns `{"holder": {"pid", "label", "since"}}`. One server holds it at a time; the action tools require it. Calling it again while holding the lease returns the same holder.

Refused with `lease_held` while another server holds it, `stopped` while the stop flag is set, `recovery_required` while the input-dirty marker exists, the niri error when niri's version can't be read, `read_only` when this build doesn't support the running niri, niri sent events it can't parse, or the policy file is invalid, and `screen_locked` while the screen is locked or its lock state is unknown. If the runtime directory can't be read, it fails with `upstream_error` rather than assume neither flag is set.

## `release_desktop`

No arguments. Gives the lease up and returns `{"released": true}`, or `{"released": false}` if this server didn't hold it. The lease is also given up when the stop flag appears and when the server exits.

## Action tools

`focus_window`, `focus_workspace`, `launch` and `close_window` act on the desktop through niri's IPC. Before each action the server checks, in this order, the stop flag (`stopped`), the input-dirty marker (`recovery_required`), the lease (`lease_required`), then niri's version, the policy file and the lock state, as `acquire_desktop` does. One action runs at a time; a call made meanwhile waits for it. A stop during an action cancels it with `stopped`, and anything niri had already accepted may have taken effect.

An unknown window or workspace id is an argument mistake, and nothing is sent. Otherwise the result has these fields:

| Field | Value |
|---|---|
| `accepted` | true once niri acknowledged the request; false when nothing was sent; null when the request was sent but niri's reply was lost |
| `observed` | what niri's event stream showed afterwards; the values are listed under each tool |
| `focused_window` | the window with keyboard focus when the observation ended, or null when focus isn't on a window or the reply was lost |
| `windows` | the windows the outcome is about, when there are any |
| `detail` | why the outcome is `uncertain` |

Every action waits up to five seconds for its effect. Two outcomes can end any of them:

- `interrupted`: focus moved to a window that was neither focused before nor the expected target, so someone else is using the desktop. `focused_window` says where focus went.
- `uncertain`: niri's reply or event stream was lost, so the action may or may not have happened. `accepted` is null when the reply was lost.

None of these outcomes is an error, and the server never retries an action.

## `focus_window`

| Argument | Value |
|---|---|
| `id` (required) | a window id from `desktop_state` |

`observed` is `focused` once the window has keyboard focus, or `timeout`. Idempotent: focusing the focused window is seen at once.

## `focus_workspace`

| Argument | Value |
|---|---|
| `id` (required) | a workspace id from `desktop_state`, not its index |

`observed` is `focused` once the workspace has focus, on whichever output it is, or `timeout`. Focus moving to one of the workspace's own windows is expected, not an interruption.

## `launch`

| Argument | Value |
|---|---|
| `preset` (required) | a preset name from the policy file, listed in `status` as `policy.preset_names` |
| `reuse` | default false; with true, focus the preset's existing window instead of starting another |

niri starts the preset's fixed `argv`; the app keeps running after the server exits. `observed` counts the windows with the preset's `app_id` that weren't open before, including one that sets its `app_id` after it appears. Once the first appears the server keeps counting for half a second, then reports `one` or `ambiguous`, with the ids in `windows`. With no such window in five seconds it reports `none`. Focus moving to a new window, before it has its `app_id`, isn't an interruption.

With `reuse`: one existing matching window is focused, and `observed` is `focused` with its id in `windows`; several give `ambiguous` with their ids and `accepted: false`, and nothing is started; none starts the preset as usual.

## `close_window`

| Argument | Value |
|---|---|
| `id` (required) | a window id from `desktop_state` |

Asks the window to close, as its close button would. `observed` is `closed` once niri reports it gone, or `pending` if it is still open after five seconds, for example behind an unsaved-changes dialog. Nothing forces it closed. `windows` holds the id.
