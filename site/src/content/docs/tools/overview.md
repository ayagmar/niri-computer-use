---
title: Tools overview
description: Every tool niri-computer-use offers, and the shapes of its results and errors.
sidebar:
  order: 1
---

The server offers up to 22 tools. `shell_panel` is listed only when Noctalia is installed, `noctalia` only when Noctalia is installed and [`unrestricted`](../../concepts/configuration/#unrestricted) is on, and `elements`, `activate_element` and `set_element_text` only when the session has an accessibility bus, so a session without Noctalia or an accessibility bus has 17. The [tool profile](../../concepts/configuration/#tools) can list fewer: `observe` lists only the tools that look, and `act` leaves out `niri_action`, `noctalia` and `shell_panel`.

## Every tool

| Tool | Lease | What it does | Page |
|---|---|---|---|
| `status` | no | the readiness report | [Looking](../looking/#status) |
| `desktop_state` | no | windows, workspaces, the focused window, the overview, keyboard layouts, monitors, Noctalia's state | [Looking](../looking/#desktop_state) |
| `clipboard_read` | yes | the clipboard's text, if copied during the lease | [Looking](../looking/#clipboard_read) |
| `wait_for` | no | waits for a window to appear, close or change title, an element to appear or go away, or the screen to settle | [Looking](../looking/#wait_for) |
| `screenshot` | no | an image of an output or a region, with its geometry; optionally saved as a PNG | [Screenshots](../screenshots/) |
| `elements` | no | one window's accessible elements and where they are | [Elements](../elements/) |
| `acquire_desktop` | takes it | takes the lease and notes the user's window | [Acting](../acting/#acquire_desktop) |
| `release_desktop` | gives it up | gives the lease up, optionally restoring focus | [Acting](../acting/#release_desktop) |
| `focus` | yes | focuses a window or a workspace by id | [Acting](../acting/#focus) |
| `launch` | yes | starts a preset from the policy file | [Acting](../acting/#launch) |
| `close_window` | yes | asks a window to close | [Acting](../acting/#close_window) |
| `niri_action` | yes | sends any niri action, such as fullscreen, floating or a window width | [Acting](../acting/#niri_action) |
| `pointer_move` | yes | moves the pointer to a pixel or an element | [Acting](../acting/#pointer_move) |
| `click` | yes | clicks a pixel or an element | [Acting](../acting/#click) |
| `drag` | yes | drags from one point to another | [Acting](../acting/#drag) |
| `scroll` | yes | turns the wheel at a pixel | [Acting](../acting/#scroll) |
| `key` | yes | presses up to 16 key combinations | [Keyboard](../keyboard/#key) |
| `type_text` | yes | types up to 1000 characters, optionally pressing Enter, or pastes up to 1 MiB through the clipboard and restores it | [Keyboard](../keyboard/#type_text) |
| `activate_element` | yes | does an accessible element's own action, such as pressing a button | [Elements](../elements/#activate_element) |
| `set_element_text` | yes | replaces an editable field's text through the accessibility bus | [Elements](../elements/#set_element_text) |
| `shell_panel` | yes | opens or closes one of three Noctalia panels | [Noctalia](../../concepts/noctalia/#panels) |
| `noctalia` | yes | sends any Noctalia command; only with `unrestricted` | [Noctalia](../../concepts/noctalia/#any-command) |

The reading tools carry MCP's `readOnlyHint`, and `close_window`, `niri_action`, `click`, `drag`, `key`, `type_text`, `activate_element`, `set_element_text` and `noctalia` carry `destructiveHint`. Every tool has a `title` and `openWorldHint: false`, since each acts only on this desktop. `status`, `desktop_state` and `screenshot` carry `_meta["anthropic/alwaysLoad"]: true`, so clients that load tools only when a search finds them, such as Claude Code, load these three up front.

## Results

A successful call returns its data as JSON text, and the same JSON as `structuredContent` unless the result has an image or is `elements`' compact listing. Claude Code and Codex show the model a result's `structuredContent` in place of its text, and Codex drops its images too, so those results go without it. `elements`' text is one line per element by default; `format: "json"` gives the JSON as text and as `structuredContent`. `screenshot` returns an image block first, and an action with `screenshot: true` returns one after its text.

Actions report what happened in two fields:

- `accepted`: whether niri, or Noctalia, took the request. False when nothing was sent, null when the request went out but the reply was lost.
- `observed`: what niri's event stream, or Noctalia's status, showed afterwards, such as `focused`, `closed` or `opened`. For input, `sent` once niri has handled it.

`changes` says what niri saw the action change: windows opened and closed, focus and the focused workspace moving, and windows whose title, size, workspace, floating or urgency changed. It is left out when nothing changed, which means niri saw nothing, not that the app did nothing. With `element_changes`, it says what changed among a window's accessible elements too. See [Action tools](../acting/#action-tools).

`interrupted` means focus moved to a window the action wasn't about, so someone else is using the desktop. `uncertain` means a reply or the event stream was lost, so the action may or may not have happened. Neither is an error, and the server never retries. When an outcome is in doubt, the result comes with a fresh screenshot; any action asked with `screenshot: true` comes with one taken once the screen stopped changing.

## Errors

A failure sets `isError` and returns:

```json
{"error": "screen_locked", "detail": "…"}
```

`error` is a stable name from the [error reference](../../reference/errors/), and `detail` keeps the upstream message: niri's or Noctalia's reply, a program's exit status and stderr.

A mistake in the arguments, such as an unknown output, a window id that doesn't exist or a value of the wrong type, comes back with `isError` and one plain-text block, without `structuredContent`. A value that doesn't fit the tool's schema starts `failed to deserialize parameters:`; one the server checks against the desktop, such as an unknown output, starts `invalid arguments:`. Nothing was done; the agent corrects the call.

## The audit log

Each call that reaches a tool is appended to the [audit log](../../concepts/safety/#the-audit-log) with its argument metadata and outcome; a call to an unlisted tool or with arguments that don't match the schema isn't. Typed and pasted text, clipboard contents, window titles, accessible names and images are never written; the log keeps their length. Other strings, such as a preset, a role, a ref, key names or an `expect` app id, are written only when they name something the server knows, and otherwise as a category and their length. Of an action's `changes`, it keeps only counts: windows opened, closed and changed, whether focus and the workspace moved, and the element changes' sizes.
