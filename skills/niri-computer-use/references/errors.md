# niri-computer-use errors

A failed call comes back with `isError`. There are two shapes.

**Argument mistakes**, such as an unknown output or a window id that doesn't exist, are plain text starting `invalid arguments:`. Nothing was done. Fix the arguments and call again.

**Everything else** is `{"error": <name>, "detail": <upstream detail>}`. Quote the `detail` when you tell the user; it carries niri's or Noctalia's own message.

## Stop and tell the user

These are the user's decisions. Don't call the refused action again and don't try to work around it.

| Error | Cause |
|---|---|
| `stopped` | The user pressed the stop key or ran `niri-computer-use stop`; it also cancels a running action. Only the user's `niri-computer-use resume` clears it. |
| `recovery_required` | Input may be stuck from an earlier crash. Only the user's `niri-computer-use recover` clears it. |
| `screen_locked` | The screen is locked, or no source can say it isn't. |
| `lease_held` | Another agent holds the lease (`acquire_desktop` only). |
| `read_only` | The niri version isn't supported, niri sent events this build can't read, or the policy file is invalid. |
| `app_denied` | The user's policy denies input to the focused app. |
| `untested_output_config` | The pointer only runs on one monitor at transform `Normal` (or in a nested niri). |

## Fix and continue

| Error | What to do |
|---|---|
| `lease_required` | You called an action without the lease. Call `acquire_desktop` if the user asked you to act. |
| `unknown_preset` | `launch` named a preset that doesn't exist; the detail lists the ones that do. If none fits, ask the user to add one. |
| `ref_invalid` | The detail starts with `unknown_ref`, `expired`, `output_changed` or `out_of_bounds`. Take a new screenshot and aim again from it; for `out_of_bounds`, use a pixel inside the image. |
| `focus_mismatch` | The window in `expect` doesn't have keyboard focus. Look at `desktop_state` and a screenshot, focus the right window with `focus_window` if that's what you meant, then type. |
| `text_too_long` | Over 1000 characters; nothing was typed. Split the text into calls of at most 1000 characters, pass `submit: true` only on the last, and send nothing more once a call comes back with `typed`. |
| `panel_not_allowed` | Only `control-center`, `wallpaper` and `tray-drawer` can be opened. Don't reach another panel some other way. |

## Report and don't loop

`niri_unavailable`, `deadline_exceeded`, `upstream_error` and `noctalia_unavailable` mean niri, a helper program or Noctalia didn't answer as expected. Tell the user the name and detail. Don't repeat the same call in a loop.

## Not an error

`focused_window` is null while keyboard focus is outside the window layout, for example on a shell panel, the lock screen or the overview.
