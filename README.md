# niri-desktop-mcp

[![CI](https://github.com/ayagmar/niri-desktop-mcp/actions/workflows/ci.yml/badge.svg)](https://github.com/ayagmar/niri-desktop-mcp/actions/workflows/ci.yml)

An MCP server for AI agents to observe and drive a [niri](https://github.com/niri-wm/niri) Wayland desktop. Noctalia is optional.

**Status: M1 in progress.** The server runs over stdio and has two read-only tools. It can't act on the desktop yet. M0's research and its nested test harness are recorded in [docs/results/m0.md](docs/results/m0.md).

## Requirements

Targets niri 26.04. Building needs Rust 1.99.0 (pinned in `rust-toolchain.toml`, so rustup installs it for you) and Make.

## Usage

Build it and print the readiness report from a shell inside your niri session:

```sh
cargo build --locked -p niri-desktop-mcp
target/debug/niri-desktop-mcp status
```

`niri-desktop-mcp serve` speaks MCP over stdin and stdout. It reads `NIRI_SOCKET` from its environment to find niri.

| Tool | What it returns |
|---|---|
| `status` | the niri instance, niri's version and whether this build supports it, and which required programs are on `PATH` |
| `outputs` | niri's outputs: modes, logical position and size, scale and transform |

Failures set `isError` and return `{"error": <name>, "detail": <upstream detail>}`. The names so far are `niri_unavailable`, `deadline_exceeded` and `upstream_error`.

## Development

See [docs/development.md](docs/development.md) and [CONTRIBUTING.md](CONTRIBUTING.md).

## License

MIT. See [LICENSE](LICENSE). Built binaries include [`niri-ipc`](https://crates.io/crates/niri-ipc), which is GPL-3.0-or-later, so a distributed binary follows GPL-3.0 terms.
