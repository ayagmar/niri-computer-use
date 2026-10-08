# Changelog

All notable changes to this project are documented here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## Unreleased

### Added

- `niri-desktop-mcp serve`: an MCP server over stdio with two read-only tools, `status` and `outputs`.
- `niri-desktop-mcp status`: prints the same readiness report as the `status` tool.
- `screenshot` tool: one output or a region inside one output, through grim, as JPEG or PNG with its geometry and scale.
- `clipboard_read` tool: the clipboard's text through `wl-paste`.
- `shell_status` tool, present when Noctalia is installed. `status` reports whether Noctalia is running and the lock state, from logind or else Noctalia.
- `desktop_state` tool: windows, workspaces, focus, overview and keyboard layouts from niri's event stream. `status` reports whether the stream is connected.
