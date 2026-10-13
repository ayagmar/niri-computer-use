# niri-computer-use errors

A failed call comes back with `isError`. There are two shapes.

**Argument mistakes**, such as an unknown output or a window id that doesn't exist, are plain text starting `invalid arguments:`. Nothing was done. Fix the arguments and call again.

**Everything else** is `{"error": <name>, "detail": <upstream detail>}`. Quote the `detail` when you tell the user; it carries niri's or Noctalia's own message. A `key` or `type_text` failure after some of its input went out, or may have, adds `"may_have_sent": true`, and its detail says how much: `typed 0 of 2 characters, and more may have gone out` means the first stroke may have reached the window. Look at a screenshot before typing again, and don't repeat the call as if nothing happened.

## Stop and tell the user

These are the user's decisions. Don't call the refused action again and don't try to work around it.

| Error | Cause |
|---|---|
| `stopped` | The user pressed the stop key or ran `niri-computer-use stop`; it also cancels a running action. A niri, Noctalia or element action already sent, or typing already under way, ends `uncertain` instead, with the stop in `detail`: stop there too. Only the user's `niri-computer-use resume` clears it. |
| `recovery_required` | Input may be stuck from an earlier crash. Only the user's `niri-computer-use recover` clears it, and it needs the lease: call `release_desktop` first. The exception: when the `detail` says input is still finishing, or another server's may still be finishing, wait a few seconds and try once more. |
| `screen_locked` | The screen is locked, or no source can say it isn't. |
| `session_mismatch` | The server's `WAYLAND_DISPLAY` and `NIRI_SOCKET` belong to two different compositors, so input, screenshots and clipboard reads are refused. The user has to fix the client's environment. |
| `lease_held` | Another agent holds the lease (`acquire_desktop` only). |
| `lease_declined` | The user's policy asks them to allow each `acquire_desktop` on the desktop, and they chose Deny, dismissed the prompt or let a minute pass. Ask the user before calling it again. |
| `read_only` | The niri version isn't supported, niri sent events this build can't read, or the policy file is invalid. |
| `app_denied` | The user's policy denies input to the focused app, or `elements`, element actions or closing for the window named. The pointer tools check only the focused app, not the window under the pointer: never aim at a denied app's window that doesn't have focus. |
| `secret_field` | `activate_element` or `set_element_text` was aimed at a field the toolkit marks as a password field (role `password_text`), including Qt's fields that hide their text. Nothing was sent; ask the user to type it themselves. |
| `untested_output_config` | The pointer only runs on one monitor at transform `Normal` (or in a nested niri). |
| `unrestricted_required` | `niri_action` was given a gated action (`Spawn`, `SpawnSh`, `Quit`, `LoadConfigFile`, niri's screenshot actions, monitor power, casts and a few more). It needs `unrestricted = true` in the user's policy file; tell the user. |
| `refused` | The selected keyboard backend cannot safely send the requested input: for example a text with more symbols missing from native's layout than it has spare keys, a missing control character, invalid backend selection, or held modifiers without native. Tell the user; do not change the backend or bypass the tools. |

## Fix and continue

| Error | What to do |
|---|---|
| `lease_required` | You called an action or `clipboard_read` without the lease, or the lease was given up while `acquire_desktop` was still finishing. Call `acquire_desktop` if the user asked you to act. |
| `lock_stale` | The desktop answered so slowly that the call couldn't go out within 250 ms of the lock state's read, so nothing was sent. Try the call once more; if it comes back `lock_stale` again, stop and tell the user the desktop is under load. |
| `unknown_preset` | `launch` named a preset that doesn't exist; the detail lists the ones that do. If none fits and `status.unrestricted.enabled` is true, start the app with `niri_action` `Spawn` and `wait_for` its window; otherwise ask the user to add one. |
| `ref_invalid` | The detail starts with `unknown_ref`, `expired`, `output_changed` or `out_of_bounds`. Take a new screenshot and aim again from it; for `out_of_bounds`, use a pixel inside the image. |
| `element_stale` | The element ref's window, app or element is gone, it is now another kind of element or moved in its window, it or an element it sits in changed its name since `elements` listed it, or this lease didn't list it. A list's rows usually go stale once it scrolls or loses a record. The pointer tools don't compare the element's own name, except for a ref to a named list row itself, whose name binds its record; `activate_element` and `set_element_text` do, so a button that relabelled itself when you activated it is stale for the next activation. Call `elements` again and aim from its new list. |
| `element_unbound` | Nothing on the accessibility bus binds the element to the record it acts on: it sits in a list row with no name, or in no row while the walk read other objects of its role and name, listed or not, and `elements` marked it `unbound`. Listing again gives the same answer: take a screenshot and aim at its pixels in the row you mean. Nothing was sent. |
| `element_unmappable` | The element is there but can't be aimed at now. The detail starts with `frame_size_mismatch` (the app's coordinates for this window can't be trusted: aim at a screenshot pixel or use `activate_element` instead), `not_showing` (bring it into view first, close niri's overview, or, when a tiled window as large as the output covers it, focus its window; when the view may still be moving, take a new screenshot and try again, or aim at screenshot pixels when the window keeps moving or resizing; when its centre is past its output's edge, move the window or aim at a screenshot pixel; when another floating window lies over it, use `activate_element` or aim at screenshot pixels), `tiled` (niri gives a tiled window no position: use `activate_element` or aim at a screenshot pixel; only when changing the user's layout is fine, float the window with `niri_action` and call `elements` again), `empty`, `outside_screenshot` (take a screenshot that shows it), or, from `activate_element`, `action_unmapped` (the app's action list doesn't say which action it would run, so nothing was sent: aim at the element's pixels on a fresh screenshot). |
| `element_busy` | The app or the desktop answered too slowly for `activate_element` or `set_element_text` to check the element right before acting, twice. Nothing was sent. Take a screenshot to see what it is busy with, then try once more; if it stays busy, use the pointer or keyboard, or ask the user. |
| `focus_mismatch` | The window in `expect` doesn't have keyboard focus, or for an element action the element's window doesn't (a shell panel may hold the keyboard). Look at `desktop_state` and a screenshot, focus the right window with `focus` if that's what you meant, then try again. |
| `text_too_long` | Over 1000 characters for `type_text`, 1 MiB with `via: "paste"`, or 64 KiB for `set_element_text`; nothing was typed or set. Split the text into calls of at most 1000 characters, pass `submit: true` only on the last and only if the original call had it, and send nothing more once a call comes back with `typed`. |
| `clipboard_unsaved` | A paste couldn't save the user's clipboard whole (too large, an owner that didn't answer, something its owner marked secret other than an earlier paste's text, or a copy made while it saved), so it changed nothing. Use `type_text` instead, in parts of at most 1000 characters with `submit: false`; only if the text was meant to be submitted, pass `submit: true` on the last part, after the others went out whole. Typed text can't hold line breaks, so ask the user about text with several lines. |
| `save_not_enabled` | The user turned saving off with `capture_dir = ""`, or their policy file is invalid. Tell the user; don't save the image another way. |
| `not_accessible` | The window's app isn't on the accessibility bus, or has no accessible window that is this one. Use screenshots for that window. |
| `ambiguous_window` | The app has several accessible windows that could be this one, the one of this window's size has another window's title, or another window of the app with the same title (or none) could be it as well. Use screenshots for that window. |
| `panel_not_allowed` | Only `control-center`, `wallpaper` and `tray-drawer` can be opened. Don't reach another panel some other way. |

## Report and don't loop

`niri_unavailable`, `deadline_exceeded`, `upstream_error` and `noctalia_unavailable` mean niri, a helper program, Noctalia or an app on the accessibility bus didn't answer as expected; for an element action, `upstream_error` can mean the app refused it, and its detail names only the D-Bus error, not the app's message. A detail starting `start grim:` or `start wtype:` means that program isn't on your client's `PATH`; `status.binaries` says which are. Tell the user the name and detail. Don't repeat the same call in a loop.

## Shared mode

`engine_lost` means the shared engine serving this session ended. What a call in flight did is unknown, and the lease is gone: look at `desktop_state` or a screenshot before acting again, and call `acquire_desktop` again if you still need it. `engine_unavailable` means no shared engine could be reached for this call; the next call tries again. Tell the user if it keeps happening.

## Not an error

`focused_window` is null while keyboard focus is outside the window layout, for example on a shell panel, the lock screen or the overview.
