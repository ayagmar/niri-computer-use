---
title: Safety
description: What niri-computer-use enforces, what it doesn't, and how you take the desktop back.
sidebar:
  order: 1
---

An agent using niri-computer-use works on your real desktop, with your apps and your files. A click or a key can do anything you could do. The server adds guardrails that catch mistakes and keep you in control; it is not a sandbox. To report a security problem, see [SECURITY.md](https://github.com/ayagmar/niri-computer-use/blob/main/SECURITY.md).

## The lease

Looking needs nothing: any connected agent can call `status`, `desktop_state`, `screenshot` and the other reading tools. Acting needs the lease. An agent takes it with `acquire_desktop` and gives it up with `release_desktop`. One agent session holds it per niri instance; another session's `acquire_desktop` is refused with `lease_held`, and its detail names the holder's PID, client and since when. That holds in [shared mode](../configuration/#shared) too, where one engine serves every client.

With [`confirm_lease`](../configuration/#confirm_lease) on, `acquire_desktop` also waits for you to choose Allow on a notification before the lease is granted, whatever the agent's client approves on its own. The notification gives the name the client calls itself and the process it started. Nothing checks that name, so the server shows at most 64 characters of it, with anything but ASCII letters, digits, spaces and `._-+@` replaced by `_`; the same name is in the audit log and the lease holder's `label`. It holds against an agent that acts through this server, not one that can run commands in a shell of its own: that agent could put a `notify-send` that answers Allow on the client's `PATH`, or press Allow through the notification server's D-Bus interface.

`acquire_desktop` notes the window you were on. `release_desktop` with `restore_focus: true` puts keyboard focus back there. The lease is also given up when the agent's session ends, because its client closed the connection or its server exited, and when you press the stop key.

## Stop and resume

Bind `niri-computer-use stop` to a key in your niri config (see [First session](../../start/first-session/#bind-the-stop-key)). Pressing it:

- cancels the action that is running, which ends with `stopped`. A niri action, an element action or a Noctalia command whose request had already gone out, `key` or `type_text` once a stroke had, and the pointer tools once their first motion or held modifier had, end as `uncertain` instead, since they may have taken effect, with `accepted: true` if niri, the app or Noctalia had answered or a stroke was typed and `null` if not. A paste ends with `stopped` either way, and a key it had already sent may have taken effect
- takes the lease back from the agent, once a screenshot or readiness check under way has finished
- refuses every later `acquire_desktop` and action with `stopped`

The flag stays until you run `niri-computer-use resume`. An agent can't press the stop key: niri's keybinds don't fire from the virtual keyboard and pointer the server uses.

## Input that may be stuck, and `recover`

Before `click`, `drag`, `key`, `type_text` or a `scroll` with held keys send any input, the server writes an input-dirty marker, and it removes the marker once the input is released. A paste takes the clipboard before it writes its marker, and its keeper process, not the marker, puts the clipboard back (see [Pasting](../../tools/keyboard/#pasting)).

If the call is cancelled midway, the server releases any held button or key. A paste key already sent isn't taken back: it finishes after the cancellation, and the marker stays until it has, for a paste until the keeper has reported. Meanwhile other calls refuse with `recovery_required` and a `detail` saying a cancelled call's input is still finishing; they can try again within seconds.

If the input can't be released, or its cleanup ended without removing the marker, for example because niri didn't acknowledge the release within the cleanup's ten seconds, the marker stays. Every action then refuses with `recovery_required`, and the `detail` tells the agent to call `release_desktop` and you to run `niri-computer-use recover`, which needs the lease. A server can only tell whether its own input is still finishing: for a marker another server wrote, such as a shared engine's seen by a standalone server, while that server still runs, the `detail` says that input may still be finishing and to try again in a few seconds before `recover`. A marker whose server has exited, or whose crash guardian has sent its releases, gets the `recover` wording at once.

`recover` releases any pointer button or key the marker names, and ends the `wtype` child a 0.1.0 server's marker names and zeroes the modifiers it may have held, then asks you to check that no key or button is held. It clears the marker only when you type `yes`. See the [CLI reference](../../reference/cli/#recover).

## The crash guardian

Each standalone `serve`, and each shared engine, starts a small guardian process. If the server or engine dies while it holds input, for example after `kill -9`, the guardian at once releases the native keyboard's keys and the pointer buttons that the marker records. It leaves alone a marker it can't read, and a release can fail or time out. The marker still stays until you run `recover`.

If the guardian dies too, as when the whole service is killed at once, the next server or engine to start on the same niri sends those releases as it starts. It skips a marker whose server's process ID is in use again.

## The lock gate

The server acts only on a screen it knows is unlocked. It asks logind for niri's session's `LockedHint` and, when Noctalia runs, Noctalia's own lock state. If either says locked, or neither can answer, `acquire_desktop` and every action refuse with `screen_locked`. The state is checked again before each action, because the screen can lock while an agent holds the lease, and an action's request, input or command goes out at most 250 ms after that check began. When the check is older by then, it runs again, and an action that still can't go out in time is refused with `lock_stale`, having sent nothing. A lock that comes after that check isn't caught.

logind only knows the lock state when niri runs as `niri --session`. Without that and without Noctalia, the state is `unknown` and the server won't act.

## Refusals

Before each action the server checks, in this order: the stop flag (`stopped`), the input-dirty marker (`recovery_required`), the lease (`lease_required`), whether niri answers (its own error), niri's version and event stream and the policy file (`read_only`), and the lock state (`screen_locked`). The tools then check their own arguments:

- `launch` starts only presets from your [policy file](../configuration/), never a command the agent writes.
- `niri_action` refuses niri's actions that run programs, write files or reach past the window layout, such as `Spawn`, `Quit` and `LoadConfigFile`, with `unrestricted_required`, and the `noctalia` tool isn't listed, unless you turn on [`unrestricted`](../configuration/#unrestricted). Turning it on lets an agent run any program through the server; every call still needs the lease, stops at the stop key and is logged.
- The pointer and keyboard tools refuse with `app_denied` while the focused window's `app_id` is on your deny list. A pixel click is judged by the focused window, not by the window under the pixel, so it can still land on a denied app's window that doesn't have focus, on another monitor as on the same one. `elements`, `activate_element`, `set_element_text` and a pointer tool aimed at an `element` refuse a denied app's window, and `close_window` and `niri_action`'s `CloseWindow` refuse to close one, whether named or focused. Focus and layout actions, such as `focus` or making a window fullscreen, still reach a denied window: they move it, not what is in it.
- The keyboard tools need `expect`, the window the agent means to type into, and refuse with `focus_mismatch` rather than type elsewhere. They check focus again before each key and stop if it moved. `expect: "none"` skips the check, for a panel or dialog that holds focus outside the windows.
- The pointer tools aim only at pixels of a screenshot the agent took under its lease, at most a minute old, of an output that hasn't changed since. They don't check whether windows moved since the screenshot. They run on any monitor at transform `Normal`, through the screenshot's monitor alone, and refuse a rotated one.
- `shell_panel` opens and closes only three Noctalia panels.

A refusal sends nothing. The [error reference](../../reference/errors/) lists every refusal.

## Evidence screenshots

Every action reports `accepted`, whether niri took the request, and `observed`, what niri's event stream showed afterwards. When the outcome is in doubt, such as a timeout, focus moving to another window, or a lost reply, the result comes with a fresh screenshot of the focused output, so the agent sees what happened instead of guessing. An outcome cut short by a stop or a lost lease comes without one. Each result also says, under `changes`, what niri saw the action change, such as a window that opened or focus that moved. The server never retries an action.

## The audit log

Each call that reaches a tool is appended to `~/.local/state/niri-computer-use/audit.jsonl` (or under `$XDG_STATE_HOME`): the time, the client, the tool, its arguments and the outcome. A call to a tool that isn't listed, or with arguments that don't match the tool's schema, is refused before that and isn't logged. A failed write doesn't stop the call; `status.audit.last_error` shows it. The directory is `0700` and the file `0600`. When the log reaches 10 MiB it is renamed to `audit.jsonl.1` before the next line, and the older ones move up to `audit.jsonl.3`; the oldest beyond that is deleted, so the logs take at most about 40 MiB. Typed and pasted text, clipboard contents, window titles and images are never written; the log keeps their length. Other text the agent gives is written only when it names something the server knows: a preset, a Noctalia panel, an accessibility role, a ref the server issued, or key names XKB knows. Anything else is written as a category and its length, so a window title an agent copies into a field stays out of the log. An `expect` app id is written as its length and whether focus matched it, a screenshot's target as `focused_output`, `output` or `region`, and a save path as its length. Text the agent passes on goes the same way: `niri_action` logs the action's name and field names, its numbers and booleans, and every string, such as a `Spawn` command, as its length in bytes, and JSON that isn't a niri action only as its size; `noctalia` logs how many arguments and how long, and the command's name and description and a plugin's id only when Noctalia's own lists name them. Of an action's `changes`, only counts are written: windows opened, closed and changed, whether focus and the workspace moved, and how many elements were added, removed and changed. A `key` or `type_text` call that failed after some of its input went out, or may have, is written with `may_have_sent: true`.

## What isn't a boundary

- Screenshots, window titles and accessible names reach any connected agent without a lease, even while the screen is locked. Connect only agents you trust with what is on your screen.
- `clipboard_read` needs the lease and returns only what was copied while it was held; what you copied before is withheld as `copied_before_lease`. A copy you make yourself while an agent holds the lease is readable. A copy its owner marks as a secret, such as a password from KeePassXC, is withheld with `reason: "secret"`; a password manager that doesn't mark its copies gets no such protection.
- The deny list follows keyboard focus, and a click lands wherever the pointer is: it can hit a denied window while an allowed app has focus.
- Only `activate_element` and `set_element_text` look for password fields, and only for fields the toolkit marks as such ([`secret_field`](../../reference/errors/#secret_field)). Typing, pasting and clicking into a password field aren't refused.
- Apps choose their own `app_id`, so any app can call itself anything.
- The preset rules refuse shells, interpreters and terminals with arguments, but a wrapper script gets past any list. With `unrestricted` on there are no preset rules.
- You can still type and click while an agent works. The server notices focus moving and stops typing, but nothing keeps you and an agent from moving the pointer at the same time.
- The lease belongs to a server process. Anything that runs as your user can start `niri-computer-use` too.

For a stronger limit, give the agent a separate desktop holding only the apps it may use, such as a nested niri session.
