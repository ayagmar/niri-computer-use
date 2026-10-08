# Changelog

All notable changes to this project are documented here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## Unreleased

### Added

- `niri-desktop-mcp serve`: an MCP server over stdio with two read-only tools, `status` and `outputs`.
- `niri-desktop-mcp status`: prints the same readiness report as the `status` tool.
- `desktop_state` tool: windows, workspaces, focus, overview and keyboard layouts from niri's event stream. `status` reports whether the stream is connected.
