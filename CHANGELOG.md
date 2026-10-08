# Changelog

All notable changes to this project are documented here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## Unreleased

### Added

- `niri-computer-use serve`: an MCP server over stdio with two read-only tools, `status` and `outputs`.
- `niri-computer-use status`: prints the same readiness report as the `status` tool.
- `screenshot` tool: one output or a region inside one output, through grim, as JPEG or PNG with its geometry and scale.
- `clipboard_read` tool: the clipboard's text through `wl-paste`.
- An audit log of every tool call at `$XDG_STATE_HOME/niri-computer-use/audit.jsonl`, with argument metadata and outcomes but no contents. `status` reports its path and last write error.
- `shell_status` tool, present when Noctalia is installed. `status` reports whether Noctalia is running and the lock state from logind and Noctalia; the screen counts as locked if either says so. logind is asked about the session niri itself runs in, read from niri's environment, so the answer doesn't depend on where the server was started.
- `desktop_state` tool: windows, workspaces, focus, overview and keyboard layouts from niri's event stream. `status` reports whether the stream is connected.
- The `niri-computer-use` skill in `skills/`, for agents using the read-only tools.
- `make inspect` and `make inspect-check`: the server under the pinned MCP Inspector.
- `niri-computer-use stop` and `resume`: set and clear a stop flag in the niri instance's runtime directory, `$XDG_RUNTIME_DIR/niri-computer-use/<instance>/`. `status` reports it as `stop`.
- `acquire_desktop` and `release_desktop`: one server at a time holds the lease on a niri instance, an exclusive `flock` in the runtime directory. The stop flag takes it back. `status` reports the holder. New error names `lease_held`, `stopped` and `recovery_required`.
- The policy file, `$XDG_CONFIG_HOME/niri-computer-use/policy.toml`: launch presets and an app deny list, checked at startup and reported by `status`. `acquire_desktop` refuses with `read_only` while it is invalid or niri's version isn't supported, and with `screen_locked` while the screen is locked or its lock state is unknown.
- The `niri-computer-use` skill covers the lease tools and tells agents to leave stops, recovery and locks to the user.
- `make nested-control`: M2's acceptance in a nested niri, with the nested Noctalia as the lock source.
- `niri-computer-use recover`: clears the input-dirty marker after ending the input child it names and asking the human to confirm that no input is held. `status` reports the marker as `input_dirty`.

### Changed

- The project is renamed from `niri-desktop-mcp` to `niri-computer-use`, including the binary, the MCP server's name and the audit log's directory.
