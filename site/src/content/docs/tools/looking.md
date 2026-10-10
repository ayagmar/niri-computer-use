---
title: Looking
description: "The read-only tools: status, desktop_state, outputs, clipboard_read and wait_for."
sidebar:
  order: 2
---

These tools change nothing and need no lease. Any connected agent can call them, even while the screen is locked, so they can show it what is on your screen. Screenshots have [their own page](../screenshots/), and accessible elements [theirs](../elements/).

## `status`

No arguments. The readiness report, also printed by `niri-computer-use status`:

| Field | Value |
|---|---|
| `instance` | the basename of niri's socket, which names the niri instance |
| `discovery.runtime_dir`, `discovery.niri_socket`, `discovery.wayland_display` | where each came from: `{"source": "environment"}`, `{"source": "discovered"}`, or `{"source": "missing", "detail": …}` with why (see [Session variables](../../start/clients/#session-variables)) |
| `discovery.warning` | null, or why the given variables might not fit together, such as `NIRI_SOCKET` outside `XDG_RUNTIME_DIR` |
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
| `accessibility.available`, `accessibility.address`, `accessibility.reason` | whether the session has an accessibility bus, its address, and why not; `elements` is listed only when it has one |
| `policy.state` | `loaded`, `missing` (valid: no presets, no denied apps) or `invalid` |
| `policy.presets`, `policy.denied_app_ids`, `policy.error` | how many presets and denied apps the policy file has, and why it is invalid |
| `policy.preset_names` | the preset names `launch` takes |
| `policy.capture_dir` | where `screenshot` saves, as the file writes it, or null when saving is off |
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

## `clipboard_read`

No arguments. Runs `wl-paste --no-newline --type text` and returns `{"text": ..., "reason": null}`, or `text: null` with `reason` `nothing_copied` or `no_text`. Text over 1 MiB or not valid UTF-8 is an `upstream_error`. The text is never logged.

## `wait_for`

Waits for a window or the screen instead of taking screenshots until something happens.


| Argument | Value |
|---|---|
| `until` (required) | `{"window": {"app_id", "title"}}`: a window with that `app_id` and a title containing that text, either one optional but not both; `{"closed": <id>}`: that window is gone; `{"title": {"window_id", "contains"}}`: that window's title contains the text; or `"screen_stable"`: the focused output stopped changing |
| `timeout_ms` | 100 to 30000; default 10000 |
| `screenshot` | with true, the result comes with a screenshot of the focused output: for `screen_stable` its last capture, otherwise one taken as for an action's `screenshot: true` once the wait ended |

Read-only, and needs no lease. The window conditions follow niri's event stream; a condition that is already true ends the wait at once. `screen_stable` first waits for a running action of this server to end, then captures the focused output every 100 ms until two captures in a row are the same image. Two captures can't match before the second one starts, about 150 ms in, and large images take longer, so a short `timeout_ms` can end in `timeout` on a still screen. Stable means two images matched, not that an app is ready. A `title` condition on a window that doesn't exist, an empty `contains`, or a `window` without `app_id` and `title` is an argument mistake.

The result has `observed`: `met`, `timeout`, or `uncertain` with a `detail` if niri's event stream was lost; `windows`, the ids that met the condition; `focused_window` for the window conditions; and `waited_ms`. Window titles are never logged: the audit log keeps their length.
