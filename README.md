# niri-desktop-mcp

[![CI](https://github.com/ayagmar/niri-desktop-mcp/actions/workflows/ci.yml/badge.svg)](https://github.com/ayagmar/niri-desktop-mcp/actions/workflows/ci.yml)

An MCP server for AI agents to observe and drive a [niri](https://github.com/niri-wm/niri) Wayland desktop. Noctalia is optional.

**Status: M0 (research) is done.** It produced a test harness that runs a nested niri, and optionally Noctalia, in a window on your desktop, a virtual pointer probe, a Noctalia IPC probe, and the results in [docs/results/m0.md](docs/results/m0.md). No MCP tools exist yet.

## Requirements

Targets niri 26.04. Building needs Rust 1.99.0 (pinned in `rust-toolchain.toml`, so rustup installs it for you) and Make.

## Development

See [docs/development.md](docs/development.md) and [CONTRIBUTING.md](CONTRIBUTING.md).

## License

MIT. See [LICENSE](LICENSE). Built binaries include [`niri-ipc`](https://crates.io/crates/niri-ipc), which is GPL-3.0-or-later, so a distributed binary follows GPL-3.0 terms.
