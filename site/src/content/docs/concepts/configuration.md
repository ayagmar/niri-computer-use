---
title: Configuration
description: The policy file, every key it takes, and the environment the server reads.
sidebar:
  order: 2
---

## The policy file

The server reads `~/.config/niri-computer-use/policy.toml`, or `$XDG_CONFIG_HOME/niri-computer-use/policy.toml`, once when it starts. Restart the agent's session after changing it. Without the file there are no launch presets and no denied apps, and screenshots save under `~/Pictures/niri-computer-use`, which is valid.

`status` shows what the server loaded under `policy`:

| Field | Value |
|---|---|
| `state` | `loaded`, `missing` or `invalid` |
| `presets`, `preset_names` | how many presets there are, and the names `launch` takes |
| `denied_app_ids` | how many apps are on the deny list |
| `turn_on_accessibility` | whether the server may turn the session's accessibility on: false when the file sets it to `false` or is invalid |
| `launch_desktop_entries` | whether `launch` also takes an installed app's desktop entry id |
| `capture_dir` | where `save_path` writes, as the file writes it or `~/Pictures/niri-computer-use` by default, or null when saving is off |
| `capture_dir_source` | `policy` when the file sets `capture_dir`, `""` included, `default` when it doesn't or there is no file, null while the file is invalid |
| `error` | why the file is invalid |

