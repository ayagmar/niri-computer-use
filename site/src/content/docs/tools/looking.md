---
title: Looking
description: "The read-only tools: status, desktop_state, outputs, clipboard_read and wait_for."
sidebar:
  order: 2
---

These tools change nothing, and all but `clipboard_read` need no lease. Any connected agent can call the others, even while the screen is locked, so they can show it what is on your screen. Screenshots have [their own page](../screenshots/), and accessible elements [theirs](../elements/).

## `status`

No arguments. The readiness report, also printed by `niri-computer-use status`:

| Field | Value |
|---|---|
| `instance` | the basename of niri's socket, which names the niri instance |
| `discovery.runtime_dir`, `discovery.niri_socket`, `discovery.wayland_display` | where each came from: `{"source": "environment"}`, `{"source": "discovered"}`, or `{"source": "missing", "detail": …}` with why (see [Session variables](../../start/clients/#session-variables)) |
| `engine.pid`, `engine.mode` | the process serving this client, and how: `standalone` serves one client over stdin and stdout, `shared` is the instance's [shared engine](../../concepts/configuration/#shared); `engine` is null from the `status` subcommand |
| `engine.sessions` | how many clients that process serves, this one included |
| `engine.fallback` | why a client in shared mode is served standalone, such as a `NIRI_COMPUTER_USE_SHARED` other than `1` or `0`, or null |
| `engine.version`, `engine.exe` | that process's version and the file it was started from |
| `engine.replaced` | `true` once `engine.exe` holds another build, as after a reinstall: the process still runs the build it started with until its clients restart |
| `engine.cgroup` | that process's cgroup; a shared engine has a scope of its own when its bridge could reach the user's systemd |
| `engine.unscoped` | why a shared engine has no scope of its own: what `systemd-run` said when it failed, or that it hung and was killed; null otherwise |
| `display_error` | null when the Wayland display is niri's right now, checked on every call; otherwise why input, screenshots and clipboard reads are refused, such as [`session_mismatch`](../../reference/errors/#session_mismatch) |
| `discovery.warning` | null, or why the given variables might not fit together, such as `NIRI_SOCKET` outside `XDG_RUNTIME_DIR` |
| `niri.version` | niri's version string, such as `26.04 (8ed0da4)` |
| `niri.ipc_crate` | the `niri-ipc` version this build uses, `26.4.0` |
| `niri.compat` | `ok` when the major and minor versions match, `patch_warning` when only the patch differs, `read_only` otherwise |
| `niri.event_stream` | `connected`, `disconnected`, or `schema_incompatible` after niri sent two events this build can't parse; null from the `status` subcommand, which opens no stream |
| `niri.error` | why niri's version couldn't be read, or null |
| `lease.held_by_me` | whether this server holds the lease |
| `lease.holder` | the holder's `pid`, `label` (client name and server PID, such as `claude-code/4711`) and `since`, or null |
| `lease.error` | null, or why no lease can be taken for this niri at all, such as a `NIRI_SOCKET` with a hard link; `acquire_desktop` fails with the same reason |
| `input_dirty` | the input-dirty marker, or null: its `operation`, `phase` (`pending` or `running`), `server_pid`, `since`, the input `child` once known, and any pointer `buttons` pressed; `{"error": …}` if the marker can't be read |
| `stop` | whether the stop flag is set for this niri instance (`niri-computer-use stop`, cleared by `niri-computer-use resume`) |
| `lock.state` | `locked`, `unlocked` or `unknown` |
| `lock.source` | `logind`, `noctalia` or `none`: the screen counts as locked when logind's `LockedHint` or Noctalia's `locked` says so |
| `lock.session` | the logind session asked: the one in niri's own `XDG_SESSION_ID`, which is where niri sets the hint |
| `lock.logind_error` | why logind couldn't answer, or null |
| `outputs.pointer_supported`, `outputs.reason` | whether niri's outputs are a setup the virtual pointer is tested on (one enabled output: a monitor at transform `Normal`, or nested niri's `winit` window), and why not |
| `noctalia` | `running`, `not_running` or `not_installed` |
| `noctalia_error` | why Noctalia counts as not running, or null |
| `accessibility.available`, `accessibility.address`, `accessibility.reason` | whether the session has an accessibility bus, its address, and why not; `elements`, `activate_element` and `set_element_text` are listed only when it has one |
| `policy.state` | `loaded`, `missing` (valid: no presets, no denied apps) or `invalid` |
| `policy.presets`, `policy.denied_app_ids`, `policy.error` | how many presets and denied apps the policy file has, and why it is invalid |
| `policy.preset_names` | the preset names `launch` takes |
| `policy.capture_dir` | where `screenshot` saves, as the file writes it, or null when saving is off |
| `unrestricted.enabled`, `unrestricted.source`, `unrestricted.error` | whether gated `niri_action`s and unrestricted presets are allowed, whether the policy file, `NIRI_COMPUTER_USE_UNRESTRICTED` or both allowed them, and why the variable was ignored |
| `confirm_lease.enabled`, `confirm_lease.source`, `confirm_lease.error` | whether `acquire_desktop` waits for the user to allow it on the desktop, whether the policy file, `NIRI_COMPUTER_USE_CONFIRM_LEASE` or both asked for that, and why the variable was ignored |
| `audit.path`, `audit.last_error` | the audit log and the last failure to write it |
| `binaries` | whether `grim`, `notify-send` and `wtype` are on this client's `PATH` |
| `desktop_programs` | the directories searched for `loginctl` and `noctalia` at startup (`search`), and the path found for each, or null |

## `desktop_state`

No arguments. One snapshot of niri's event stream:

- `windows`, by id: niri's window objects, with `id`, `title`, `app_id`, `pid`, `workspace_id`, `is_focused`, `is_floating`, `is_urgent` and `layout`
- `workspaces`, by output and index
- `focused_window`: the focused window's id, or null while keyboard focus is outside the window layout, for example on a shell panel, the lock screen or the overview
- `overview_open`
- `keyboard_layouts`: the layout names and the current index
- `outputs`, by connector name, such as `DP-1`: each output's `name`, `make` and `model`, and under `logical` its position and size in layout coordinates, its scale and its transform. A disabled output has `logical: null`. niri's mode lists and hardware details are left out.
- `shell`, only when Noctalia is installed: Noctalia's own status, `barVisible`, `panelOpen`, `activePanelId` and `locked`, or `{"error": "noctalia_unavailable", "detail": ...}` when Noctalia isn't answering. Its absence or error doesn't fail the call.

Right after the server starts or reconnects, it waits up to two seconds for niri's initial state. The outputs come from a separate niri request and Noctalia's status from its socket, both asked at the same time as the snapshot, each with a two-second deadline.

## `clipboard_read`

No arguments. Needs the lease, and refuses with [`lease_required`](../../reference/errors/#lease_required) without it. Returns the clipboard's text only if it was copied while this lease was held, such as a link an app's "Copy link" button put there after the agent pressed it: text that never appears on screen. Returns `{"text": ..., "reason": null}`, or `text: null` with a `reason`:

- `copied_before_lease`: the clipboard still holds what was there when the lease was taken, or nothing was copied since. Your earlier clipboard is never read.
- `nothing_copied`: something copied during the lease was cleared, and the clipboard is empty.
- `no_text`: nothing copied is text.
- `secret`: the clipboard's owner marked it `x-kde-passwordManagerHint: secret`, as KeePassXC does for a copied password. Its text is never read. Text that a paste left on the clipboard because it couldn't put the user's copy back carries the same mark, so it is `secret` too.

`acquire_desktop` starts a watcher on a Wayland connection of its own to niri, through wlr data-control, and returns once it is bound, so anything copied after `acquire_desktop` returns counts. The watcher notes every selection niri announces and ends within a quarter of a second of the lease's end: `release_desktop`, a stop, or the session's end. A copy you make yourself while an agent holds the lease counts as made during the lease. A paste puts your earlier clipboard back after its text; that restore still counts as copied before the lease, while a copy the agent made before a paste counts as its own once restored.

The hint and the text come from the same copy, so a copy made in between can't pass a secret off as plain text. The text is read as `wl-paste --type text` reads it: as `text/plain;charset=utf-8` if offered, else `text/plain`, else the first other text type. Text over 1 MiB or not valid UTF-8 is an `upstream_error`. If the watcher couldn't bind or lost its connection, reads fail with its error, such as `upstream_error` with niri's message, until the lease is released and taken again. The text is never logged.

## `wait_for`

Waits for a window or the screen instead of taking screenshots until something happens.


| Argument | Value |
|---|---|
| `until` (required) | `{"window": {"app_id", "title"}}`: a window with that `app_id` and a title containing that text, either one optional but not both; `{"closed": <id>}`: that window is gone; `{"title": {"window_id", "contains"}}`: that window's title contains the text; or `"screen_stable"`: the focused output stopped changing |
| `timeout_ms` | 100 to 30000; default 10000 |
| `screenshot` | with true, the result comes with a screenshot of the focused output: for `screen_stable` its last capture, otherwise one taken as for an action's `screenshot: true` once the wait ended |

Read-only, and needs no lease. The window conditions follow niri's event stream; a condition that is already true ends the wait at once. `screen_stable` first waits for a running action of this server to end, then captures the focused output every 100 ms until two captures in a row are the same image. Two captures can't match before the second one starts, about 150 ms in, and large images take longer, so a short `timeout_ms` can end in `timeout` on a still screen. Stable means two images matched, not that an app is ready. A `title` condition on a window that doesn't exist, an empty `contains`, or a `window` without `app_id` and `title` is an argument mistake.

The result has `observed`: `met`, `timeout`, or `uncertain` with a `detail` if niri's event stream was lost; `windows`, the ids that met the condition; `focused_window` for the window conditions; and `waited_ms`. Window titles and app ids are never logged: the audit log keeps their length.
