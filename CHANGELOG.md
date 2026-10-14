# Changelog

All notable changes to this project are documented here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## Unreleased

### Added

- `wait_for` waits on a window's accessible elements: `{"element": {"window_id", "role", "name_contains", "state", "absent"}}` waits until an element `elements` would list, with that role, name text and state (a name from a JSON listing's `states`, or `disabled` for one without `sensitive`), is there, or with `absent: true` until none is. A met wait for an element that appeared returns its `element_ref` while you hold the lease, with `unbound` or `unmappable` when `elements` would give them, and no ref, with the reason in `detail`, when a walk cut short can't tell it from a look-alike further on. A window that closes ends the wait as `window_gone`, and one whose app isn't on the accessibility bus as `not_accessible`. A walk cut short never counts as proof that the element is gone. The audit log keeps the role and state names and the length of `name_contains`, never the text.
- `activate_element` and `set_element_text` return `effect`: `seen` when reading the element back showed the action's effect, `differs` when a field holds other text than was sent, `none_seen` when nothing the server can read changed, and `unverified` when it couldn't be read. A read that fails after the action went out is `unverified` with the reason in `detail`, never an error. The audit log keeps `effect` beside `observed`.
- `activate_element` reads the value of an element that has one, such as a slider or spin button, before and after, and says in `element.value_changed` whether it changed, never the value. It also compares the element's name, and says in `element.name_changed` whether a button relabelled itself, never either name.
- When `elements`, `wait_for`'s element condition or an action's `element_changes` finds the window of a Chromium, Electron or CEF app off the accessibility bus and the session's accessibility is off, the server turns it on by registering an AT-SPI event listener, as a screen reader does, and holds it on until the server ends, however it ends. Chromium, Electron and CEF apps join the bus only when it was on when they started, so apps started after that are listed; one already running needs a restart, and `not_accessible`'s detail says to ask the user first. The server tells such an app by the files Chromium maps (`icudtl.dat` or `v8_context_snapshot.bin` in `/proc/<pid>/maps`); for any other app off the bus, such as a terminal, or a process it can't inspect, it changes nothing and gives the plain `not_accessible`. The server never writes `org.a11y.Status`, leaves accessibility alone when something else turned it on, and needs at-spi2-core 2.61 or later: with an older one it lets go and says so. `turn_on_accessibility = false` in the policy file turns this off. `status.accessibility_switch` says what the server did (`off`, `on`, `turning_on`, `unsupported`, `let_go`), and `status.policy.turn_on_accessibility` whether it may. The launcher also sets GSettings' `toolkit-accessibility` to true, as it does when a screen reader starts, and that stays after the server ends; `gsettings reset org.gnome.desktop.interface toolkit-accessibility` undoes it.
- `elements` asks a Chromium window for its page's tree when it shows none, as Chromium builds it only once an assistive technology asks. That listing has `building: true`, or `the app built its tree just now` in the compact header: list again a second later for the page.

### Changed

- `elements` reads the elements inside a container of more than 64 children, such as a long list, table or grid, after the rest of the window, and lists them after it. A window whose walk is cut short inside such a list still lists its own controls, such as the toolbar's and the buttons below the list, where it used to stop before them; those outside a named list row are `unbound: cut_short`, since the unread part may hold a look-alike.
- `elements` reads up to 20,000 objects of a window's tree, up from 2,000. One long list, table or grid used to stop the walk before the rest of the window, so its other controls were never listed, or were `unbound: cut_short`. In nested GTK 3, GTK 4 and Chromium windows of 2,218 to 15,022 objects, the whole tree now reads in 0.15 to 0.82 s and every control outside the list has a ref that acts; listing such a window takes that long. A window too large to read in the three seconds is still cut short.
- `type_text` refuses text, typed or pasted, that ends with tool-call closing tags such as `</parameter>` or `</invoke>` that no opening tag earlier in the text balances, each opening tag balancing one closing tag, with `leaked_tag` and the tags named in `detail`. Models sometimes leak their own tool-call markup into the end of a long argument. Nothing is typed or pasted; send the text again without the tags.
- `set_element_text` reads the field's text back and compares it with the text sent, instead of comparing only the character count. A field that reformats or masks text without changing its length is now `differs`, not `matched`. Neither text is returned or logged.

