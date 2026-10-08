---
name: niri-computer-use
description: "See and act on a niri Wayland desktop through the niri-computer-use MCP server: windows, workspaces, outputs, screenshots, the clipboard's text and Noctalia's panels; with the lease, focus windows and workspaces, launch preset apps and close windows."
license: MIT
compatibility: Needs the niri-computer-use MCP server (niri-computer-use serve) registered in the agent, running inside a niri 26.04 session.
---

# niri desktop

The `niri-computer-use` MCP server shows you the user's niri desktop and, while you hold its lease, lets you focus windows and workspaces, start apps from the user's presets and close windows. It can't click or type yet. One agent at a time holds the lease, and the user can take it back at any moment with a stop key.

## Rules

1. Use only the `niri-computer-use` MCP tools on the desktop. Never run `grim`, `niri msg`, `noctalia msg`, `wl-paste` or `wtype` yourself.
2. Start with `status`. Note whether Noctalia is running (`noctalia`), whether the screen is locked (`lock.state`), whether niri's event stream is connected (`niri.event_stream`), and which presets exist (`policy.preset_names`). If `niri.error` says `NIRI_SOCKET is not set`, the agent started the server without the niri session's environment. Tell the user instead of retrying.
3. Prefer structured data to screenshots:
   - `desktop_state` for windows, workspaces, the focused window and the overview
   - `outputs` for the monitors' layout, scale and transform
   - `shell_status` for Noctalia's bar, open panel and lock screen (listed only when Noctalia is installed)
   - `clipboard_read` for the clipboard's text
4. Take a `screenshot` when you need to see pixels. It returns a JPEG of at most 1280 pixels wide by default. To read small text, take a `region` screenshot around it instead of guessing; `max_width` can go higher when the region is wider than 1280 pixels.
5. Report what you observed separately from what you infer.
6. Take the lease with `acquire_desktop` only when the user asks you to act on the desktop, then look at it (`desktop_state`, and a fresh `screenshot` when pixels matter) before the first action. Give the lease back with `release_desktop` when you are done. Watching the desktop never needs it.
7. Act in a loop: observe, one action, read `accepted` and `observed`, observe again. An action after which neither the structured state nor a new screenshot shows any change toward the goal is a no-progress attempt. After three in a row, stop and tell the user what you saw.
8. Use the structured actions: `focus_window` and `focus_workspace` with ids from `desktop_state`, `launch` with a preset name, `close_window`. `launch` starts only the user's presets; if the app you need has none, ask the user to add one rather than looking for another way to start it.
9. Use `launch` with `reuse: true` unless the user asked for another window of the app.
10. An outcome that isn't the one you wanted is information, not a failure to retry. After `timeout`, `none`, `pending` or `uncertain`, look at the desktop before doing anything else: the result already has a fresh screenshot of the focused output, so look at that before taking another. Never repeat a `launch` or a `close_window` on your own: a second launch opens a second app, and a second close can answer the app's unsaved-changes dialog.
11. `interrupted` means someone else moved focus while you waited. Stop and tell the user; don't continue the plan.
12. If a tool returns `stopped`, `recovery_required`, `screen_locked`, `lease_held` or `read_only`, stop and tell the user, with the detail. Never try to clear a stop, recover, unlock or take the lease from another agent yourself: `resume` and `recover` are the user's commands, and you must not run them.

## Tools

| Tool | Arguments | Returns |
|---|---|---|
| `status` | none | readiness: niri's version and event stream, who holds the lease, the stop flag, the input-dirty marker, lock state, Noctalia, the policy file and its preset names, audit log, programs on `PATH` |
| `desktop_state` | none | one snapshot: windows, workspaces, `focused_window`, `overview_open`, keyboard layouts |
| `outputs` | none | outputs by connector name, with logical position, size, scale and transform |
| `screenshot` | `target`, and optionally `region`, `max_width`, `format` | an image, then metadata: output, captured rectangle in layout coordinates, scale, image size |
| `clipboard_read` | none | `text`, or `text: null` with `reason` `nothing_copied` or `no_text` |
| `shell_status` | none | Noctalia's `barVisible`, `panelOpen`, `activePanelId` and `locked` |
| `acquire_desktop` | none | `holder`: your PID, label and since when; calling it again while you hold it returns the same holder |
| `release_desktop` | none | `released`: whether you held it; the user's stop also takes it back |
| `focus_window` | `id`: a window id | `observed`: `focused` or `timeout`; `accepted: false` when it already had focus |
| `focus_workspace` | `id`: a workspace id, not its index | `observed`: `focused` or `timeout`; `accepted: false` when it already had focus |
| `launch` | `preset`, optionally `reuse` | `observed`: `one`, `ambiguous` or `none`, with the new window ids in `windows`, or `focused` when a single-instance app showed the window it had; with `reuse`, `focused` for one existing window, or `ambiguous` with several and nothing started |
| `close_window` | `id`: a window id | `observed`: `closed`, or `pending` when the window is still open after five seconds, for example behind an unsaved-changes dialog |

Every action result also has `accepted` (true once niri took the request, false when nothing was sent, null when niri's reply was lost), `focused_window` when the observation ended, and possibly `interrupted` or `uncertain` as `observed` (rules 10 and 11). Each waits up to five seconds. With `timeout`, `pending`, `none`, `interrupted` or `uncertain`, the result also has an image of the focused output and its metadata in `screenshot`, or `screenshot_error` if it couldn't be taken.

`screenshot` targets:

- `focused_output`: the output with keyboard focus
- `output:<name>`: an output by its name from `outputs`, such as `output:DP-1`
- `region`, with `region: {x, y, width, height}` in layout coordinates (the coordinates `outputs` uses). The rectangle must lie inside one output.

`format` is `jpeg` (the default) or `png`.

## Errors

- A mistake in the arguments, such as an unknown output or a window id that doesn't exist, comes back with `isError` and a plain-text message starting `invalid arguments:`. Nothing was done. Fix the arguments and call again.
- Other failures come back with `isError` and `{"error": <name>, "detail": <upstream detail>}`: `niri_unavailable`, `deadline_exceeded`, `upstream_error` or `noctalia_unavailable`. Report the name and detail to the user. Don't loop on the same call.
- `lease_required`: you called an action without holding the lease. Call `acquire_desktop` if the user asked you to act.
- `unknown_preset`: `launch` named a preset that doesn't exist; the detail lists the ones that do.
- `acquire_desktop` and every action refuse with `stopped` (the user pressed the stop key or ran `niri-computer-use stop`; it also cancels a running action), `recovery_required` (input may be stuck), `screen_locked` (locked, or nobody can say it isn't) or `read_only` (unsupported niri, events this build can't parse, or an invalid policy file); `acquire_desktop` also refuses with `lease_held` (another agent has it). Rule 12 applies to all of them.
- `focused_window` is null while keyboard focus is outside the window layout, for example on a shell panel, the lock screen or the overview. That's information, not an error.
