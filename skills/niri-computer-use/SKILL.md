---
name: niri-computer-use
description: "Operates the user's niri Wayland desktop through the niri-computer-use MCP server: reads windows, workspaces and outputs, takes screenshots, and while holding the desktop lease focuses windows, launches preset apps, clicks, scrolls, drags, types and opens Noctalia panels. Load this before the first call to any niri-computer-use tool (status, screenshot, desktop_state, acquire_desktop, click, key, type_text and the rest), and whenever the user asks to look at their screen, check or test a GUI app, click or type in a window, switch windows or workspaces, or otherwise drive their Linux desktop, even if they don't mention niri."
license: MIT
compatibility: Needs the niri-computer-use MCP server (niri-computer-use serve) registered in the agent, running inside a niri 26.04 session.
---

# Operating the niri desktop

The `niri-computer-use` MCP server shows you the user's real desktop and, while you hold its lease, lets you act on it. Everything you do happens on the screen the user is working on, and they can take the desktop back at any moment with a stop key. The server's checks are guardrails, not a sandbox: a click or a key can do anything the user could do.

In Claude Code the tools are named `mcp__niri-computer-use__<tool>`; this skill uses the short names. Use them for everything on the desktop. Running `grim`, `niri msg`, `wtype`, `wl-paste` or `noctalia msg` yourself, or starting apps from a shell, skips the lease, the stop key and the audit log the user relies on.

## Looking

Start with `status`: it says whether the screen is locked, whether Noctalia runs, which launch presets exist and who holds the lease. Then prefer structured data, which is exact and cheap, to screenshots:

- `desktop_state` for windows (ids, `app_id`, title), workspaces, the focused window
- `outputs` for monitors, `shell_status` for Noctalia's panels, `clipboard_read` for copied text

Take a `screenshot` when you need pixels. For small text, take a `region` screenshot around it rather than guessing from a downscaled full screen. Report what you saw separately from what you infer.

## Acting

Acting needs the lease. Take it with `acquire_desktop` only when the user asked you to act, then work in a loop:

1. Look: `desktop_state`, and a fresh `screenshot` when pixels matter.
2. Do one action.
3. Read its result: `accepted` says whether niri or Noctalia took the request, `observed` what happened. `sent` only means the input arrived; it says nothing about what the app did with it.
4. Look again before the next action.

Wait for each call's result before the next call. A screenshot sent alongside an action can be taken before the action lands and show you the old screen, and an action sent alongside another can land on whatever the first one changed.

Prefer structured actions to input, because they can't land on the wrong thing: `focus_window` and `focus_workspace` with ids from `desktop_state`, `launch` with a preset name, `close_window`, `shell_open` and `shell_close`. Use the pointer and keyboard for what happens inside an app.

When three actions in a row change nothing toward the goal, stop and tell the user what you see.

## Pointer and keyboard

- Pointer tools aim at pixels of a screenshot: pass that screenshot's `screenshot_ref` and the pixel. Use your latest screenshot, and take a new one after anything that may have moved the screen. A `ref_invalid` error means exactly that.
- Keyboard tools take `expect`, the window you mean to type into: `{"window_id": …}` or `{"app_id": "…"}`. The server refuses with `focus_mismatch` rather than typing into another window. Use `"none"` only for a Noctalia panel or dialog that holds the keyboard, after a screenshot shows it's ready.
- `key` sends the app's own shortcuts. niri's keybinds don't fire from it, so don't try to switch windows or start apps with keys.
- Never type a password or other secret unless the user gave it to you for that.

### Typing text, then sending it

`type_text` takes up to 1000 characters in one call and sends them in parts of 100, checking between parts that focus stayed on your window. If focus moves, it stops: `observed` is `interrupted` and `typed` says how many characters went out. A failed call's `detail` says the same.

Pressing Enter sends whatever is in the box. So press it only after a `type_text` that came back `sent` without a `typed` field. Otherwise look at a screenshot and finish or fix the text first: a half-typed message that gets sent can't be taken back.

## Outcomes that need care

- `timeout`, `pending`, `none` or `uncertain`: something may still be happening. The result already carries a fresh screenshot of the focused output; look at it before anything else.
- Never repeat a `launch` or `close_window` on your own. A second launch opens a second window, and a second close can answer the app's "save changes?" dialog.
- `interrupted`: someone else moved focus while you waited. The user is probably using the desktop; stop and tell them.

## When to stop and hand back

Stop and tell the user, quoting the error's `detail`, when a tool returns `stopped`, `recovery_required`, `screen_locked`, `lease_held`, `read_only`, `app_denied` or `untested_output_config`. These are the user's decisions: `resume` and `recover` are their commands, and only they can unlock the screen or free the lease. Don't call the refused action again.

If the app the user wants has no launch preset (`status` lists `policy.preset_names`), say so and ask them to add one to their policy file. Don't look for another way to start it: keys and typing would land in whatever window has focus.

## Giving the desktop back

When you are done, put focus back on the window the user was on (note `focused_window` from `desktop_state` before you start), close any shell panel you opened, and call `release_desktop`. The user is often away from the screen while you work and comes back to whatever you left focused.

## Examples

Sending a message to the chat window 42:

```
desktop_state                         → focused_window 42, app_id "chat"
acquire_desktop
type_text {text: <the message>, expect: {window_id: 42}}  → sent, no `typed`
screenshot                            → the whole message is in the box
key {combo: "Return", expect: {window_id: 42}}            → sent
screenshot                            → the message was sent
release_desktop
```

Clicking a button in another window and coming back:

```
desktop_state             → the user is on window 42; the target is window 7
acquire_desktop
focus_window {id: 7}      → focused
screenshot                → shot-3, the button at (412, 230)
click {screenshot_ref: "shot-3", x: 412, y: 230}  → sent
screenshot                → the click had its effect
focus_window {id: 42}     → focused
release_desktop
```

## Reference

- [references/tools.md](references/tools.md): every tool's arguments and results, screenshot targets, and what each `observed` value means. Read it when a tool's own description leaves you unsure.
- [references/errors.md](references/errors.md): every error name, what caused it and what to do. Read it when a call fails with an error not covered above.
