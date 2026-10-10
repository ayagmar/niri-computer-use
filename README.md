# niri-computer-use

[![CI](https://github.com/ayagmar/niri-computer-use/actions/workflows/ci.yml/badge.svg)](https://github.com/ayagmar/niri-computer-use/actions/workflows/ci.yml)

An MCP server for AI agents to observe and drive a [niri](https://github.com/niri-wm/niri) Wayland desktop. Noctalia is optional.

**Status: M6.1 stabilization complete; M7 native input is experimental and incomplete.** The server runs over stdio and has six read-only tools, plus `shell_status` when Noctalia is installed and `elements` when the session has an accessibility bus, plus `acquire_desktop` and `release_desktop`, which take and give up the lease, four tools that act through niri's IPC while the lease is held (focus a window or a workspace, launch a preset, close a window), and seven input tools: the pointer tools `pointer_move`, `click`, `drag` and `scroll`, aimed at pixels of a screenshot, the keyboard tools `key` and `type_text`, and `paste`. With Noctalia installed, `shell_open` and `shell_close` open and close three of its panels. M0's research is recorded in [docs/results/m0.md](docs/results/m0.md), M1's acceptance in [docs/results/m1.md](docs/results/m1.md), M2's in [docs/results/m2.md](docs/results/m2.md), M3's in [docs/results/m3.md](docs/results/m3.md), M4's in [docs/results/m4.md](docs/results/m4.md), M5's in [docs/results/m5.md](docs/results/m5.md), M6's in [docs/results/m6.md](docs/results/m6.md), and M6.1's in [docs/results/m6.1.md](docs/results/m6.1.md). M7's tested subset and blockers are in [docs/results/m7.md](docs/results/m7.md).

## Requirements

Targets niri 26.04. Building needs Rust 1.99.0 (pinned in `rust-toolchain.toml`, so rustup installs it for you), Make and libxkbcommon, which the native keyboard links against (`libxkbcommon-dev` on Debian and Ubuntu).

## Usage

Build it and print the readiness report from a shell inside your niri session:

```sh
cargo build --locked -p niri-computer-use
target/debug/niri-computer-use status
```

`niri-computer-use stop` sets a stop flag for the niri instance in `NIRI_SOCKET`, and `niri-computer-use resume` clears it; `status` reports it as `stop`. `niri-computer-use recover` clears the input-dirty marker, which says input may be stuck: it ends the input child the marker names, releases any pointer button it names, asks you to check that no key or button is held, and clears the marker only after you type `yes`. Each `serve` starts a small guardian process that, if the server dies with input held, releases the keys and buttons its marker names at once; the marker still waits for `recover`.

`niri-computer-use serve` speaks MCP over stdin and stdout. It reads `NIRI_SOCKET` from its environment to find niri. Every tool call is logged, without contents, to `$XDG_STATE_HOME/niri-computer-use/audit.jsonl` (by default `~/.local/state/niri-computer-use/audit.jsonl`).

| Tool | What it returns |
|---|---|
| `status` | the niri instance, niri's version and whether this build supports it, whether niri's event stream is connected, who holds the lease, the stop flag, the policy file, the lock state, whether Noctalia is running, and which required programs are on `PATH` |
| `desktop_state` | windows, workspaces, the focused window, whether the overview is open, and the keyboard layouts, as one snapshot |
| `outputs` | niri's outputs: modes, logical position and size, scale and transform |
| `screenshot` | an image of one output or of a region inside one output, with its geometry. JPEG, at most 1280 image pixels wide by default. With `save_path` and a `capture_dir` in the policy file, it also writes a full-resolution PNG there |
| `clipboard_read` | the clipboard's text, or why there is none |
| `shell_status` | Noctalia's status: bar, open panel and lock screen. Only listed when `noctalia` is on `PATH` |
| `elements` | one window's accessible elements from its app's accessibility tree: role, name, states, action names, and the element's box in layout coordinates, or why it has none. Only listed when the session has an accessibility bus |
| `wait_for` | waits until a window appears, closes or changes its title, or the screen stops changing, for up to 30 seconds |
| `acquire_desktop` | takes the lease, so this agent is the one controlling this niri desktop, and notes the window the user was on; refused while another agent holds it, while the stop flag is set, while input may be stuck, while the screen is locked or its lock state is unknown, or when this build doesn't support the running niri or the policy file is invalid |
| `release_desktop` | gives the lease up, after giving focus back to the user's window if asked; the stop flag also takes it back |
| `focus_window` | focuses a window by id; says whether niri accepted it and whether focus was seen to arrive |
| `focus_workspace` | focuses a workspace by id, likewise |
| `launch` | starts a preset from the policy file and reports the new windows with its `app_id`; with `reuse`, focuses its one existing window instead |
| `close_window` | asks a window to close and reports `closed`, or `pending` if it is still open after five seconds, for example behind an unsaved-changes dialog |
| `pointer_move`, `click`, `drag`, `scroll` | move, click, drag or turn the wheel at pixels of a screenshot taken under the lease, through a virtual pointer bound to that screenshot's output; `pointer_move`, `click` and `drag` can aim at an element `elements` listed instead, checked again just before |
| `key`, `type_text` | press up to 16 key combinations, or type up to 1000 characters and optionally press Enter, into the focused app with `wtype`, after checking that focus is where the agent expects and stopping if it moves |
| `paste` | pastes up to 1 MiB of text with the paste combination the agent names (`ctrl+v`, `ctrl+shift+v` or `shift+Insert`), after the same checks as `key`, then puts the user's clipboard back, every type it offered, and says whether an app read the text |
| `shell_open`, `shell_close` | open or close a Noctalia panel, only `control-center`, `wallpaper` or `tray-drawer`, and report whether Noctalia shows it open. Only listed when `noctalia` is on `PATH` |

The action, input and shell tools require the lease and check the stop flag, the input-dirty marker and the lock state again before each action; a stop cancels the running one. Each result has `accepted`, whether niri or Noctalia took the request, and `observed`, what niri's event stream or Noctalia's status showed afterwards, including `interrupted` when focus went elsewhere during the wait and `uncertain` when the reply was lost; for input, `sent` once niri handled it. An outcome in doubt comes with a fresh screenshot of the focused output, and any action asked with `screenshot: true` comes with one taken once the screen stopped changing. Nothing is retried.

Failures set `isError` and return `{"error": <name>, "detail": <upstream detail>}`. The names so far are `niri_unavailable`, `deadline_exceeded`, `upstream_error`, `noctalia_unavailable`, `lease_held`, `lease_required`, `stopped`, `recovery_required`, `read_only`, `screen_locked`, `unknown_preset`, `ref_invalid`, `untested_output_config`, `app_denied`, `focus_mismatch`, `text_too_long`, `panel_not_allowed`, `clipboard_unsaved`, `save_not_enabled`, `not_accessible`, `ambiguous_window`, `element_stale` and `element_unmappable`. A mistake in the arguments, such as an unknown output or a value of the wrong type, comes back with `isError` and a plain-text message instead.

## Policy file

`$XDG_CONFIG_HOME/niri-computer-use/policy.toml` (by default `~/.config/niri-computer-use/policy.toml`) is read once when the server starts. Without it there are no launch presets and no denied apps, which is valid. For example:

```toml
deny_input_app_ids = ["org.keepassxc.KeePassXC"]
capture_dir = "~/Pictures/agent-shots"

[[preset]]
name = "firefox"
argv = ["firefox"]
app_id = "firefox"
```

A preset may not start a shell, an interpreter, `env`, `sudo` or another program that runs any command it is given, nor a terminal with arguments, not even `--app-id`, because terminals run trailing arguments as a command; a desktop file started with `gtk-launch` gives a terminal its own `app_id`. These rules catch common mistakes; they are a guardrail, not a boundary, since a wrapper script gets past any list. If the file breaks a rule or doesn't parse, `status` reports it as `invalid` and `acquire_desktop` and the action tools refuse with `read_only` until it is fixed and the server restarted. `launch` takes a preset's `name` and starts its `argv` through niri; `status` lists the names. The input tools refuse with `app_denied` while the focused window's `app_id` is on `deny_input_app_ids`, even with `expect: "none"`. This is a focus-based, best-effort guardrail, not target isolation: a pointer can hit a different, denied window while an allowed app has focus, and app IDs are self-reported. For high-assurance restrictions, use a separate desktop containing only approved applications.

`capture_dir` turns on saving screenshots; it is off when the key is absent, and `screenshot` then refuses `save_path` with `save_not_enabled`. It is an absolute path or one starting with `~/`, and `status` shows it. `save_path` is relative to it, made only of plain names (no `..`, `.` or leading `/`), and names a `.png` file. The directory is created with mode `0700` if it is missing; subdirectories in `save_path` must already exist. Subdirectories are opened without following symlinks, and the file is created new with mode `0600` without following a symlink at its name, so a save can't leave the directory or replace a file. The saved PNG is a separate capture at the output's own scale, taken just before the image the tool returns, so `max_width` doesn't shrink it; `saved` in the result gives its path and pixel size.

## Observation and input limits

Read-only does not mean private: screenshots, window titles, accessible names and clipboard text can expose secrets to the connected MCP client without a control lease, including while the host is locked. Only connect trusted clients. The audit log excludes those contents; it does not prevent the client from seeing them. `elements` reads only the window it is asked about, and refuses with `app_denied` for an app on `deny_input_app_ids`.

Focus and stop are checked between `wtype` calls. An in-flight call may finish up to 100 characters within its three-second deadline after focus changes or stop arrives. `typed` counts completed helper strokes, not characters confirmed in the intended application. No later part or submission is sent once interruption is detected; do not retry uncertain input automatically.

The experimental native backend is selected with `NIRI_COMPUTER_USE_KEYBOARD=native`; unset or `wtype` keeps the default. It uses the compositor's keymap and checks focus, layout and stop between individual key pairs. Text symbols absent from the active layout go, for that call only, on keys of the compositor's map that have none, with every existing key unchanged; afterwards niri must send the compositor's map back, byte for byte, before the call succeeds. Only spare keycodes up to 255 are used (14 in a US evdev map), so a text with more distinct missing symbols, or a missing control character, refuses whole. `key` combinations still refuse keysyms absent from the layout. Use only in isolated sessions without physical input: preservation of physical modifiers is unverified. Stop/cancel cleanup must be acknowledged before the marker clears. After a SIGKILL the server's crash guardian releases the marker's native keycodes, modifiers and pointer buttons and sends the compositor's map back within a deadline; the marker still blocks until `recover`, which sends the releases again before human confirmation. Native has not met the equivalence gate and is not the default. With the native backend selected, `click`, `drag` and `scroll` accept `keys: ["ctrl", "shift"]` (up to five modifiers: shift, ctrl, alt, altgr, super). One dirty marker covers the keyboard and pointer; both must release before it clears, and after a SIGKILL the guardian releases both.

`paste` saves the clipboard whole before touching it, through the wlr data-control protocol: every type it offers, up to 16 MiB in all, within two seconds. If it can't, or the clipboard holds what its owner marked as a secret (`x-kde-passwordManagerHint`, as password managers set it), it refuses with `clipboard_unsaved` and changes nothing. A keeper process then holds the text as the selection, marked with that same hint so clipboard managers that honour it, Noctalia's among them, leave it out of their history. After the key it waits up to two seconds for a read, then 100 ms of quiet, and offers the saved clipboard again; it keeps serving it, as `wl-copy` would, until something else is copied. A stop, a cancelled call or a failed key restores at once. Which client read the text can't be told: a clipboard manager that reads every new selection and ignores the hint would count as the read.

A settled screenshot means two sampled images matched, not that an application is ready. Its budget includes the initial delay and capture work. For action-return screenshots it starts under the action mutex; for `wait_for` it also includes time queued for individual captures. Read-only wait captures hold the mutex per image, not between samples, so actions, release and stop's lease takeback do not wait for the entire visual-settlement timeout. A timeout returns the last completed image with `settled: false`, including a first image completed at or just after the deadline; only a timeout with no completed image returns `deadline_exceeded`. The 100 ms minimum `wait_for` timeout is a budget, not a promise of an image or stability: two samples cannot match before the 150 ms second-capture start, and large downscaled images need additional capture time.

## Install and register

Install the binary, then register it in each agent from a shell inside your niri session:

```sh
cargo install --locked --path .
claude mcp add --scope user niri-computer-use -- ~/.cargo/bin/niri-computer-use serve
pi mcp add niri-computer-use --exposure direct -- ~/.cargo/bin/niri-computer-use serve
codex mcp add niri-computer-use -- ~/.cargo/bin/niri-computer-use serve
```

Claude Code and Pi pass their environment on to the server, so it finds niri. Codex passes only a short list of variables, so add this line to the `[mcp_servers.niri-computer-use]` section that `codex mcp add` writes to `~/.codex/config.toml`:

```toml
env_vars = ["NIRI_SOCKET", "XDG_RUNTIME_DIR", "WAYLAND_DISPLAY", "XDG_SESSION_ID"]
```

Without it, `status` reports `NIRI_SOCKET is not set`.

The skill in [`skills/niri-computer-use`](skills/niri-computer-use/SKILL.md) tells an agent how to use the tools. Link it into the agent's skills directory, for example:

```sh
ln -s ~/projects/niri-computer-use/skills/niri-computer-use ~/.claude/skills/niri-computer-use
ln -s ~/projects/niri-computer-use/skills/niri-computer-use ~/.agents/skills/niri-computer-use
```

## Stop key

Bind `niri-computer-use stop` in your niri config, so you can take the lease back from an agent with one key, even while an app inhibits shortcuts or the screen is locked. niri passes `NIRI_SOCKET` to what it spawns, so the stop reaches that niri instance:

```kdl
binds {
    Mod+Shift+Escape allow-inhibiting=false allow-when-locked=true hotkey-overlay-title="Stop the AI Agent (niri-computer-use)" { spawn "/home/you/.cargo/bin/niri-computer-use" "stop"; }
}
```

Use the path where `cargo install` put the binary. An agent can't press this key: niri binds don't fire from virtual keyboards. After a stop, `niri-computer-use resume` lets agents take the lease again; if `status` shows `input_dirty`, run `niri-computer-use recover` first.

## Development

See [docs/development.md](docs/development.md) and [CONTRIBUTING.md](CONTRIBUTING.md).

## License

MIT. See [LICENSE](LICENSE). Built binaries include [`niri-ipc`](https://crates.io/crates/niri-ipc), which is GPL-3.0-or-later, so a distributed binary follows GPL-3.0 terms.
