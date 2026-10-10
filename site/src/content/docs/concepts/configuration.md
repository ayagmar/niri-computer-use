---
title: Configuration
description: The policy file, every key it takes, and the environment the server reads.
sidebar:
  order: 2
---

## The policy file

The server reads `~/.config/niri-computer-use/policy.toml`, or `$XDG_CONFIG_HOME/niri-computer-use/policy.toml`, once when it starts. Restart the agent's session after changing it. Without the file there are no launch presets, no denied apps and no saved screenshots, which is valid.

`status` shows what the server loaded under `policy`:

| Field | Value |
|---|---|
| `state` | `loaded`, `missing` or `invalid` |
| `presets`, `preset_names` | how many presets there are, and the names `launch` takes |
| `denied_app_ids` | how many apps are on the deny list |
| `capture_dir` | as the file writes it, or null when saving is off |
| `error` | why the file is invalid |

`status` also shows `unrestricted`: `enabled`, `source` (`policy`, `env`, `both`, or null while off) and `error`, which explains a `NIRI_COMPUTER_USE_UNRESTRICTED` value other than `1`.

An invalid file doesn't stop the server, but `acquire_desktop` and every action refuse with `read_only` until you fix it and restart. A file with an unknown key is invalid too.

## Full example

```toml
# Input tools refuse while one of these apps has keyboard focus.
deny_input_app_ids = ["org.keepassxc.KeePassXC", "org.gnome.Settings"]

# Where screenshot's save_path writes PNG files. Without it, saving is off.
capture_dir = "~/Pictures/agent-shots"

[[preset]]
name = "firefox"
argv = ["firefox"]
app_id = "firefox"

[[preset]]
name = "terminal"
argv = ["foot"]
app_id = "foot"

[[preset]]
name = "files"
argv = ["nautilus", "--new-window"]
app_id = "org.gnome.Nautilus"
```

With this file, `status` reports:

```json
{"state":"loaded","presets":3,"preset_names":["firefox","terminal","files"],"denied_app_ids":2,"capture_dir":"~/Pictures/agent-shots","error":null}
```

## Keys

### `deny_input_app_ids`

A list of `app_id`s, default empty. While the window with keyboard focus has one of them, `click`, `drag`, `scroll`, `pointer_move`, `key`, `type_text` and `paste` refuse with `app_denied`, even with `expect: "none"`. `elements` refuses for a window of a denied app whatever has focus.

The check follows keyboard focus, and apps choose their own `app_id`, so this guards against mistakes. A click can still land on a denied window while an allowed app has focus. `desktop_state` shows each window's `app_id`.

### `capture_dir`

Where `screenshot` may save files, default unset. It is an absolute path or one starting with `~/`. Without it, `screenshot` refuses `save_path` with `save_not_enabled`.

The directory is created with mode `0700` if it is missing. `save_path` is relative to it, made only of plain names (no `..`, `.` or leading `/`), and names a `.png` file; subdirectories in it must already exist. The server opens subdirectories without following symlinks and creates the file new, with mode `0600`, so a save can't leave the directory or replace a file. See [Screenshots](../../tools/screenshots/#saving-a-screenshot).

### `unrestricted`

`true` or `false`, default `false`. It is off because some agents, such as desktop apps, have no shell of their own, and this server is then the only way they reach your programs. With it on, an agent can run any program through the server.

With `true`:

