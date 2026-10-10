---
title: Keyboard and paste
description: key, type_text and paste, the focus check, and the experimental native backend.
sidebar:
  order: 5
---

The keyboard tools type into the focused app. They need the lease, and each takes `expect`, the window the agent means to type into.

## How typing works

With the default backend, `key` presses each of its combinations with one `wtype` call, and `type_text` types with one `wtype` call per part of 100 characters, plus one for `Return` with `submit`. The keys go to the app, not to niri: niri's own keybinds don't fire from them. They run through the same gate as the other action tools, then check, before anything is typed:

1. the arguments: `type_text` refuses over 1000 characters, counted as Unicode scalar values, with `text_too_long`, and an empty text, an empty list of keys, more than 16 keys, or any combination it can't read is an argument mistake
2. `expect` against the window with keyboard focus: `focus_mismatch` when it doesn't match. A lock screen, the overview or a shell panel leaves no window focused, so only `"none"` types there
3. the focused window: `app_denied` if its `app_id` is on the policy's deny list

`expect` is required, so the agent says where it means to type:

| `expect` | Means |
|---|---|
| `{"window_id": <id>}` | that window, from `desktop_state`, must have keyboard focus |
| `{"app_id": "<app_id>"}` | the focused window must have that `app_id` |
| `"none"` | no check, for example to type into a shell panel or a dialog that holds focus outside the windows |

After each `wtype` call the server checks that keyboard focus is still on the window it started on, and if it moved, sends nothing more. The result has `accepted: true`, `focus` (`matched`, or `unchecked` for `"none"`), and `observed`: `sent` once the last `wtype` has exited, or `interrupted` if keyboard focus moved off the window that had it at any time during the call; an interrupted result comes with a screenshot. If niri's event stream was lost meanwhile, `observed` is `uncertain` with `accepted: true`: the keys went out, but where focus went is unknown. What the keys did is for the next screenshot to show.

Before `wtype` starts, the server writes the input-dirty marker (`pending`), and once `wtype` runs it adds its PID and start time (`running`). `wtype` starts with `-`, so it waits at its stdin until the server has recorded it. The server removes the marker when `wtype` exits by itself. A stop or a cancelled request ends the call with `stopped` or a cancellation, but a `wtype` already running keeps typing and then removes the marker; a server whose client has gone waits for it before exiting. A `wtype` still running three seconds after it started is killed with its process group, and one killed by a signal, the same way: the marker stays and every action refuses with `recovery_required` until the user runs `niri-computer-use recover`. The text is never logged: the audit log has `text_len`.

### `key`

| Argument | Value |
|---|---|
| `keys` (required) | 1 to 16 combinations, pressed in order. Each is modifiers and one key joined by `+`, such as `ctrl+s`, `ctrl+shift+t`, `alt+F4` or `Return`. The key is an XKB keysym name (`a`, `Return`, `Escape`, `F5`, `slash`, `Page_Down`); the modifiers are `shift`, `ctrl`, `alt`, `altgr` and `super` |
| `expect` (required) | see above |
| `screenshot` | see Action tools |

For each combination, presses the modifiers, presses and releases the key, then releases the modifiers in reverse order. If focus moves after one, the rest aren't pressed: `observed` is `interrupted` and `pressed` gives the number that were. A combination that fails ends the call with that error, its `detail` starting with how many were pressed before it.

### `type_text`

| Argument | Value |
|---|---|
| `text` (required) | 1 to 1000 characters |
| `expect` (required) | see above |
| `submit` | default false; with true, `Return` is pressed once all of the text went out |
| `screenshot` | see Action tools |

The text goes out in parts of 100 characters. If focus moved after a part, the rest isn't typed: `observed` is `interrupted` and `typed` gives the number of characters sent. A part that fails ends the call with that error, its `detail` starting with how many characters were typed before it. A result without `typed` means the whole text went out. With `submit`, the result has `submitted`: true once `Return` was pressed, false when the text stopped early and `Return` wasn't pressed.

### `paste`

