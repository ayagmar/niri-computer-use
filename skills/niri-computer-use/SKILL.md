---
name: niri-computer-use
description: "See and act on a niri Wayland desktop through the niri-computer-use MCP server: windows, workspaces, outputs, screenshots, the clipboard's text and Noctalia's panels; with the lease, focus windows and workspaces, launch preset apps, close windows, click, drag, scroll and type."
license: MIT
compatibility: Needs the niri-computer-use MCP server (niri-computer-use serve) registered in the agent, running inside a niri 26.04 session.
---

# niri desktop

The `niri-computer-use` MCP server shows you the user's niri desktop and, while you hold its lease, lets you focus windows and workspaces, start apps from the user's presets, close windows, use the pointer on pixels of a screenshot, and type into the focused app. One agent at a time holds the lease, and the user can take it back at any moment with a stop key.

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
8. Prefer the structured actions to input: `focus_window` and `focus_workspace` with ids from `desktop_state`, `launch` with a preset name, `close_window`. `launch` starts only the user's presets; if the app you need has none, ask the user to add one rather than looking for another way to start it. Use the pointer and keyboard tools for what happens inside an app. `key` is for the app's own shortcuts only: niri's keybinds don't fire from it, so desktop actions always use the structured tools.
9. Use `launch` with `reuse: true` unless the user asked for another window of the app.
10. An outcome that isn't the one you wanted is information, not a failure to retry. After `timeout`, `none`, `pending` or `uncertain`, look at the desktop before doing anything else: the result already has a fresh screenshot of the focused output, so look at that before taking another. Never repeat a `launch` or a `close_window` on your own: a second launch opens a second app, and a second close can answer the app's unsaved-changes dialog.
11. `interrupted` means someone else moved focus while you waited. Stop and tell the user; don't continue the plan.
12. If a tool returns `stopped`, `recovery_required`, `screen_locked`, `lease_held`, `read_only`, `app_denied` or `untested_output_config`, stop and tell the user, with the detail. Never try to clear a stop, recover, unlock or take the lease from another agent yourself: `resume` and `recover` are the user's commands, and you must not run them.
13. The pointer tools take a `screenshot_ref` and pixel coordinates in that image. Use the ref of your latest screenshot: take a new one after every action that may have changed the screen, and whenever a pointer tool says `ref_invalid`. A ref is only good for 60 seconds and for the lease it was taken under.
14. Keyboard tools always pass `expect`. Use `{"window_id": …}` or `{"app_id": …}` whenever you type into a window; check `desktop_state` first. Use `"none"` only for a shell panel or dialog that holds keyboard focus, after a screenshot shows it is ready. `focus_mismatch` means focus isn't where you think: look again before typing.
15. `type_text` takes at most 100 characters; split longer text into several calls, checking a screenshot between them when it matters. Never type a password or secret unless the user gave it to you for that purpose.

## Tools

| Tool | Arguments | Returns |
|---|---|---|
| `status` | none | readiness: niri's version and event stream, who holds the lease, the stop flag, the input-dirty marker, lock state, whether the outputs suit the pointer (`outputs.pointer_supported`), Noctalia, the policy file and its preset names, audit log, programs on `PATH` |
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
| `pointer_move` | `screenshot_ref`, `x`, `y` | `observed`: `sent`; moves the pointer there, to hover |
| `click` | `screenshot_ref`, `x`, `y`, optionally `button` (`left`, `right`, `middle`) and `count` (1 to 3) | `observed`: `sent` |
| `drag` | `screenshot_ref`, `from: {x, y}`, `to: {x, y}`, optionally `button` | `observed`: `sent`; presses at `from`, moves, releases at `to` |
| `scroll` | `screenshot_ref`, `x`, `y`, `notches_y` (positive is down) and/or `notches_x` (positive is right), at most 10 each | `observed`: `sent` |
| `key` | `combo` such as `ctrl+s`, `ctrl+shift+t`, `Return` or `alt+F4` (keysym names: `a`, `Return`, `Escape`, `F5`, `slash`, `Page_Down`; modifiers `shift`, `ctrl`, `alt`, `altgr`, `super`), `expect` | `observed`: `sent` or `interrupted`; `focus`: `matched` or `unchecked` |
| `type_text` | `text` (1 to 100 characters), `expect` | as for `key` |

`sent` means niri received the input; it says nothing about what the app did with it, so take a screenshot to see. `screenshot` returns a `screenshot_ref` while you hold the lease. A pixel targets its centre.

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
- `ref_invalid`: the detail starts with `unknown_ref`, `expired`, `output_changed` or `out_of_bounds`. Take a new screenshot and aim again from it; for `out_of_bounds`, use a pixel inside that image.
- `focus_mismatch`: the window you named in `expect` doesn't have keyboard focus. Look at `desktop_state` and a screenshot, focus the right window with `focus_window` if that is what you meant, then type.
- `text_too_long`: split the text into calls of at most 100 characters.
- `app_denied`: the user denied input to the focused app. Stop and tell the user (rule 12).
- `untested_output_config`: the pointer only runs on one monitor at transform `Normal` (or nested niri). Stop and tell the user (rule 12).
- `acquire_desktop` and every action refuse with `stopped` (the user pressed the stop key or ran `niri-computer-use stop`; it also cancels a running action), `recovery_required` (input may be stuck), `screen_locked` (locked, or nobody can say it isn't) or `read_only` (unsupported niri, events this build can't parse, or an invalid policy file); `acquire_desktop` also refuses with `lease_held` (another agent has it). Rule 12 applies to all of them.
- `focused_window` is null while keyboard focus is outside the window layout, for example on a shell panel, the lock screen or the overview. That's information, not an error.
