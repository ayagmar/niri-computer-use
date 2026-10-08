# niri-computer-use

[![CI](https://github.com/ayagmar/niri-computer-use/actions/workflows/ci.yml/badge.svg)](https://github.com/ayagmar/niri-computer-use/actions/workflows/ci.yml)

An MCP server for AI agents to observe and drive a [niri](https://github.com/niri-wm/niri) Wayland desktop. Noctalia is optional.

**Status: M1 in progress.** The server runs over stdio and has five read-only tools, six when Noctalia is installed. It can't act on the desktop yet. M0's research and its nested test harness are recorded in [docs/results/m0.md](docs/results/m0.md).

## Requirements

Targets niri 26.04. Building needs Rust 1.99.0 (pinned in `rust-toolchain.toml`, so rustup installs it for you) and Make.

## Usage

Build it and print the readiness report from a shell inside your niri session:

```sh
cargo build --locked -p niri-computer-use
target/debug/niri-computer-use status
```

`niri-computer-use serve` speaks MCP over stdin and stdout. It reads `NIRI_SOCKET` from its environment to find niri. Every tool call is logged, without contents, to `$XDG_STATE_HOME/niri-computer-use/audit.jsonl` (by default `~/.local/state/niri-computer-use/audit.jsonl`).

| Tool | What it returns |
|---|---|
| `status` | the niri instance, niri's version and whether this build supports it, whether niri's event stream is connected, the lock state, whether Noctalia is running, and which required programs are on `PATH` |
| `desktop_state` | windows, workspaces, the focused window, whether the overview is open, and the keyboard layouts, as one snapshot |
| `outputs` | niri's outputs: modes, logical position and size, scale and transform |
| `screenshot` | an image of one output or of a region inside one output, with its geometry. JPEG, at most 1280 image pixels wide by default |
| `clipboard_read` | the clipboard's text, or why there is none |
| `shell_status` | Noctalia's status: bar, open panel and lock screen. Only listed when `noctalia` is on `PATH` |

Failures set `isError` and return `{"error": <name>, "detail": <upstream detail>}`. The names so far are `niri_unavailable`, `deadline_exceeded`, `upstream_error` and `noctalia_unavailable`. A mistake in the arguments, such as an unknown output or a value of the wrong type, comes back with `isError` and a plain-text message instead.

## Development

See [docs/development.md](docs/development.md) and [CONTRIBUTING.md](CONTRIBUTING.md).

## License

MIT. See [LICENSE](LICENSE). Built binaries include [`niri-ipc`](https://crates.io/crates/niri-ipc), which is GPL-3.0-or-later, so a distributed binary follows GPL-3.0 terms.
