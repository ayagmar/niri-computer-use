---
title: Acting
description: The lease, the structured actions through niri, and the pointer tools.
sidebar:
  order: 4
---

Acting needs the lease. Prefer the structured actions, which go through niri's IPC and can't land on the wrong thing: `focus`, `launch` and `close_window`. Use the pointer for what happens inside an app, and the [keyboard tools](../keyboard/) for typing. The Noctalia tools are on the [Noctalia page](../../concepts/noctalia/).

## The lease

### `acquire_desktop`

No arguments. Takes the lease on this niri instance and returns `{"holder": {"pid", "label", "since"}, "users_window": <id>}`. `users_window` is the window that had keyboard focus when the lease was taken, or null; `release_desktop` can give focus back to it. One server holds it at a time; the action tools require it. Calling it again while holding the lease returns the same holder and window. It starts the clipboard's watcher (see [`clipboard_read`](../looking/#clipboard_read)) before the lease is granted, so a request cancelled before it returns may leave the lease granted or not, but never without its watcher; `status` says which.

Refused with `lease_held` while another server holds it. While the holder's own `acquire_desktop` is still starting its watcher, another session's call waits for that to finish before it is refused, which is bounded by the watcher's deadlines but can take seconds when niri is slow. It is also refused with `stopped` while the stop flag is set, `recovery_required` while the input-dirty marker exists, the niri error when niri's version can't be read, `read_only` when this build doesn't support the running niri, niri sent events it can't parse, or the policy file is invalid, and `screen_locked` while the screen is locked or its lock state is unknown. If the runtime directory can't be read, it fails with `upstream_error` rather than assume neither flag is set.

