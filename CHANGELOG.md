# Changelog

All notable changes to this project are documented here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## 0.1.0 - unreleased

The first release: an MCP server over stdio that lets an AI agent read and act on a niri 26.04 desktop.

### Upgrading

- If you run a source build of `main`, end every running `niri-computer-use` process (servers, bridges and shared engines) before installing this one, then start your clients again. `pgrep -a '^niri-computer-u'` lists them. Older builds of `main` kept their runtime directory under `XDG_RUNTIME_DIR`, so an old and a new server on one niri would share neither the lease nor the stop flag. Tools were also merged and renamed during development; link or copy `skills/niri-computer-use` again so your agents read the current names.

### Added

- `niri-computer-use serve`, an MCP server over stdio, and the commands `status`, `stop`, `resume`, `recover` and `--version`.
- 22 tools. Reading, with no lease: `status`, `desktop_state`, `screenshot`, `elements` and `wait_for`. The lease: `acquire_desktop` and `release_desktop`. Under the lease: `clipboard_read`, and the tools that act, `focus`, `launch`, `close_window`, `niri_action`, `pointer_move`, `click`, `drag`, `scroll`, `key`, `type_text`, `activate_element`, `set_element_text`, and with Noctalia installed `shell_panel`, and `noctalia` when `unrestricted` is on as well. `elements`, `activate_element` and `set_element_text` are listed when the session has an accessibility bus.
- `desktop_state`: windows, workspaces, focus, the overview, keyboard layouts and outputs from niri's event stream, and with Noctalia installed its status as `shell`.
- `screenshot`: one output or a region inside one, through `grim`, as JPEG or PNG, at most 1280 pixels wide unless `max_width` says otherwise. A region of an earlier screenshot, named by its `screenshot_ref`, is captured again at full resolution to read small text. `save_path` writes a PNG under `~/Pictures/niri-computer-use`, or the policy's `capture_dir`, never through a symlink and never over an existing file.
- `elements`: one window's accessible elements over AT-SPI, one line each with an `element_ref`, role, name, states, actions and box in layout coordinates. `activate_element` runs an element's own action and `set_element_text` replaces a field's text, both through the accessibility bus. The pointer tools take an `element` in place of screenshot pixels.
- `wait_for`: waits for a window to appear, close or change its title, or for the screen to stop changing, for up to 30 seconds.
- `focus` a window or workspace, `launch` an app from a policy preset, optionally reusing an open window, and `close_window`. `niri_action` sends any niri action in niri's IPC JSON, such as fullscreen, floating or a column's width, and returns the window's state after it.
- Pointer input through niri's virtual pointer (`pointer_move`, `click`, `drag`, `scroll`), aimed at a screenshot's pixels through a `screenshot_ref` that expires after 60 seconds or when its output changes.
- Keyboard input through `wtype` into the window named in `expect`: `key` presses up to 16 combinations, and `type_text` types up to 1000 characters in parts of 100, with focus and the lock state checked again before each part. `submit: true` presses Enter only once all of the text went out. `via: "paste"` pastes up to 1 MiB through the clipboard and puts the clipboard back afterwards. `NIRI_COMPUTER_USE_KEYBOARD=native` selects an experimental backend on niri's virtual keyboard that checks focus before every key.
- Every action result says whether niri `accepted` the action and what was `observed` after it. `changes` lists what niri saw change, such as windows opened, closed or moved and focus moving, and `element_changes` lists the accessible elements that changed. `screenshot: true` adds a screenshot taken once the screen stopped changing.
- `clipboard_read`: the clipboard's text, under the lease.
- `shell_panel` opens and closes the Noctalia panels `control-center`, `wallpaper` and `tray-drawer`. `noctalia` sends any Noctalia command and is listed only with `unrestricted` on.
- A policy file at `$XDG_CONFIG_HOME/niri-computer-use/policy.toml` with launch presets (`[[preset]]` with `name`, `argv`, `app_id` and `env`), `deny_input_app_ids`, `capture_dir`, `unrestricted`, `confirm_lease`, `shared` and `tools`. `NIRI_COMPUTER_USE_UNRESTRICTED`, `NIRI_COMPUTER_USE_CONFIRM_LEASE`, `NIRI_COMPUTER_USE_SHARED` and `NIRI_COMPUTER_USE_TOOLS` set the same for one client.
- Tool profiles: `tools = "observe"` lists only the five reading tools, `"act"` leaves out `niri_action`, `noctalia` and `shell_panel`, and `"full"`, the default, lists every tool.
- A shared engine per niri instance, on by default: each client's `serve` relays to one `niri-computer-use engine`, started in its own systemd user scope where systemd allows it. Policy, keyboard backend, `PATH` and working directory stay per client.
- Session discovery: with no `XDG_RUNTIME_DIR`, `NIRI_SOCKET` or `WAYLAND_DISPLAY` from the client, the server takes them from the user's only running niri, so no client needs environment settings.
- The `niri-computer-use` skill in `skills/`, which tells agents how to use the tools, with tool and error tables in `references/`.
- An audit log of every tool call at `$XDG_STATE_HOME/niri-computer-use/audit.jsonl`, with argument metadata and outcomes but no typed text, clipboard, window titles or images, moved aside at 10 MiB with three old logs kept.
- Failures carry a stable error name and the upstream detail; the skill's `references/errors.md` and the documentation site list them.
- The documentation site, with `llms.txt` and a Markdown address for every page.

### Security

- One server at a time holds the lease on a niri instance, an exclusive `flock` in a runtime directory beside niri's socket. Only the holder acts; another session's `acquire_desktop` fails with `lease_held`.
- `niri-computer-use stop`, meant for a niri bind, cancels the running action and takes the lease back until `niri-computer-use resume`. It doesn't connect to niri, so it works while niri hangs.
- Every action goes out within 250 ms of a read of the lock state, from Noctalia and logind, that said unlocked. A locked or unknown state fails with `screen_locked`, and a read that got too old before the call could go out fails with `lock_stale`.
- With `confirm_lease` on, `acquire_desktop` waits for you to choose Allow on a desktop notification sent with `notify-send`. A denial, a dismissal or no answer within 60 seconds fails with `lease_declined`.
- An input-dirty marker is written before input and removed once the input is released. A crash mid-input leaves `recovery_required` until you run `niri-computer-use recover`. A crash guardian beside each server releases held native keys and pointer buttons when the server dies.
- Keyboard and pointer input refuse with `app_denied` while an app on the deny list has focus, and element actions and `close_window` refuse on its windows. `set_element_text` and `activate_element` refuse fields marked as passwords with `secret_field`.
- `launch` only starts presets, and a preset can't start a program that runs whatever follows it, such as `tmux`, `ssh`, `flatpak-spawn` or `docker`. `niri_action` refuses actions that run programs, write files or reach past the layout, such as `Spawn` and `DoScreenTransition`, with `unrestricted_required`. `unrestricted`, off by default, allows both, and lists `noctalia`.
- `clipboard_read` returns only what was copied while the lease was held (`copied_before_lease` otherwise), and never text its owner marked secret for clipboard managers, such as a password copied from KeePassXC (`secret`).
- The name an MCP client gives itself is cut to 64 characters of ASCII letters, digits, spaces and `._-+@` before it labels the audit log, the lease holder and the `confirm_lease` prompt.
