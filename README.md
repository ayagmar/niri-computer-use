# niri-computer-use

[![CI](https://github.com/ayagmar/niri-computer-use/actions/workflows/ci.yml/badge.svg)](https://github.com/ayagmar/niri-computer-use/actions/workflows/ci.yml)

An MCP server for AI agents to observe and drive a [niri](https://github.com/niri-wm/niri) Wayland desktop. Noctalia is optional.

Documentation: **https://ayagmar.github.io/niri-computer-use/**, also as plain Markdown for agents ([llms.txt](https://ayagmar.github.io/niri-computer-use/llms.txt)).

**Status: early development.** Up to 25 tools over stdio: reading the desktop, screenshots, accessible elements, and, under a lease, focus, launch, close, niri actions, pointer, keyboard, paste, three Noctalia panels and, with `unrestricted`, any Noctalia command. The native keyboard backend is experimental and off by default. Acceptance evidence for each milestone is in [docs/results/](docs/results/).

## Install

Needs niri 26.04, Rust 1.99.0 through rustup, libxkbcommon, and `grim`, `wl-clipboard`, `wtype` and `loginctl` at run time. From a shell inside your niri session:

```sh
git clone https://github.com/ayagmar/niri-computer-use.git
cargo install --locked --path niri-computer-use
claude mcp add --scope user niri-computer-use -- ~/.cargo/bin/niri-computer-use serve
```

No client needs extra configuration, Codex included: when `XDG_RUNTIME_DIR`, `NIRI_SOCKET` or `WAYLAND_DISPLAY` is missing, the server finds your session itself, to match the ones that are set. A variable you set still wins, which matters when you run more than one niri session. If `WAYLAND_DISPLAY` and `NIRI_SOCKET` belong to different compositors, the server refuses input, screenshots and clipboard reads. A server started over SSH, from a TTY or as a service without these variables still attaches to your desktop when your niri session is the only one. [Client setup](https://ayagmar.github.io/niri-computer-use/start/clients/) covers Pi, Codex and other MCP clients, and the agent skill in [`skills/niri-computer-use`](skills/niri-computer-use/SKILL.md).

## Stop key

Bind `niri-computer-use stop` in your niri config so you can take the desktop back from an agent with one key:

```kdl
binds {
    Mod+Shift+Escape allow-inhibiting=false allow-when-locked=true hotkey-overlay-title="Stop the AI agent (niri-computer-use)" { spawn "/home/you/.cargo/bin/niri-computer-use" "stop"; }
}
```

`niri-computer-use resume` lets agents act again. See [First session](https://ayagmar.github.io/niri-computer-use/start/first-session/).

## Safety

The lease, the stop key, the lock gate, the policy file and the audit log are guardrails, not a sandbox. An agent holding the lease can do anything you could with a mouse and keyboard, and any connected agent can see your screen, window titles and clipboard. Read [Safety](https://ayagmar.github.io/niri-computer-use/concepts/safety/) before letting an agent act, and [SECURITY.md](SECURITY.md) to report a problem.

## Policy

`~/.config/niri-computer-use/policy.toml` holds launch presets, apps that never get input, and where screenshots may be saved. One key, `unrestricted`, is off by default. Agents without a shell of their own, such as desktop apps, rely on that: with it off, the server can't run anything you didn't put in a preset. With `unrestricted = true`, or `NIRI_COMPUTER_USE_UNRESTRICTED=1` in one client's server settings, an agent can run any program through the server: `niri_action` accepts `Spawn`, `Quit` and the other gated niri actions, the `noctalia` tool sends any Noctalia command, and presets may pass arguments to terminals and set `env`. Everything still needs the lease, stops at the stop key and goes to the audit log. See [Configuration](https://ayagmar.github.io/niri-computer-use/concepts/configuration/#unrestricted).

With `shared = true`, or `NIRI_COMPUTER_USE_SHARED=1`, every MCP client of a niri instance is served by one engine process, which the first client starts and which exits two seconds after its last connection, lease and pending input cleanup have ended. Each client keeps its own policy file, `unrestricted`, keyboard backend and home directory. See [Configuration](https://ayagmar.github.io/niri-computer-use/concepts/configuration/#shared).

## Documentation

- [Tools](https://ayagmar.github.io/niri-computer-use/tools/overview/): every tool, its arguments and results
- [Configuration](https://ayagmar.github.io/niri-computer-use/concepts/configuration/): `policy.toml`, launch presets, the deny list, `capture_dir`, `unrestricted`, `shared`
- [Error reference](https://ayagmar.github.io/niri-computer-use/reference/errors/) and [Troubleshooting](https://ayagmar.github.io/niri-computer-use/reference/troubleshooting/)
- [CLI reference](https://ayagmar.github.io/niri-computer-use/reference/cli/): `serve`, `status`, `stop`, `resume`, `recover`

## Development

See [docs/development.md](docs/development.md) and [CONTRIBUTING.md](CONTRIBUTING.md).

## License

MIT. See [LICENSE](LICENSE). Built binaries include [`niri-ipc`](https://crates.io/crates/niri-ipc), which is GPL-3.0-or-later, so a distributed binary follows GPL-3.0 terms.
