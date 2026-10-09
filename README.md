# niri-computer-use

[![CI](https://github.com/ayagmar/niri-computer-use/actions/workflows/ci.yml/badge.svg)](https://github.com/ayagmar/niri-computer-use/actions/workflows/ci.yml)

An MCP server for AI agents to observe and drive a [niri](https://github.com/niri-wm/niri) Wayland desktop. Noctalia is optional.

**Status: M5 complete.** The server runs over stdio and has five read-only tools, six when Noctalia is installed, plus `acquire_desktop` and `release_desktop`, which take and give up the lease, four tools that act through niri's IPC while the lease is held (focus a window or a workspace, launch a preset, close a window), and six input tools: the pointer tools `pointer_move`, `click`, `drag` and `scroll`, aimed at pixels of a screenshot, and the keyboard tools `key` and `type_text`. With Noctalia installed, `shell_open` and `shell_close` open and close three of its panels. M0's research is recorded in [docs/results/m0.md](docs/results/m0.md), M1's acceptance in [docs/results/m1.md](docs/results/m1.md), M2's in [docs/results/m2.md](docs/results/m2.md), M3's in [docs/results/m3.md](docs/results/m3.md), M4's in [docs/results/m4.md](docs/results/m4.md) and M5's in [docs/results/m5.md](docs/results/m5.md).

## Requirements

Targets niri 26.04. Building needs Rust 1.99.0 (pinned in `rust-toolchain.toml`, so rustup installs it for you) and Make.

## Usage

Build it and print the readiness report from a shell inside your niri session:

```sh
cargo build --locked -p niri-computer-use
target/debug/niri-computer-use status
```

`niri-computer-use stop` sets a stop flag for the niri instance in `NIRI_SOCKET`, and `niri-computer-use resume` clears it; `status` reports it as `stop`. `niri-computer-use recover` clears the input-dirty marker, which says input may be stuck: it ends the input child the marker names, releases any pointer button it names, asks you to check that no key or button is held, and clears the marker only after you type `yes`.

`niri-computer-use serve` speaks MCP over stdin and stdout. It reads `NIRI_SOCKET` from its environment to find niri. Every tool call is logged, without contents, to `$XDG_STATE_HOME/niri-computer-use/audit.jsonl` (by default `~/.local/state/niri-computer-use/audit.jsonl`).

| Tool | What it returns |
|---|---|
| `status` | the niri instance, niri's version and whether this build supports it, whether niri's event stream is connected, who holds the lease, the stop flag, the policy file, the lock state, whether Noctalia is running, and which required programs are on `PATH` |
| `desktop_state` | windows, workspaces, the focused window, whether the overview is open, and the keyboard layouts, as one snapshot |
| `outputs` | niri's outputs: modes, logical position and size, scale and transform |
| `screenshot` | an image of one output or of a region inside one output, with its geometry. JPEG, at most 1280 image pixels wide by default |
| `clipboard_read` | the clipboard's text, or why there is none |
| `shell_status` | Noctalia's status: bar, open panel and lock screen. Only listed when `noctalia` is on `PATH` |
| `acquire_desktop` | takes the lease, so this agent is the one controlling this niri desktop; refused while another agent holds it, while the stop flag is set, while input may be stuck, while the screen is locked or its lock state is unknown, or when this build doesn't support the running niri or the policy file is invalid |
| `release_desktop` | gives the lease up; the stop flag also takes it back |
| `focus_window` | focuses a window by id; says whether niri accepted it and whether focus was seen to arrive |
| `focus_workspace` | focuses a workspace by id, likewise |
| `launch` | starts a preset from the policy file and reports the new windows with its `app_id`; with `reuse`, focuses its one existing window instead |
| `close_window` | asks a window to close and reports `closed`, or `pending` if it is still open after five seconds, for example behind an unsaved-changes dialog |
| `pointer_move`, `click`, `drag`, `scroll` | move, click, drag or turn the wheel at pixels of a screenshot taken under the lease, through a virtual pointer bound to that screenshot's output |
| `key`, `type_text` | press a key combination or type up to 1000 characters into the focused app with `wtype`, 100 per call, after checking that focus is where the agent expects |
| `shell_open`, `shell_close` | open or close a Noctalia panel, only `control-center`, `wallpaper` or `tray-drawer`, and report whether Noctalia shows it open. Only listed when `noctalia` is on `PATH` |

The action, input and shell tools require the lease and check the stop flag, the input-dirty marker and the lock state again before each action; a stop cancels the running one. Each result has `accepted`, whether niri or Noctalia took the request, and `observed`, what niri's event stream or Noctalia's status showed afterwards, including `interrupted` when focus went elsewhere during the wait and `uncertain` when the reply was lost; for input, `sent` once niri handled it. An outcome in doubt comes with a fresh screenshot of the focused output. Nothing is retried.

Failures set `isError` and return `{"error": <name>, "detail": <upstream detail>}`. The names so far are `niri_unavailable`, `deadline_exceeded`, `upstream_error`, `noctalia_unavailable`, `lease_held`, `lease_required`, `stopped`, `recovery_required`, `read_only`, `screen_locked`, `unknown_preset`, `ref_invalid`, `untested_output_config`, `app_denied`, `focus_mismatch`, `text_too_long` and `panel_not_allowed`. A mistake in the arguments, such as an unknown output or a value of the wrong type, comes back with `isError` and a plain-text message instead.

## Policy file

`$XDG_CONFIG_HOME/niri-computer-use/policy.toml` (by default `~/.config/niri-computer-use/policy.toml`) is read once when the server starts. Without it there are no launch presets and no denied apps, which is valid. For example:

```toml
deny_input_app_ids = ["org.keepassxc.KeePassXC"]

[[preset]]
name = "firefox"
argv = ["firefox"]
app_id = "firefox"
```

A preset may not start a shell, an interpreter, `env`, `sudo` or another program that runs any command it is given, nor a terminal with arguments, not even `--app-id`, because terminals run trailing arguments as a command; a desktop file started with `gtk-launch` gives a terminal its own `app_id`. These rules catch common mistakes; they are a guardrail, not a boundary, since a wrapper script gets past any list. If the file breaks a rule or doesn't parse, `status` reports it as `invalid` and `acquire_desktop` and the action tools refuse with `read_only` until it is fixed and the server restarted. `launch` takes a preset's `name` and starts its `argv` through niri; `status` lists the names. The input tools refuse with `app_denied` while the focused window's `app_id` is on `deny_input_app_ids`.

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