### Fixed

- `elements` no longer binds a ref by a count of look-alike elements it didn't finish. A control outside any list row whose twin, with the same role and name, lay past the listing's `limit` got a ref that could act on the wrong record, as with two `Delete` buttons of a list put on the bus without rows. The walk now reads on past `limit` to the end of the window's tree, so such a control is `unbound: repeated`. When the walk still didn't read the whole tree, cut short at its 2,000 objects or its three seconds, or because part of the tree went away while it read, a control in no row is `unbound: cut_short`, and every tool refuses its ref: aim at its pixels. In a window that large, only elements in a named list row keep a ref that acts. A listing now reads the whole tree whatever its `limit` and filters: about 55 ms for 600 objects and 190 ms at the cap in a nested GTK 4 window, where a listing that stopped at its limit took 2 to 13 ms.
- A screenshot or element ref spelled with a sign or leading zeros in its number, such as `shot-<tag>-+1` or `shot-<tag>-01`, no longer names the ref issued as `shot-<tag>-1`. Such a ref worked while the audit log recorded it as `invalid_ref`; it is now `unknown_ref` or `element_stale`, as any ref the server didn't issue.

## 0.1.1 - 2026-10-14

### Added

- `install.sh` offers to register the server in each of Claude Code, Pi and Codex it finds on `PATH`, through the agent's own `mcp add` command, asking per agent when its output is a terminal. `NCU_REGISTER=1` registers in all of them without asking, `NCU_REGISTER=0` in none. An agent that refuses doesn't fail the install; its command is printed under the next steps.

### Changed

- `key`, `type_text` and a paste's key always go through the server's own virtual keyboard with the compositor's keymap, and `click`, `drag` and `scroll` take held `keys` without configuration. In Chromium and Electron apps, wtype's keys lost spaces and punctuation and missed shortcuts such as VS Code's `ctrl+s`; the native keyboard types text your layout has exactly. Symbols your layout lacks, such as accented letters, Greek, Cyrillic or arrows on a US layout, arrive exactly there too, because native puts them on keycodes Chromium knows, F13 to F24 among them. Emoji and other characters outside the Basic Multilingual Plane still arrive as other characters there, so paste them.
- Native text with more distinct symbols the layout lacks than the keymap has spare keycodes, 14 in a US layout, is typed in parts, each with a temporary keymap of its own, instead of refused.
- A native `key` combination, a paste's key included, that the active layout lacks goes out in another of your layouts that has it, so `ctrl+v` and `ctrl+s` work while a layout such as Russian is active.
- After a native call, a lock that was on, such as Caps Lock or Num Lock, reads as off in the focused application until you press a modifier or lock key, the keymap changes or focus moves.

### Removed

- The `wtype` keyboard. `NIRI_COMPUTER_USE_KEYBOARD` is no longer read, and any value is ignored rather than refused, since 0.1.0's docs told you to set it to `native`; a setting of `wtype` now gets the native keyboard. `wtype` is no longer needed at run time, and `status.binaries` lists only `grim` and `notify-send`.
- `recover` no longer ends an input program or offers to end your `wtype` processes. It releases the keys and buttons the marker names, asks for the manual check, and clears the marker after `yes`. An input-dirty marker 0.1.0 wrote while `wtype` ran can't be read by this version, so input stays blocked until you run `recover`. Restart every MCP client after upgrading, so no 0.1.0 server keeps running beside this one: a marker it writes while typing reads as unreadable here, and sends agents to `recover` until it finishes.

## 0.1.0 - 2026-10-14

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
- Release tarballs for x86_64 and aarch64 Linux with the binary, the skill and the licenses of the crates built in, and `install.sh`, which installs the binary and the skill from one of them.

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
