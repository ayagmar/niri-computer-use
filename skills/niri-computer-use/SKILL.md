---
name: niri-computer-use
description: "Look at a niri Wayland desktop through the niri-computer-use MCP server: windows, workspaces, outputs, screenshots, the clipboard's text and Noctalia's panels, and take the lease that controlling the desktop will need."
license: MIT
compatibility: Needs the niri-computer-use MCP server (niri-computer-use serve) registered in the agent, running inside a niri 26.04 session.
---

# niri desktop

The `niri-computer-use` MCP server shows you the user's niri desktop. It can't act on it yet: there are no tools to click, type, focus or launch. It has the lease that those tools will require: one agent at a time holds it, and the user can take it back at any moment.

## Rules

1. Use only the `niri-computer-use` MCP tools to inspect the desktop. Never run `grim`, `niri msg`, `noctalia msg` or `wl-paste` yourself.
2. Start with `status`. Note whether Noctalia is running (`noctalia`), whether the screen is locked (`lock.state`), and whether niri's event stream is connected (`niri.event_stream`). If `niri.error` says `NIRI_SOCKET is not set`, the agent started the server without the niri session's environment. Tell the user instead of retrying.
3. Prefer structured data to screenshots:
   - `desktop_state` for windows, workspaces, the focused window and the overview
   - `outputs` for the monitors' layout, scale and transform
   - `shell_status` for Noctalia's bar, open panel and lock screen (listed only when Noctalia is installed)
   - `clipboard_read` for the clipboard's text
4. Take a `screenshot` when you need to see pixels. It returns a JPEG of at most 1280 pixels wide by default. To read small text, take a `region` screenshot around it instead of guessing; `max_width` can go higher when the region is wider than 1280 pixels.
5. Report what you observed separately from what you infer.
6. Take the lease with `acquire_desktop` only when the user asks you to control the desktop, and give it back with `release_desktop` when you are done. Watching the desktop never needs it.
7. If a tool returns `stopped`, `recovery_required`, `screen_locked`, `lease_held` or `read_only`, stop and tell the user, with the detail. Never try to clear a stop, recover, unlock or take the lease from another agent yourself: `resume` and `recover` are the user's commands, and you must not run them.

## Tools

| Tool | Arguments | Returns |
|---|---|---|
| `status` | none | readiness: niri's version and event stream, who holds the lease, the stop flag, the input-dirty marker, lock state, Noctalia, the policy file, audit log, programs on `PATH` |
| `desktop_state` | none | one snapshot: windows, workspaces, `focused_window`, `overview_open`, keyboard layouts |
| `outputs` | none | outputs by connector name, with logical position, size, scale and transform |
| `screenshot` | `target`, and optionally `region`, `max_width`, `format` | an image, then metadata: output, captured rectangle in layout coordinates, scale, image size |
| `clipboard_read` | none | `text`, or `text: null` with `reason` `nothing_copied` or `no_text` |
| `shell_status` | none | Noctalia's `barVisible`, `panelOpen`, `activePanelId` and `locked` |
| `acquire_desktop` | none | `holder`: your PID, label and since when; calling it again while you hold it returns the same holder |
| `release_desktop` | none | `released`: whether you held it; the user's stop also takes it back |

`screenshot` targets:

- `focused_output`: the output with keyboard focus
- `output:<name>`: an output by its name from `outputs`, such as `output:DP-1`
- `region`, with `region: {x, y, width, height}` in layout coordinates (the coordinates `outputs` uses). The rectangle must lie inside one output.

`format` is `jpeg` (the default) or `png`.

## Errors

- A mistake in the arguments, such as an unknown output, comes back with `isError` and a plain-text message starting `invalid arguments:`. Fix the arguments and call again.
- Other failures come back with `isError` and `{"error": <name>, "detail": <upstream detail>}`: `niri_unavailable`, `deadline_exceeded`, `upstream_error` or `noctalia_unavailable`. Report the name and detail to the user. Don't loop on the same call.
- `acquire_desktop` refuses with `lease_held` (another agent has it), `stopped` (the user pressed the stop key or ran `niri-computer-use stop`), `recovery_required` (input may be stuck), `screen_locked` (locked, or nobody can say it isn't) or `read_only` (unsupported niri, or an invalid policy file). Rule 7 applies to all of them.
- `focused_window` is null while keyboard focus is outside the window layout, for example on a shell panel, the lock screen or the overview. That's information, not an error.
