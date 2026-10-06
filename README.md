# niri-desktop-mcp

`niri-desktop-mcp` is a Rust project for an MCP server that can observe and operate a live niri desktop. Noctalia integration is optional.

Status: **M0: research and test harness, no MCP tools yet.**

## Requirements

Runtime work targets niri 26.04, wtype 0.4, wl-clipboard 2.x, a version of grim with `-s`, `-o`, and `-g`, and systemd-logind. Noctalia 5.2.1 is supported but optional.

Building requires Rust 1.99.0 with rustfmt and Clippy, plus Make. Development checks also use cargo-deny, cargo-machete, typos, and ShellCheck. The nested harness will require wev and `dbus-run-session` when it is implemented.

## License

Licensed under the MIT License. See [LICENSE](LICENSE).
