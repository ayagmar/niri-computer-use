---
title: Tools overview
description: Every tool niri-computer-use offers, and the shapes of its results and errors.
sidebar:
  order: 1
---

The server offers up to 25 tools. `shell_status`, `shell_open` and `shell_close` are listed only when Noctalia is installed, `noctalia` only when Noctalia is installed and [`unrestricted`](../../concepts/configuration/#unrestricted) is on, and `elements` only when the session has an accessibility bus, so a session without Noctalia or an accessibility bus has 20.

## Every tool

| Tool | Lease | What it does | Page |
|---|---|---|---|
| `status` | no | the readiness report | [Looking](../looking/#status) |
| `desktop_state` | no | windows, workspaces, the focused window, the overview, keyboard layouts | [Looking](../looking/#desktop_state) |
| `outputs` | no | monitors: position, size, scale, transform | [Looking](../looking/#outputs) |
| `clipboard_read` | no | the clipboard's text | [Looking](../looking/#clipboard_read) |
| `wait_for` | no | waits for a window to appear, close or change title, or for the screen to settle | [Looking](../looking/#wait_for) |
| `screenshot` | no | an image of an output or a region, with its geometry; optionally saved as a PNG | [Screenshots](../screenshots/) |
| `elements` | no | one window's accessible elements and where they are | [Elements](../elements/) |
| `shell_status` | no | Noctalia's bar, panel and lock state | [Noctalia](../../concepts/noctalia/) |
| `acquire_desktop` | takes it | takes the lease and notes the user's window | [Acting](../acting/#acquire_desktop) |
| `release_desktop` | gives it up | gives the lease up, optionally restoring focus | [Acting](../acting/#release_desktop) |
| `focus_window` | yes | focuses a window by id | [Acting](../acting/#focus_window) |
| `focus_workspace` | yes | focuses a workspace by id | [Acting](../acting/#focus_workspace) |
| `launch` | yes | starts a preset from the policy file | [Acting](../acting/#launch) |
| `close_window` | yes | asks a window to close | [Acting](../acting/#close_window) |
| `niri_action` | yes | sends any niri action, such as fullscreen, floating or a window width | [Acting](../acting/#niri_action) |
| `pointer_move` | yes | moves the pointer to a pixel or an element | [Acting](../acting/#pointer_move) |
| `click` | yes | clicks a pixel or an element | [Acting](../acting/#click) |
| `drag` | yes | drags from one point to another | [Acting](../acting/#drag) |
| `scroll` | yes | turns the wheel at a pixel | [Acting](../acting/#scroll) |
| `key` | yes | presses up to 16 key combinations | [Keyboard](../keyboard/#key) |
| `type_text` | yes | types up to 1000 characters, optionally pressing Enter | [Keyboard](../keyboard/#type_text) |
| `paste` | yes | pastes up to 1 MiB through the clipboard, then restores it | [Keyboard](../keyboard/#paste) |
| `shell_open` | yes | opens one of three Noctalia panels | [Noctalia](../../concepts/noctalia/#panels) |
| `shell_close` | yes | closes it | [Noctalia](../../concepts/noctalia/#panels) |
| `noctalia` | yes | sends any Noctalia command; only with `unrestricted` | [Noctalia](../../concepts/noctalia/#any-command) |

The reading tools carry MCP's `readOnlyHint`, and `close_window`, `niri_action`, `click`, `drag`, `key`, `type_text`, `paste` and `noctalia` carry `destructiveHint`.

## Results

A successful call returns its data as `structuredContent` and the same JSON as text. `screenshot` returns an image block first.

Actions report what happened in two fields:

- `accepted`: whether niri, or Noctalia, took the request. False when nothing was sent, null when the request went out but the reply was lost.
- `observed`: what niri's event stream, or Noctalia's status, showed afterwards, such as `focused`, `closed` or `opened`. For input, `sent` once niri has handled it; what the app did with it is for the next screenshot to show.

`interrupted` means focus moved to a window the action wasn't about, so someone else is using the desktop. `uncertain` means a reply or the event stream was lost, so the action may or may not have happened. Neither is an error, and the server never retries. When an outcome is in doubt, the result comes with a fresh screenshot; any action asked with `screenshot: true` comes with one taken once the screen stopped changing.

## Errors

A failure sets `isError` and returns:

```json
{"error": "screen_locked", "detail": "…"}
```

`error` is a stable name from the [error reference](../../reference/errors/), and `detail` keeps the upstream message: niri's or Noctalia's reply, a program's exit status and stderr.

A mistake in the arguments, such as an unknown output, a window id that doesn't exist or a value of the wrong type, comes back with `isError` and one plain-text block starting `invalid arguments:`, without `structuredContent`. Nothing was done; the agent corrects the call.

## The audit log

Every call is appended to the [audit log](../../concepts/safety/#the-audit-log) with its argument metadata and outcome. Typed and pasted text, clipboard contents, window titles, accessible names and images are never written; the log keeps their length.
