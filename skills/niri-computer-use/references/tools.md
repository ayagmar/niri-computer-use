# niri-computer-use tools

## Contents

- Reading tools
- The lease
- Structured actions
- Pointer and keyboard
- Fields every action result has
- Screenshot targets

## Reading tools

| Tool | Arguments | Returns |
|---|---|---|
| `status` | none | readiness: niri's version and event stream, who holds the lease, the stop flag, the input-dirty marker, lock state, whether the outputs suit the pointer (`outputs.pointer_supported`), Noctalia, the policy file and its preset names, the audit log, programs on `PATH` |
| `desktop_state` | none | one snapshot: windows, workspaces, `focused_window`, `overview_open`, keyboard layouts |
| `outputs` | none | outputs by connector name, with logical position, size, scale and transform |
| `screenshot` | `target`, and optionally `region`, `max_width`, `format`, `save_path` | an image, then metadata: output, captured rectangle in layout coordinates, scale, image size, and `screenshot_ref` while you hold the lease; with `save_path`, `saved`: the PNG's path and pixel size |
| `clipboard_read` | none | `text`, or `text: null` with `reason` `nothing_copied` or `no_text` |
| `shell_status` | none | Noctalia's `barVisible`, `panelOpen`, `activePanelId` and `locked`. Listed only when Noctalia is installed |
| `wait_for` | `until`, optionally `timeout_ms` (100 to 30000, default 10000) and `screenshot` | `observed`: `met` with the matching `windows`, `timeout`, or `uncertain`; `focused_window`; `waited_ms` |

`until` is one of `{"window": {"app_id": …, "title": …}}` (a window with that `app_id` and a title containing that text; either one may be left out), `{"closed": <window id>}`, `{"title": {"window_id": …, "contains": …}}`, or `"screen_stable"` (two captures of the focused output 100 ms apart are the same).

If `status` shows `niri.error` "NIRI_SOCKET is not set", the agent started the server without the niri session's environment. Tell the user rather than retrying.

## The lease

| Tool | Arguments | Returns |
|---|---|---|
| `acquire_desktop` | none | `holder`: your PID, label and since when, and `users_window`, the window that had focus; calling it again while you hold it returns the same |
| `release_desktop` | `restore_focus` | `released`: whether you held it, and `users_window`. With `restore_focus: true`, focus first goes back to `users_window` and `restored` is that action's result, `observed: closed` if the window is gone. The user's stop also takes the lease back |

Watching the desktop never needs the lease.

## Structured actions

| Tool | Arguments | `observed` |
|---|---|---|
| `focus_window` | `id`: a window id | `focused` or `timeout`; `accepted: false` when it already had focus |
| `focus_workspace` | `id`: a workspace id, not its index | `focused` or `timeout`; `accepted: false` when it already had focus |
| `launch` | `preset`, optionally `reuse` | `one`, `ambiguous` or `none`, with the new window ids in `windows`, or `focused` when a single-instance app showed the window it had. With `reuse: true`: `focused` for one existing window, or `ambiguous` with several and nothing started |
| `close_window` | `id`: a window id | `closed`, or `pending` when the window is still open after five seconds, for example behind an unsaved-changes dialog |
| `shell_open` | `panel`: `control-center`, `wallpaper` or `tray-drawer` | `opened` or `timeout` within two seconds, with `shell.active_panel`; `accepted: false` when it was already open. Listed only with Noctalia |
| `shell_close` | `panel`, as for `shell_open` | `closed` or `timeout`; `accepted: false` when it wasn't open |

Use `launch` with `reuse: true` unless the user asked for another window of the app.

An open panel holds keyboard focus, so `focused_window` is null while it is open. To type into it, use `expect: "none"` after a screenshot shows it ready, and close it with `shell_close` when you're done.

## Pointer and keyboard

| Tool | Arguments | `observed` |
|---|---|---|
| `pointer_move` | `screenshot_ref`, `x`, `y` | `sent`; moves the pointer there, to hover |
| `click` | `screenshot_ref`, `x`, `y`, optionally `button` (`left`, `right`, `middle`) and `count` (1 to 3) | `sent` |
| `drag` | `screenshot_ref`, `from: {x, y}`, `to: {x, y}`, optionally `button` | `sent`; presses at `from`, moves, releases at `to` |
| `scroll` | `screenshot_ref`, `x`, `y`, `notches_y` (positive is down) and/or `notches_x` (positive is right), at most 10 each | `sent` |
| `key` | `keys`, 1 to 16 combinations such as `["ctrl+s"]` or `["Down", "Down", "Return"]`, and `expect` | `sent` or `interrupted`; `focus`: `matched` or `unchecked`; if focus moves after a key the rest aren't pressed and `pressed` counts the ones that were |
| `type_text` | `text` (1 to 1000 characters), `expect`, optionally `submit` | as for `key`; sent in parts of 100, and if focus moves during a part the rest isn't typed: `interrupted`, with `typed` counting the characters sent. With `submit: true`, Enter is pressed after the whole text and `submitted` says whether it was |

The table's 100-character parts describe the default wtype backend. Experimental native input checks between individual key pairs. It types symbols the layout lacks through a call-long extended keymap, and refuses before typing when there are more distinct missing symbols than spare keys or a control character is missing. Native-only `keys` on click/drag/scroll is a list of up to five held modifiers (shift, ctrl, alt, altgr, super), not a list of combinations. Do not enable native yourself or work around a refusal. Both backend counts describe completed input, not application-confirmed delivery. No clipboard-preserving paste tool exists.

A combination uses keysym names (`a`, `Return`, `Escape`, `F5`, `slash`, `Page_Down`) and the modifiers `shift`, `ctrl`, `alt`, `altgr` and `super`.

Pixel coordinates are in the screenshot named by `screenshot_ref`, and a pixel targets its centre. A ref is good for 60 seconds and for the lease it was taken under.

## Fields every action result has

- `accepted`: true once niri or Noctalia took the request, false when nothing was sent (already in that state), null when the reply was lost.
- `observed`: what the server saw by the end of its wait, as in the tables above, or `interrupted` (someone else moved focus) or `uncertain`.
- `focused_window` when the observation ended.
- `typed`, for `type_text` that stopped early: the characters sent; `pressed` likewise for `key`; `submitted` with `submit`.

Every action takes `screenshot: true`: the result then has an image of the focused output taken once the screen stopped changing (`settled` in its metadata says whether it did within 1.5 seconds), and its `screenshot_ref` works for the pointer tools.

Each action waits up to five seconds, the shell tools two. With `timeout`, `pending`, `none`, `interrupted` or `uncertain`, the result has an image of the focused output with its metadata in `screenshot` even without asking, or `screenshot_error` if it couldn't be taken.

## Screenshot targets

- `focused_output`: the output with keyboard focus.
- `output:<name>`: an output by name from `outputs`, such as `output:DP-1`.
- `region`, with `region: {x, y, width, height}` in layout coordinates (the coordinates `outputs` uses). The rectangle must lie inside one output.

Images are JPEG at most 1280 pixels wide by default. `max_width` can go higher for a region wider than that, and a region keeps the output's native scale when it fits. `format` is `jpeg` (the default) or `png`.

`save_path` writes a full-resolution PNG, whatever `max_width` is, to a new file relative to the user's `capture_dir`, such as `readme/editor.png`. It works only when the user set `capture_dir` in the policy file (`status` shows `policy.capture_dir`); otherwise it fails with `save_not_enabled`. The path must be relative, without `..`, and end in `.png`; its directories must exist; an existing file is never replaced, so pick a new name rather than retrying the same one.