With [`confirm_lease`](../../concepts/configuration/#confirm_lease) on, the call then waits for the user to choose Allow on a desktop notification, after the checks above pass and before the lease is granted. Deny, a dismissed notification or no answer within 60 seconds is `lease_declined`, and the checks run again after an Allow, since the answer can take up to a minute. A stop or the client's end while it waits refuses the call at once. Meanwhile `status` names the asking client as the `holder` with `pending: true`, and another server's `acquire_desktop` is refused with `lease_held`. Another session of the same engine that calls `acquire_desktop` waits for the answer before it is refused with `lease_held` or gets the lease; its `release_desktop` and its end don't wait.

### `release_desktop`

| Argument | Value |
|---|---|
| `restore_focus` (required) | true to give keyboard focus back to `users_window` before giving the lease up |

Returns `{"users_window": <id>, "released": true}`, or `released: false` if this server didn't hold it. With `restore_focus` and a `users_window`, it first focuses that window as `focus` would, through the same checks, and adds `restored`: that action's result, with `observed: closed` and `accepted: false` if the window is gone, or `{"error", "detail"}` if the action was refused. Those checks include the readiness report and a second read of the lock state, so giving focus back takes as long as they do before any action: milliseconds usually, and seconds when niri, Noctalia or logind is slow, since each call to them has a two-second deadline. After the stop key there is nothing to give back: the stop took the lease and forgot `users_window` with it, so the result has `users_window: null`, `released: false` and no `restored`, and nothing is read or sent. A stop that comes before the lease is taken back refuses the focus change with `stopped` before anything is read. The lease is given up either way. While its own action runs, it waits for that action to end. Each step of an action has its own deadline, but the action as a whole has none, so the wait is at worst the sum of its steps, such as a `key` call's two-second round trips to niri, its keys' time of 50 ms a key and at least three seconds, and the ten seconds its release may take. The stop key doesn't wait. The lease is also given up when the stop flag appears and when the server exits.

## Action tools

`focus`, `launch`, `close_window` and `niri_action` act on the desktop through niri's IPC, and the pointer and keyboard tools send input. Before each action the server checks, in this order, the stop flag (`stopped`), the input-dirty marker (`recovery_required`), the lease (`lease_required`), then niri's version, the policy file and the lock state, as `acquire_desktop` does. One action runs at a time; an action or `release_desktop` called meanwhile waits for it. A stop during an action cancels it with `stopped`, or, once a niri action's request or a Noctalia command went out or `key` or `type_text` started typing, with the outcome `uncertain`, the stop in `detail`, `accepted: true` if niri or Noctalia had answered or a stroke was typed and `null` if not, and no screenshot; nothing more is sent. So does the lease file being removed or replaced, with `lease_required`. The [pointer tools](#pointer-tools) end `uncertain` too once their first motion or held modifier went out, with `accepted: null`, and release what they held. A paste ends with the error either way, and a key it had already sent may have taken effect. Cancelling the MCP request cancels the action and keeps the lease.

Every action tool also takes `screenshot`, default false. With true, the result comes with a screenshot of the focused output taken once the screen stopped changing: the server looks 50 ms after the action, then captures every 100 ms until two captures in a row are the same image, for at most 1.5 seconds, and `settled` in its metadata says whether they were. The screenshot is taken before the next action can start, so it shows this action's result, and its `screenshot_ref` serves the pointer tools.

Every action's result also says what niri saw it change, as `changes`, so the agent can often tell what an action did without a screenshot. The server watches niri's event stream from the start of the action, without asking niri anything, until niri has reported no change that `changes` would show for 250 ms, counted from the start of the action, so the time the action's own wait spends quiet counts; at most half a second after that wait, or with `screenshot: true` until the screenshot was taken; so a dialog that opens a moment after a click is in it. `focus` doesn't wait for that once its own wait ended with `focused` or `closed`, and only takes the rest of niri's events from that moment, at most 50 ms more. `niri_action` doesn't once its own wait ended with `changed`, `closed` or `unchanged`, and its `Spawn` and `SpawnSh` never do: the window they start comes later anyway, so find it with `wait_for`. A change that comes after they returned, such as an app's answer to the focus change, isn't in their `changes`: `desktop_state` has it, and so does the next action's `changes` if it comes while that action runs. `changes` is left out when nothing changed. Absent `changes` means niri saw no change, not that the app did nothing: typed text or a ticked check box changes nothing niri reports. It has, each only when it has something:

| Field | Value |
|---|---|
| `opened` | windows open afterwards and not before, each as `{"id", "app_id", "workspace_id", "title"}`; a window that opened and closed again in between is in both lists |
| `closed` | windows open before and not afterwards, each as `{"id", "app_id"}` |
| `focus` | `{"from", "to"}`: the window with keyboard focus before and afterwards, null for none |
| `workspace` | `{"from", "to"}`: the focused workspace before and afterwards |
| `windows` | windows open before and afterwards whose `title`, `app_id`, `workspace_id`, `floating`, `size` (niri's `window_size`), `position` or `urgent` changed, each as its `id` with the new values of the fields that changed. niri 26.04 has no fullscreen flag, so fullscreen shows as a size. `position` is niri's `tile_pos_in_workspace_view` for a window floating before and after that moved. Positions in the scrolling layout aren't reported: opening a column moves every column to its right |
| `truncated` | how many window entries were left out: `opened`, `closed` and `windows` hold 12 together |
| `lost` | why the report is incomplete, with niri's reason: its event stream ended before the report did, and what is there was seen before, or it couldn't be watched at all, and no window, focus or workspace change is there; `elements`, from its own walks, can still be |
| `elements` | with `element_changes`, what changed among a window's accessible elements: see [Element changes](../elements/#element-changes) |

Titles are cut at 100 characters, with `…`. A stop or another server taking the lease during the action leaves `changes` out, as it leaves out the screenshot.

An unknown window or workspace id is an argument mistake, and nothing is sent. Otherwise the result has these fields:

| Field | Value |
|---|---|
| `accepted` | true once niri acknowledged the request; false when nothing was sent; null when the request was sent but niri's reply was lost |
| `observed` | what niri's event stream showed afterwards; the values are listed under each tool |
| `focused_window` | the window with keyboard focus when `changes` ended, the same view `changes.focus.to` comes from; null when focus isn't on a window or niri's event stream was lost meanwhile |
| `windows` | the windows the outcome is about, when there are any |
| `changes` | what niri saw the action change, as above; absent when nothing did |
| `detail` | why the outcome is `uncertain` |
| `screenshot` | with `screenshot: true` or an outcome in doubt, the metadata of a screenshot of the focused output, as `screenshot` returns it; the image follows the text in the result's content |
| `screenshot_error` | `{"error", "detail"}` saying why that screenshot couldn't be taken |

`focus`, `launch` and `close_window` wait up to five seconds for their effect; the other tools' waits are under each tool, and none of them bounds the whole call. Two outcomes can end any action:

- `interrupted`: focus moved to a window that was neither focused before nor the expected target, so someone else is using the desktop. `focused_window` says where focus came to rest.
- `uncertain`: niri's reply or event stream was lost, so the action may or may not have happened. `accepted` is null when the reply was lost.

None of these outcomes is an error, and the server never retries an action. `timeout`, `pending`, `none`, `interrupted`, `uncertain` and `unknown` are outcomes in doubt: the result comes with a fresh screenshot of the focused output, 1280 pixels wide, so the agent can see the desktop without another call.

## `focus`

| Argument | Value |
|---|---|
| `window_id` | a window id from `desktop_state` |
| `workspace_id` | instead of `window_id`: a workspace id from `desktop_state`, not its index |

Give exactly one of the two; both or neither is an argument mistake.

For a window, `observed` is `focused` once the window has keyboard focus, or `timeout`. If the window already has focus, nothing is sent: `observed` is `focused` with `accepted: false`.

For a workspace, `observed` is `focused` once the workspace has focus, on whichever output it is, and keyboard focus is on the workspace's active window, or on no window when it is empty; otherwise `timeout`. While the overview, the screenshot UI or a shell surface that keeps keyboard focus is open, focus stays off every window, so a switch that happened still reports `timeout` with `focused_window` null; the screenshot shows why. Focus moving to one of the workspace's own windows is expected, not an interruption. If the workspace already has focus, nothing is sent: `observed` is `focused` with `accepted: false`. Sending it would let niri's `workspace-auto-back-and-forth` switch to the previous workspace.

## `launch`

| Argument | Value |
|---|---|
| `preset` (required) | a preset name from the policy file, listed in `status` as `policy.preset_names` |
| `reuse` | default false; with true, focus the preset's existing window instead of starting another |

niri starts the preset's fixed `argv`; the app keeps running after the server exits. `observed` counts the windows with the preset's `app_id` that weren't open before, including one that sets its `app_id` after it appears. Once the first appears the server keeps counting for half a second, then reports `one` or `ambiguous`, with the ids in `windows`. With no such window in five seconds it reports `none`. Focus moving to a new window, before it has its `app_id`, isn't an interruption. A single-instance app hands a second start to its running process, which focuses the window it already has: `observed` is then `focused`, with that window in `windows`.

With `reuse`: one existing matching window is focused, and `observed` is `focused` with its id in `windows`; several give `ambiguous` with their ids and `accepted: false`, and nothing is started; none starts the preset as usual.

With no preset for the app and [`unrestricted`](../../concepts/configuration/#unrestricted) on, the skill tells agents to start it with `niri_action`'s `Spawn` and the program's argv, never a shell or `SpawnSh`. `Spawn` reports only `sent`, so they then find the window with `wait_for` or `desktop_state`, without assuming its `app_id` is the program's name, and don't start it again after an uncertain result or a timeout. With `unrestricted` off they ask you for a preset.

## `close_window`

| Argument | Value |
|---|---|
| `id` (required) | a window id from `desktop_state` |

Asks the window to close, as its close button would. `observed` is `closed` once niri reports it gone, or `pending` if it is still open after five seconds, for example behind an unsaved-changes dialog. Nothing forces it closed. `windows` holds the id.

## `niri_action`

| Argument | Value |
|---|---|
| `action` (required) | one niri action in niri's IPC JSON: an object whose only key is the action's name, as in niri-ipc 26.4's `Action` |

For window layout and other compositor actions that have no tool of their own. For example:

```json
{"action": {"FullscreenWindow": {"id": 12}}}
{"action": {"SetWindowWidth": {"id": 12, "change": {"SetFixed": 1600}}}}
{"action": {"ToggleWindowFloating": {"id": null}}}
{"action": {"MaximizeColumn": {}}}
```

An `id` (or `window_id`) of null means the focused window. The action goes out naming the window focused at the check by its id, so focus moving before niri handles it can't redirect it; with no window focused it sends nothing and is an argument mistake. JSON that isn't a niri action is an argument mistake carrying serde's message, and so is a window id that doesn't exist. niri's refusal is `upstream_error` with niri's message.

For an action about one window, the one it names or the focused one, the server waits up to a second for niri to report a change in that window, then 200 ms more for a resize that comes in steps. `observed` is `changed`, `unchanged` or `closed`, `windows` holds the id, and `window` is the window as niri reports it when `changes` ended, or as last seen once it closed:

| Field | Value |
|---|---|
| `window_size` | the window's size in logical pixels, without niri's borders |
| `tile_size` | the tile's size, borders included |
| `is_floating`, `is_focused`, `is_urgent` | as niri reports them |
| `workspace_id` | the window's workspace |
| `pos_in_scrolling_layout` | column and row from 1, or null while floating |
| `tile_pos_in_workspace_view` | a floating window's top-left corner in the workspace view, or null for a tiled one, so moving a floating window is `changed` |

niri 26.04 reports no fullscreen or maximized flag; a fullscreen window's `window_size` is its output's logical size. It reports no place for a tiled window either, so an action that only scrolls the view, such as `CenterWindow` on a tiled window, is `unchanged`. Any other action, such as focusing a column or opening the overview, gives `sent` once niri handled it.

Prefer `focus`, `close_window`, `launch` and `screenshot` where they fit, since they watch for their effect. Some actions are gated: `Spawn` and `SpawnSh` run programs, `Quit` ends the session, `PowerOffMonitors` and `PowerOnMonitors` switch the monitors, `LoadConfigFile` loads a config that can bind keys and start programs, niri's `Screenshot` actions write files and replace the clipboard, `ToggleKeyboardShortcutsInhibit` can keep the stop key's bind from reaching niri, `SwitchLayout` changes the layout the user types with, the cast actions change what a screencast shows, the debug toggles change niri's rendering, and `DoScreenTransition` freezes what every output and screencast shows for up to 65 seconds. They fail with `unrestricted_required` unless the user turned on [`unrestricted`](../../concepts/configuration/#unrestricted). Every action, gated or not, needs the lease, stops at the stop key and is written to the audit log as metadata: the action's name and field names, its numbers and booleans, and every string as its length in bytes, so a `Spawn` command or a workspace name isn't kept. JSON that isn't a niri action is logged as `invalid_action` with its size.

## Pointer tools

`pointer_move`, `click`, `drag` and `scroll` aim at pixels of a screenshot: each takes the `screenshot_ref` of a screenshot this server took under the current lease, and pixel coordinates in that image, counted from its top-left corner. A pixel targets its centre. They run through the same gate as the other action tools. Then, before sending anything, the server checks:

1. the arguments: `count` from 1 to 3, at least one scroll axis, at most 10 notches each way (otherwise an argument mistake)
2. the ref: `ref_invalid` with `unknown_ref` if it isn't a screenshot of this lease
3. the focused window: `app_denied` if its `app_id` is on the policy's deny list
4. niri's outputs, read again right then: `untested_output_config` unless there is one enabled output, a monitor at transform `Normal` or nested niri's `winit` window
5. each pixel through the ref: `ref_invalid` with `expired` past 60 seconds, `unknown_ref` if niri's event stream reconnected since the screenshot, `output_changed` if its output moved, resized, changed scale or transform, or is gone, `layout_changed` if the windows on that output moved after the capture began or just before it, and `out_of_bounds` for a pixel outside the image

`layout_changed` follows what niri reports for each output: the overview opening or closing, the output showing another workspace, a window closing, opening or coming onto that workspace, niri moving or resizing one there, or the workspace's active window changing, which is the window niri scrolls the view to, as after a focus change. The server keeps the time of the last such change on each output. A pixel aim is refused when it came after the capture began, even when it was undone since, as with focus moving away and back, and when it came less than a second before, while niri may still have been animating the windows the image shows. A screenshot that waited for the screen to settle, as `screenshot: true` on an action or `wait_for` with `screen_stable` does, is still good when the screen stopped changing after that change. The `detail` names the last change. niri reports no place for a tiled window, so a view scrolled with no other change goes unseen, and changes inside a window, such as a page loading, are for a screenshot to show. An aim at an `element` is placed again from the app right before the gesture, so it doesn't take this check.

`pointer_move`, `click` and `drag` can aim at an element from [`elements`](../elements/) instead of a pixel: pass its `element_ref` as `element`, with the `screenshot_ref` of a screenshot that shows it. Just before sending, the server asks the app for the element again and aims at the centre of its box now. If the element, its window or its app is gone, the call fails with `element_stale`; if it can't be aimed at, such as an element that isn't showing or whose centre is outside the screenshot, with `element_unmappable`. To press an element without the pointer, use [`activate_element`](../elements/#activate_element).

`click`, `drag` and `scroll` also take `keys`, up to five modifiers held through the gesture: `shift`, `ctrl`, `alt`, `altgr` and `super`, as the [keyboard](../keyboard/#modifiers-you-hold) holds them.

The input goes through a virtual pointer bound to the screenshot's output, on a Wayland connection of its own to the display in `WAYLAND_DISPLAY`. The server first checks that the process serving that display is the niri at `NIRI_SOCKET`, and fails with `upstream_error` otherwise. The result has `accepted: true` and `observed: sent` once niri has handled the input, with `focused_window` as niri's event stream showed it just after. What niri saw the input change is in `changes`; what it did inside a window is for a screenshot to show. If the Wayland connection fails before anything reached it, the call fails with that error. If it breaks or niri stops answering after something was sent, `observed` is `uncertain` with `accepted` null and a `detail`, and the result comes with a screenshot.

`click` and `drag` write the input-dirty marker before they send anything, and remove it once niri has handled the button's release. A stop, or the lease file being removed or replaced, once the first motion or held modifier went out ends the call `uncertain`, with `accepted` null and a `detail` saying the input may have taken effect; before that, it fails with `stopped` or `lease_required` and nothing was sent. If the call is cancelled midway, by a stop or by the client, the server releases the button on the way out and removes the marker once niri has handled the release, so other calls may see `recovery_required` for a moment. If the release can't be sent or the marker can't be removed, the marker stays, and every action refuses with `recovery_required` until the user runs `niri-computer-use recover`. If the server itself is killed, its crash guardian sends the releases at once; the marker still stays until `recover`.

### `pointer_move`

| Argument | Value |
|---|---|
| `screenshot_ref` (required) | from a screenshot under this lease |
| `x`, `y` | the pixel in that image |
| `element` | instead of `x` and `y`: an `element_ref` from [`elements`](../elements/), aimed at its centre |

Moves the pointer there, to hover.

### `click`

| Argument | Value |
|---|---|
| `screenshot_ref`, and `x` and `y` or `element` | as for `pointer_move` |
| `button` | `left` (default), `right` or `middle` |
| `count` | 1 (default) to 3; 2 is a double click |
| `element_changes` | default false; with true and `element`, [element changes](../elements/#element-changes) for the element's window. With a pixel, it is an argument mistake: the server doesn't know which window is under a pixel |

Moves to the pixel, then presses and releases the button `count` times.

### `drag`

| Argument | Value |
|---|---|
| `screenshot_ref` (required) | from a screenshot under this lease |
| `from`, `to` (required) | each `{"x", "y"}`, a pixel in that image, or `{"element"}` |
| `button` | `left` (default), `right` or `middle` |

Moves to `from`, waits 50 ms, presses the button, waits 50 ms, moves to `to` in ten even steps 20 ms apart, and releases the button there. After each wait it looks at niri's events again. If niri switched a workspace or opened or closed the overview meanwhile, or, before the press of a drag from an element, changed any window, the drag stops there: it releases the button where the pointer is and sends nothing more, and `observed` is `uncertain`, since the motion or the press may have done something already. A window that moves while the button is held doesn't stop it, as dragging a window by its title bar moves that window, unless it is the window of the element `to` names: when that window moves, resizes, goes to another workspace or closes, the element isn't where the drag goes any more, so the drag stops the same way. So does another floating window opening or moving over that element, up to the release. The one exception is a window that opened during the drag under the pointer and clear of the element, as a tab the drag tears out does: it can follow the pointer onto the element, and the drag stops only if it covers the element without lying under the pointer. Dragging an existing floating window by its title bar onto an element stops the drag once that window covers the element. The same look comes after held `keys` are pressed, before the first motion. Each look also reads the lock state again if its last read is more than 250 ms old, so a screen locked during one of the waits stops the gesture the same way. A read that is itself too slow is made again before that, for up to one second of reading again in all per call.

### `scroll`

| Argument | Value |
|---|---|
| `screenshot_ref`, `x`, `y` (required) | as for `pointer_move` |
| `notches_y` | wheel notches down, negative for up; default 0 |
| `notches_x` | wheel notches right, negative for left; default 0 |

Moves to the pixel and turns the wheel by whole notches, the vertical axis first: for each axis, one frame of `axis_discrete` with 15 per notch, as niri's own wheel uses, then `axis_source` wheel. Apps scroll by their own amount per notch.