| Argument | Value |
|---|---|
| `text` (required) | up to 1 MiB, counted in UTF-8 bytes |
| `keys` (required) | the combination that pastes in the focused app: `ctrl+v`, `ctrl+shift+v` (terminals) or `shift+Insert` |
| `expect` (required) | see above |
| `screenshot` | see Action tools |

Pastes the text through the clipboard, then puts the clipboard back. `expect` and the deny list are checked first, before the clipboard is touched. Then a keeper process, the server's own binary as `niri-computer-use paste-keeper`, reads every type the clipboard offers through the wlr data-control protocol, up to 16 MiB in all within two seconds; if it can't, or the clipboard holds `x-kde-passwordManagerHint` set to `secret`, the call ends with `clipboard_unsaved` and nothing changes. The keeper takes the selection with the text, offered as `text/plain;charset=utf-8`, `text/plain`, `UTF8_STRING`, `STRING` and `TEXT`, and marked with that hint so clipboard managers that honour it keep it out of their history. The server then presses `keys` as `key` would, with the same checks. Right before the key goes out, it asks the keeper to admit it and waits up to half a second for the answer. The keeper admits the key only while it still holds the text on the clipboard and is still waiting for the key, which it does for ten seconds after taking the clipboard; after that it has already put the clipboard back, and it admits nothing. A key the keeper doesn't admit isn't sent.

Once the key went out, the keeper waits up to two seconds for an app to read the text, then until no new read has begun for 100 ms, and offers every saved type again. It keeps serving them, as `wl-copy` does, until something else is copied. If the key didn't go out, it restores at once. A stop, a cancelled call or a client that goes away doesn't cut short a key already on its way: the keeper still waits for that key and then for the read, and other input is refused until it has finished. If the server dies once the key may be on its way, the keeper can't know whether the key will still arrive, so it doesn't restore: the pasted text stays on the clipboard, and what you had copied before is lost, rather than pasted into the app by a late key. If something else was copied meanwhile, that copy stays, unless niri handles it in the moment between the keeper's last check and its restore: the clipboard protocol can't set the selection only if nothing else took it.

The result is `key`'s with `paste`: `read`, whether an app asked for the text after the key, and `clipboard`: `restored`; `cleared`, when nothing was copied before and nothing is again; `replaced`; `failed`, with `detail`; `kept`, when the server couldn't say whether the key went out, so the pasted text stays; or `unknown` when the keeper didn't report within five seconds. A failed key's error says what became of the clipboard in its `detail`. When the keeper didn't admit the key, the call fails with `upstream_error`, and its `detail` says that the key wasn't sent, that nothing was pasted, and what became of the clipboard, usually that it was restored. A read can't be traced to the app it came from: a clipboard manager that reads every new selection and ignores the hint counts as a read. The text is never logged: the audit log has `text_len`.

## The native backend

`NIRI_COMPUTER_USE_KEYBOARD=native` in the server's environment selects an experimental backend that types through a virtual keyboard of the server's own instead of `wtype`. Unset, or `wtype`, keeps the default; any other value refuses every keyboard call with `refused`. It is not the default because it hasn't met the test bar the default has, and whether it preserves physical modifiers you hold while it types is unverified. Use it only in sessions where nobody else types.

What changes with native:

- It uses the compositor's keymap and checks focus, the layout and the stop flag between individual key presses, not between parts of 100 characters.
- Text with symbols the active layout lacks is typed through a temporary keymap for that call, using keycodes the layout leaves empty, and the compositor's keymap is restored before the call succeeds. There are only so many spare keycodes (14 in a US layout), so a text needing more distinct missing symbols, or a missing control character, is refused whole with `refused`; split it.
- `key` combinations still refuse keysyms the layout lacks.
- `click`, `drag` and `scroll` can hold modifiers with `keys`.
- If the server is killed mid-call, its crash guardian releases the keys and sends the compositor's keymap back.
- A call that ends, is cancelled or is cleaned up after a crash leaves the focused application in the layout niri has active, also when you switched layouts during the call.