`status.desktop_entries` lists the [desktop entries](#launch_desktop_entries) `launch` would start, and is null while `launch_desktop_entries` is off. `status` also shows `unrestricted` and `confirm_lease`, each with `enabled`, `source` (`policy`, `env`, `both`, or null while off) and `error`, which explains a `NIRI_COMPUTER_USE_UNRESTRICTED` or `NIRI_COMPUTER_USE_CONFIRM_LEASE` value other than `1`. Its `tools` shows the [tool profile](#tools): `profile`, `source` (`policy`, `env`, `both`, or null for `full`) and `error`, which explains a `NIRI_COMPUTER_USE_TOOLS` value that names no profile.

An invalid file doesn't stop the server, but `acquire_desktop` and every action refuse with `read_only` until you fix it and restart. A file with an unknown key is invalid too.

## Full example

```toml
# Input tools refuse while one of these apps has keyboard focus.
deny_input_app_ids = ["org.keepassxc.KeePassXC", "org.gnome.Settings"]

# launch also starts installed apps by their desktop entry id, such as org.gnome.Nautilus.
launch_desktop_entries = true

# Where screenshot's save_path writes PNG files. Without it, ~/Pictures/niri-computer-use;
# "" turns saving off.
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
{"state":"loaded","presets":3,"preset_names":["firefox","terminal","files"],"denied_app_ids":2,"launch_desktop_entries":true,"capture_dir":"~/Pictures/agent-shots","capture_dir_source":"policy","error":null}
```

## Keys

### `deny_input_app_ids`

A list of `app_id`s, default empty. While the window with keyboard focus has one of them, `click`, `drag`, `scroll`, `pointer_move`, `key` and `type_text` refuse with `app_denied`, even with `expect: "none"`. `elements`, `activate_element`, `set_element_text` and a pointer tool aimed at an `element` refuse for a window of a denied app whatever has focus, and so do `close_window` and `niri_action`'s `CloseWindow`, for the window they name or the focused one. Focus and layout actions aren't checked.

The check follows keyboard focus, and apps choose their own `app_id`, so this guards against mistakes. A click can still land on a denied window while an allowed app has focus. `desktop_state` shows each window's `app_id`.

### `turn_on_accessibility`

`true` or `false`, default `true`. Chromium, Electron and CEF apps join the accessibility bus only when the session's accessibility was on when they started. With `true`, when `elements`, an element wait or an action's `element_changes` finds the window of a Chromium, Electron or CEF app off the bus while accessibility is off, the server turns it on by registering a listener with the accessibility registry, as a screen reader does, and accessibility goes back off when the server ends. It never writes the setting itself, and leaves it alone when something else turned it on. With `false`, it does nothing, and `elements` refuses such apps with `not_accessible` until you turn accessibility on yourself, for example by running a screen reader. While the policy file is invalid it does nothing either. Like the rest of the file, it is read when the client's session starts, while the listener belongs to the process serving it. In [shared](#shared) mode that is the engine, which keeps a listener it registered for another client, or for an earlier session, until the engine ends two seconds after its last client: `false` then keeps the server from turning accessibility on again, but doesn't turn it off, and `status` can show `policy.turn_on_accessibility` false beside `accessibility_switch` `on`. Close every client of the niri instance, or set `false` before the first one starts, for it to stay off. See [Chromium and Electron apps](../../tools/elements/#chromium-and-electron-apps).

The server tells such an app from its process, not its `app_id`: the window's process maps Chromium's `icudtl.dat` or `v8_context_snapshot.bin`, as `/proc/<pid>/maps` shows. Any other app off the bus, such as a terminal, a GTK or a Qt app, gets a plain `not_accessible`, and so does a process the server can't inspect, such as another user's: turning accessibility on wouldn't bring them onto the bus. When accessibility turns on, at-spi2-core also sets the GSettings key `org.gnome.desktop.interface toolkit-accessibility` to true, in your dconf database, and nothing sets it back: it stays after the server ends, across sessions. The `not_accessible` detail says so when it happens. at-spi2-core's own autostart and GTK module files name the key: a session that honours them starts the accessibility bus at login and loads `at-spi2-atk` into GTK 3 apps while it is true. To put it back to its default:

```sh
gsettings reset org.gnome.desktop.interface toolkit-accessibility
```

### `capture_dir`

Where `screenshot` saves files, default `~/Pictures/niri-computer-use`. It is an absolute path or one starting with `~/`, resolved against the `HOME` of the client's session, so in [shared](#shared) mode each client saves under its own home. `capture_dir = ""` turns saving off, and `screenshot` then refuses `save_path` with `save_not_enabled`. While the policy file is invalid, saving is off too, since the file might have turned it off.

The directory, the default or yours, is created with mode `0700` if it is missing, along with any missing parent. `save_path` is relative to it, made only of plain names (no `..`, `.` or leading `/`), and names a `.png` file; subdirectories in it must already exist. The server opens subdirectories without following symlinks and creates the file new, with mode `0600`, so a save can't leave the directory or replace a file. See [Screenshots](../../tools/screenshots/#saving-a-screenshot).

### `unrestricted`

`true` or `false`, default `false`. It is off because some agents, such as desktop apps, have no shell of their own, and this server is then the only way they reach your programs. With it on, an agent can run any program through the server.

With `true`:

- `niri_action` also sends the gated actions: `Spawn` and `SpawnSh`, which run any program, `Quit`, monitor power, `LoadConfigFile`, niri's screenshot actions, `ToggleKeyboardShortcutsInhibit`, `SwitchLayout`, the cast actions, the debug toggles and `DoScreenTransition`. Without it they fail with `unrestricted_required`. See [`niri_action`](../../tools/acting/#niri_action).
- The [`noctalia`](../noctalia/#any-command) tool is listed, when Noctalia is installed, and sends any Noctalia command.
- Presets may start terminals with arguments and programs that run commands, and may set `env`.

`NIRI_COMPUTER_USE_UNRESTRICTED=1` in the server's environment turns it on too, so you can allow it for one MCP client only, in that client's server settings, without a policy file. Either one turns it on, and nothing in the environment turns off a file's `true`. Any other value of the variable leaves it off and shows up as `status.unrestricted.error`.

Turning it on never skips the lease, the stop key or the audit log: every gated action still needs the lease, stops at the stop key, and is logged, as metadata like every call that reaches a tool.

### `confirm_lease`

`true` or `false`, default `false`. With `true`, every `acquire_desktop` waits for you to allow it on the desktop before the lease is granted. The server shows a notification through `notify-send`, titled "Allow an agent to use this desktop?", that names the client asking and has Deny and Allow buttons. Only Allow grants the lease. Deny, dismissing the notification, or no answer within 60 seconds refuses the call with `lease_declined`. A session that already holds the lease isn't asked again.

The question comes from the desktop, not from the agent's client, so it still applies when the client approves tool calls on its own, as in Claude Code's bypass mode or Codex's `--yolo`. The agent can't answer it through this server either: until you allow it, it has no lease, so no tool that clicks, types or sends Noctalia commands will run. An agent that has a shell can still reach the notification server or inject input by other means, as it could with any prompt on your desktop.

The notification needs `notify-send` (libnotify) on the client's `PATH` and a notification server that shows actions, such as Noctalia's; `status.binaries` says whether `notify-send` was found. Without one, `acquire_desktop` fails with the `upstream_error` `notify-send` gave, and no lease is granted. Press the stop key, or close the client, while the notification is up and the call is refused at once. The notification then stays up until it expires, and answering it does nothing.

`NIRI_COMPUTER_USE_CONFIRM_LEASE=1` in one client's server settings turns it on for that client only. As with `unrestricted`, either source turns it on and nothing in the environment turns off a file's `true`. Any other value of the variable, such as `true`, shows up as `status.confirm_lease.error` and, unlike `unrestricted`, turns the prompt on: a typo shouldn't drop a safeguard you meant to add.

### `shared`

`true`, `false` or `"required"`, default `true`. With `true`, the server serves its client through the one shared engine of its niri instance, starting the engine if none runs, instead of on its own. Every client then shares one crash guardian, one niri event stream and one accessibility connection, while its policy, `unrestricted`, `HOME`, `PATH` and the directories its desktop entries are in stay its own, read from its own environment as before. Its `grim` runs in its own working directory, as a standalone server's would, even after that directory is renamed or removed, so relative entries on its `PATH` work as they do in its shell.

With `false`, each server serves its own client, with a guardian, event stream and accessibility connection of its own.

With `"required"`, the server serves its client only through the shared engine, so every client of the niri instance is served by one process, with one crash guardian, one niri event stream, one accessibility connection and one audit writer. The lease and the stop flag are shared either way. When the engine can't be reached or refuses the client, the server doesn't serve it on its own: it still lists the client's tools, and every call fails with [`engine_unavailable`](../../reference/errors/#engine_unavailable), whose `detail` says why, such as the engine's socket path and the connect error, or the engine's refusal. The server's stderr says the same. Each call tries the engine again, starting one if none runs, so once the cause is gone, such as an engine of an older build that exited after its last client, the next call is served without restarting the MCP server. Until then it lists the tools without knowing whether the engine finds Noctalia or an accessibility bus. Once a call reaches an engine it sends `notifications/tools/list_changed`, and a client that ignores the notification keeps the first list until the MCP server restarts, so a tool the engine leaves out fails as an unknown tool. A cause that no call can fix, such as a socket path over 108 bytes, is reported once and never tried again: fix it, then restart the MCP server, with `/mcp` in Claude Code or pi, or by restarting Codex. The server starts rather than exits so that the agent learns why: Claude Code, Codex and pi all pass a failed call's `detail` to the agent, while a server that exits at start shows as a failed server in the client's MCP list, and of the three only Claude Code can tell the agent about it.

`NIRI_COMPUTER_USE_SHARED=1`, `0` or `required` in the server's environment overrides the file for that one client. Any other non-empty value is shown in `status.shared.error` and serves the client on its own, and [`status.engine.fallback`](../../tools/looking/) and the server's stderr say why, unless the file says `"required"`: then every call fails with `engine_unavailable` and the variable's error, since a typo shouldn't drop the requirement. For the same reason a policy file with a mistake elsewhere, which is otherwise ignored until you fix it, still requires the shared engine when its `shared` says `"required"`. That holds for a syntax error too, as long as the `shared = "required"` line, or `shared = 'required'`, comes before the file's first table header, such as `[[preset]]`.

The server decides before it reads anything from its client. If no engine answers within five seconds, the engine's socket path in the runtime directory is over the 108 bytes a Unix socket allows, or the engine refuses it, for example because the engine runs another build of `niri-computer-use` or another `WAYLAND_DISPLAY`, it serves its client on its own and says why on stderr, or, with `"required"`, answers every call with `engine_unavailable` as above. An engine newer than 0.1.5 whose file an update replaced doesn't refuse a client of the new build: it exits for it, and the client starts an engine of its build, unless an agent of the old build holds the desktop (see [Updating while agents run](../../start/install/#updating-while-agents-run)). Once it is relaying, it stays a relay: if the engine ends, calls in flight fail with [`engine_lost`](../../reference/errors/#engine_lost), the session's next call reaches a new engine, and while none can be reached calls fail with [`engine_unavailable`](../../reference/errors/#engine_unavailable). The engine writes its errors to `engine.log` in the runtime directory and exits two seconds after its last connection, lease and pending input cleanup have ended. A client that stops reading its server's output loses its server, and with it the lease: once 32 lines wait for it, or one line has waited 30 seconds, the server ends that client's session and exits, saying why on stderr. The same happens to a client that sends more than 32 MiB the engine hasn't taken yet.

### `tools`

`"observe"`, `"act"` or `"full"`, default `"full"`. It decides which tools the server lists. A tool left out isn't in the client's tool list, so it costs no context, and calling it fails as an unknown tool.

| Profile | Tools |
|---|---|
| `observe` | `status`, `desktop_state`, `screenshot`, `wait_for`, and `elements` when there is an accessibility bus. None of them needs the lease, so the agent can look but can't act |
| `act` | every tool but `niri_action`, `noctalia` and `shell_panel` |
| `full` | every tool, as Noctalia, `unrestricted` and the accessibility bus allow |

`clipboard_read` isn't in `observe` because it needs the lease, which `observe` can't take.

`NIRI_COMPUTER_USE_TOOLS` in the server's environment names a profile too, so one MCP client can list fewer tools than another. The narrower of the file's profile and the variable's wins: either can take tools away and neither can add them back. A value other than `observe`, `act` or `full` lists only `observe`'s tools and shows up as `status.tools.error`.

### `launch_desktop_entries`

`true` or `false`, default `false`. With `true`, `launch` also takes the desktop entry id of an installed app, such as `org.gnome.Nautilus` or `firefox`: the name of its `.desktop` file without the extension. A [preset](#preset) with the same name wins. The server reads the entry each time `launch` names it, so an app installed or changed since the server started is found, and niri starts the entry's `Exec` line as an argv, as it does a preset's `argv`: no shell runs it, and no files or URLs are passed. While the key is off, no entry file is read.

The server reads an entry only from a file root owns that neither its group nor anyone else may write, reached from `/` through directories that are the same, along every symlink on the way. A directory such as `/tmp`, which anyone may add to but where only a name's owner may rename it, counts for the names in it that root owns. A symlink in a directory you can write is refused, whatever it leads to, and so is a file under `/proc`, `/sys` or `/dev`, or anything on a FUSE filesystem, whose daemon chooses the owners it reports. That takes the apps installed for the whole system, in `/usr/share`, `/usr/local/share` and flatpak's `/var/lib/flatpak/exports/share`, and refuses entries in `~/.local/share/applications` and `~/.local/share/flatpak/exports`: you, and any program you run, can write those, and an entry's `Exec` line is a command. A user-writable entry needs a preset, so an app installed with `flatpak install --user`, or an entry you copied into `~/.local/share/applications` to change it, starts only through a [preset](#preset).

`status.desktop_entries` has `count`, how many entries `launch` would start, `ids`, the first 64 of their ids in order, and `user_writable`, how many entries `launch` refuses because you can write them. For an id `launch` doesn't find, `unknown_preset`'s `detail` names up to five entries whose id or `Name` contains it, ignoring case, so `files` finds `org.gnome.Nautilus (Files)`.

The server looks where an app menu in the client's session would, in this order, and the first directory that has the id wins:

1. `$XDG_DATA_HOME/applications`, or `~/.local/share/applications` when it is unset
2. `applications` in each directory of `$XDG_DATA_DIRS`, or `/usr/local/share` and `/usr/share` when it is unset

An entry in a subdirectory has its path as its id, with `/` as `-`: `kde4/okular.desktop` is `kde4-okular`. A directory that is missing or can't be read is skipped. Flatpak apps are found when `XDG_DATA_DIRS` names flatpak's `exports/share` directories, as a session with flatpak installed sets it; a client that starts its server without `XDG_DATA_DIRS` sees only `/usr/local/share` and `/usr/share`.

`launch` refuses an entry with [`entry_refused`](../../reference/errors/#entry_refused) when it:

- is in a file or directory you can write, as above
- has `Hidden=true` or `NoDisplay=true`
- isn't `Type=Application`
- isn't shown on this desktop by its `OnlyShowIn` or `NotShowIn`, read against `$XDG_CURRENT_DESKTOP`, or `niri` when it is unset
- has a `TryExec` that isn't on the client's `PATH`
- has `Terminal=true`. It needs a terminal, and a terminal started with a command is what the preset rules refuse
- has `DBusActivatable=true` and no `Exec` line. With an `Exec` line, that line is started, as other launchers do
- is malformed, or has an `Exec` line written for a shell: an unquoted `|`, `;`, `>`, `$`, `~`, `'` or another character only a shell reads, an unescaped `$` or `` ` `` in quotes, or a field code the spec doesn't define
- starts a program the [preset rules](#preset) refuse. They apply even with `unrestricted` on, and `env` is one of those programs, so `Exec=env NAME=value app` doesn't start. A preset with `env` does, under `unrestricted`
- runs `flatpak` other than as `flatpak run` of an app. Flatpak's own exported entries name the app's own command, as in `--command=firefox`, so an entry may give one `--command=`, which the preset rules refuse, as long as it doesn't name a program those rules refuse or a terminal. Before the app only `--user`, `--system`, `--installation=`, `--arch=`, `--branch=`, `-v`, `--verbose`, `--ostree-verbose`, `--file-forwarding` and `--no-desktop` may come too. Any other option, such as `--filesystem`, `--talk-name`, `--socket`, `--env`, `--runtime` or `--devel`, changes the app's sandbox or what runs in it and is refused, as is a second `--command`. Arguments after the app are passed to the app

In the `Exec` line, `%f`, `%F`, `%u` and `%U` are dropped, `%i` becomes `--icon` and the entry's `Icon`, `%c` its `Name`, `%k` the file's path and `%%` a `%`; the deprecated codes are dropped too.

An entry is refused with `app_denied` when [`deny_input_app_ids`](#deny_input_app_ids) has its id, its `StartupWMClass`, the flatpak app its `Exec` line runs, such as `org.keepassxc.KeePassXC` in `flatpak run org.keepassxc.KeePassXC`, or the base name of its program, such as `keepassxc` in `/usr/bin/keepassxc`. An app whose windows have another `app_id` can't be caught before it starts; once its window has focus, input to it is refused as for any window.

`launch` watches for windows whose `app_id` is the entry's id, its `StartupWMClass` or the flatpak app it runs. An app whose windows have none of these still starts, but `launch` reports `observed: none` after five seconds; a preset with the right `app_id` fixes that. `desktop_state` shows each window's `app_id`.

This sits between presets and [`unrestricted`](#unrestricted). The agent can start only apps installed for the whole system, each with the command its entry fixes, but it can start any of them, a terminal without arguments included, as a preset may. A program running as you can't add an entry, since entries you can write are refused.

### `[[preset]]`

One table per app `launch` may start. `name`, `argv` and `app_id` are required:

| Key | Value |
|---|---|
| `name` | what the agent passes to `launch`; unique and not empty |
| `argv` | the program and its fixed arguments, started by niri as given; no shell runs it |
| `app_id` | the `app_id` its windows have, so `launch` can see them appear and `reuse` can find them |
| `env` | only with [`unrestricted`](#unrestricted): a table of variables added to the app's environment, such as `env = { GDK_SCALE = "2" }`. niri starts `env -- NAME=value … argv`, because niri's spawn takes no environment |

`launch` takes only the `name`; the agent can't add arguments or variables. Unless `unrestricted` is on, a preset may not start:

- a shell, an interpreter, `env`, `sudo`, `doas`, `pkexec`, `systemd-run`, `nohup`, `setsid`, `xargs`, `timeout`, `nice`, `busybox`, `uwsm`, `distrobox`, `toolbox`, `tmux`, `screen`, `bwrap`, `firejail`, `unshare`, `chroot`, `ssh`, `flatpak-spawn`, `nix`, `nix-shell`, `strace`, `gdb`, `script`, `docker`, `podman`, or another program that runs any command it is given; versioned names such as `python3.13` count too
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
| `NIRI_SOCKET` | niri's IPC socket. With every symlink resolved, it also names the niri instance that the lease, the stop flag and the input-dirty marker belong to: they are in `niri-computer-use/<instance>/` beside the socket, where `<instance>` is its file name without `.sock`. A socket with a hard link, which would be another name for the same niri, gets no lease by any name |
| `XDG_RUNTIME_DIR` | finding niri's socket, the display and the session bus |
| `WAYLAND_DISPLAY` | everything that reaches the display: input, screenshots, pasting, `clipboard_read`, and finding Noctalia's socket |
| `DBUS_SESSION_BUS_ADDRESS` | finding the accessibility bus for `elements`; without it, `$XDG_RUNTIME_DIR/bus` |
| `PATH` | `grim`, with [`confirm_lease`](#confirm_lease) `notify-send`, and a desktop entry's `TryExec`, for this client's calls |
| `NIRI_COMPUTER_USE_DESKTOP_PATH` | where to look for `loginctl` and `noctalia`, as a `PATH`-style list of absolute directories; unset or empty looks in `/usr/local/bin:/usr/bin:/bin:/run/current-system/sw/bin`. Only absolute directories count, each once, and a trailing slash makes no difference. In [shared](#shared) mode, a client that sets other directories than the engine's, or the same ones in another order, is served standalone |
| `XDG_CONFIG_HOME`, else `HOME` | the policy file |
| `XDG_STATE_HOME`, else `HOME` | the audit log |
| `HOME` | a `capture_dir` that starts with `~/`, and the default one |
| `XDG_DATA_HOME`, `XDG_DATA_DIRS`, else `HOME` | where [desktop entries](#launch_desktop_entries) are, with `launch_desktop_entries` |
| `XDG_CURRENT_DESKTOP` | an entry's `OnlyShowIn` and `NotShowIn`; `niri` when unset |
| `NIRI_COMPUTER_USE_UNRESTRICTED` | `1` turns on [`unrestricted`](#unrestricted) for this server; unset or empty leaves the policy file to decide |
| `NIRI_COMPUTER_USE_CONFIRM_LEASE` | `1` turns on [`confirm_lease`](#confirm_lease) for this server; unset or empty leaves the policy file to decide |
| `NIRI_COMPUTER_USE_TOOLS` | `observe`, `act` or `full`: the [tool profile](#tools) for this server; unset or empty leaves the policy file to decide |
| `NIRI_COMPUTER_USE_SHARED` | `1` turns on [`shared`](#shared) mode for this server, `0` turns it off and `required` requires the shared engine; unset or empty leaves the policy file to decide |

The lock state comes from logind's session for niri's own process, which the server finds through niri's socket, so the server's own `XDG_SESSION_ID` doesn't matter.
