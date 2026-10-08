---
title: Tools reference
description: Every tool niri-computer-use offers, with its arguments, results and errors.
---

All tools are read-only and carry the `readOnlyHint` annotation. Each successful result has the data as `structuredContent` and the same JSON as text.

## Errors

A failure sets `isError` and returns `{"error": <name>, "detail": <upstream detail>}`:

| Name | Meaning |
|---|---|
| `niri_unavailable` | niri's socket is missing, refused the connection, or closed it |
| `deadline_exceeded` | niri or a program didn't answer in time: two seconds for niri and `wl-paste`, five for `grim` |
| `upstream_error` | niri, a program or Noctalia answered with an error, or with something unreadable; `detail` keeps its message, exit status and stderr |
| `noctalia_unavailable` | Noctalia is installed but didn't answer on its socket within two seconds |

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
| `lock.state` | `locked`, `unlocked` or `unknown` |
| `lock.source` | `logind`, `noctalia` or `none`: the screen counts as locked when logind's `LockedHint` or Noctalia's `locked` says so |
| `lock.logind_error` | why logind couldn't answer, or null |
| `noctalia` | `running`, `not_running` or `not_installed` |
| `noctalia_error` | why Noctalia counts as not running, or null |
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