- `niri_action` also sends the gated actions: `Spawn` and `SpawnSh`, which run any program, `Quit`, monitor power, `LoadConfigFile`, niri's screenshot actions, `ToggleKeyboardShortcutsInhibit`, `SwitchLayout`, the cast actions and the debug toggles. Without it they fail with `unrestricted_required`. See [`niri_action`](../../tools/acting/#niri_action).
- The [`noctalia`](../noctalia/#any-command) tool is listed, when Noctalia is installed, and sends any Noctalia command.
- Presets may start terminals with arguments and programs that run commands, and may set `env`.

`NIRI_COMPUTER_USE_UNRESTRICTED=1` in the server's environment turns it on too, so you can allow it for one MCP client only, in that client's server settings, without a policy file. Either one turns it on, and nothing in the environment turns off a file's `true`. Any other value of the variable leaves it off and shows up as `status.unrestricted.error`.

Turning it on never skips the lease, the stop key or the audit log: every gated action still needs the lease, stops at the stop key, and is logged with its arguments.

### `[[preset]]`

One table per app `launch` may start. `name`, `argv` and `app_id` are required:

| Key | Value |
|---|---|
| `name` | what the agent passes to `launch`; unique and not empty |
| `argv` | the program and its fixed arguments, started by niri as given; no shell runs it |
| `app_id` | the `app_id` its windows have, so `launch` can see them appear and `reuse` can find them |
| `env` | only with [`unrestricted`](#unrestricted): a table of variables added to the app's environment, such as `env = { GDK_SCALE = "2" }`. niri starts `env -- NAME=value … argv`, because niri's spawn takes no environment |

`launch` takes only the `name`; the agent can't add arguments or variables. Unless `unrestricted` is on, a preset may not start:

- a shell, an interpreter, `env`, `sudo`, `doas`, `pkexec`, `systemd-run`, `nohup`, `setsid`, `xargs`, `timeout`, `nice`, `busybox`, `uwsm`, `distrobox`, `toolbox`, or another program that runs any command it is given; versioned names such as `python3.13` count too
- `flatpak` with `--command`
- a terminal with arguments, not even `--app-id`, because terminals run trailing arguments as a command. A terminal without arguments is fine. To give a terminal its own `app_id`, start a desktop file with `gtk-launch`.

A file that breaks a rule is invalid, and `status` names the preset:

```json
{"state":"invalid", …, "error":"…/policy.toml: preset \"shell\" starts the terminal foot with arguments, which it can run as a command"}
```

These rules catch common mistakes. A wrapper script or a symlink with another name gets past any list, so they are a guardrail, not a boundary. The full lists are `COMMAND_RUNNERS` and `TERMINALS` in [`src/policy.rs`](https://github.com/ayagmar/niri-computer-use/blob/main/src/policy.rs).

## Environment

The server reads these once, at startup. When `XDG_RUNTIME_DIR`, `NIRI_SOCKET` or `WAYLAND_DISPLAY` is unset, it finds your session itself, as described in [Session variables](../../start/clients/#session-variables); a set variable always wins.

| Variable | Used for |
|---|---|
| `NIRI_SOCKET` | niri's IPC socket. Its file name also names the niri instance that the lease and stop flag belong to |
| `XDG_RUNTIME_DIR` | the lease, the stop flag and the input-dirty marker, in `niri-computer-use/<instance>/`, where `<instance>` is the socket's file name without `.sock` |
| `WAYLAND_DISPLAY` | the pointer tools, `paste`, and finding Noctalia's socket |
| `DBUS_SESSION_BUS_ADDRESS` | finding the accessibility bus for `elements`; without it, `$XDG_RUNTIME_DIR/bus` |
| `PATH` | `grim`, `wl-paste`, `wl-copy`, `wtype`, `loginctl` and `noctalia` |
| `XDG_CONFIG_HOME`, else `HOME` | the policy file |
| `XDG_STATE_HOME`, else `HOME` | the audit log |
| `HOME` | a `capture_dir` that starts with `~/` |
| `NIRI_COMPUTER_USE_UNRESTRICTED` | `1` turns on [`unrestricted`](#unrestricted) for this server; unset or empty leaves the policy file to decide |
| `NIRI_COMPUTER_USE_KEYBOARD` | the keyboard backend: unset or `wtype` for the default, `native` for the [experimental native backend](../../tools/keyboard/#the-native-backend) |

The lock state comes from logind's session for niri's own process, which the server finds through niri's socket, so the server's own `XDG_SESSION_ID` doesn't matter.
