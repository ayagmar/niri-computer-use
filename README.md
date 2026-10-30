# niri-computer-use

[![CI](https://github.com/ayagmar/niri-computer-use/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/ayagmar/niri-computer-use/actions/workflows/ci.yml) [![Release](https://img.shields.io/github/v/release/ayagmar/niri-computer-use)](https://github.com/ayagmar/niri-computer-use/releases/latest) [![crates.io](https://img.shields.io/crates/v/niri-computer-use)](https://crates.io/crates/niri-computer-use) [![Crate size](https://img.shields.io/crates/size/niri-computer-use)](https://crates.io/crates/niri-computer-use) [![MSRV](https://img.shields.io/crates/msrv/niri-computer-use)](https://crates.io/crates/niri-computer-use) [![niri 26.04](https://img.shields.io/badge/niri-26.04-f2a33a)](https://github.com/niri-wm/niri) [![Docs](https://img.shields.io/badge/docs-site-informational)](https://ayagmar.github.io/niri-computer-use/) [![License: MIT](https://img.shields.io/crates/l/niri-computer-use)](https://github.com/ayagmar/niri-computer-use/blob/main/LICENSE)

Let Claude Code, Codex, Pi or any MCP agent see and drive your [niri](https://github.com/niri-wm/niri) desktop: windows, screenshots, buttons by name, keys and clicks, with a stop key the agent can't press.

<picture>
  <source media="(prefers-reduced-motion: reduce)" srcset="https://ayagmar.github.io/niri-computer-use/demo/errand.png">
  <img src="https://ayagmar.github.io/niri-computer-use/demo/errand.gif" width="960" alt="A niri desktop with a terminal running Claude Code on the left and Files on the right. The agent selects two screenshots in Files, makes a folder for them from the right-click menu, names it release-0.1, and gives focus back to the terminal. A caption band under the desktop shows each request, a summary of the server's answer, its round trip and whether the agent holds the lease.">
</picture>

Asked: "In Files, put the two screenshots into a new folder called release-0.1, then bring me back here." This is a real Claude Code session (Opus 5.5) on a niri 26.04 desktop, with the installed server; nothing was scripted. The agent's keyboard shortcut doesn't open the dialog, so it uses the right-click menu instead. Thinking time between calls is shortened; the calls play in real time. [The full 19-second recording (MP4)](https://ayagmar.github.io/niri-computer-use/demo/errand.mp4).

Documentation: **https://ayagmar.github.io/niri-computer-use/**, also as plain Markdown for agents ([llms.txt](https://ayagmar.github.io/niri-computer-use/llms.txt)).

## What it is

- **Your real desktop, not a VM.** The agent looks at and acts on the screen you are working on.
- **niri only, through niri's own IPC.** Windows, focus and layout come from niri. Where an app exposes its buttons and fields on the accessibility bus, the agent can act on them by name rather than by pixels. With a niri release other than 26.04 the server only looks; a 26.04 patch release works, with a warning in `status`.
- **Guardrails, not a sandbox.** A lease, a stop key, a lock check and an audit log. An agent holding the lease can do anything you could do with a mouse and keyboard.

It offers up to 22 tools over stdio: reading the desktop, screenshots it can save to `~/Pictures/niri-computer-use`, zooming into part of a screenshot to read small text, accessible elements, and, under the lease, focus, launch, close, niri's layout actions, pointer, keyboard, paste, element actions and three Noctalia panels. Acceptance evidence for each milestone is in [docs/results/](https://github.com/ayagmar/niri-computer-use/tree/main/docs/results). It is in early development.

## Install

Release builds exist for x86_64 and aarch64 Linux, and both need glibc 2.39 or newer and libxkbcommon. Either way you need niri 26.04, and `grim` and `loginctl` at run time, plus `notify-send` if you turn on `confirm_lease`.

Two ways to install, both giving the same server:

| | Install script | Cargo |
|---|---|---|
| Gets | The release binary for your machine, checked against its `.sha256` | The crate from crates.io, built on your machine |
| Binary | `~/.local/bin/niri-computer-use` | `~/.cargo/bin/niri-computer-use` |
| Skill | Installed and linked for Claude Code, Pi and Codex | From the skills CLI or the clone, below |
| Update | Run the script again | Run `cargo install` again |

**With the install script.** Pipe it into `sh`, or download it with `curl -fsSLO` to read it first. It puts the binary in `~/.local/bin`, copies the skill that tells your agents how to use the server to `~/.local/share/niri-computer-use`, links it from `~/.claude/skills` and `~/.agents/skills`, links the Noctalia plugin when Noctalia is installed, and ends with the steps in the Quickstart below. [The install script](https://ayagmar.github.io/niri-computer-use/start/install-script/) lists every file it writes, what it checks, `NCU_VERSION` and `NCU_BIN_DIR`, and how to uninstall.

```sh
curl -fsSL https://github.com/ayagmar/niri-computer-use/releases/latest/download/install.sh | sh
```

**With cargo.** With Rust 1.99 or newer and libxkbcommon's headers (`libxkbcommon-dev` on Debian and Ubuntu, `libxkbcommon` on Arch), cargo builds the crate from crates.io into `~/.cargo/bin`:

```sh
cargo install --locked niri-computer-use
```

The crate holds no skill. Install it with the [skills CLI](https://skills.sh), which copies it into `~/.agents/skills` and links it from `~/.claude/skills`, or link `skills/niri-computer-use` from a clone:

```sh
npx skills add ayagmar/niri-computer-use --skill niri-computer-use --agent claude-code --agent pi --agent codex -g
```

[Install](https://ayagmar.github.io/niri-computer-use/start/install/) has the full steps, among them building from a clone, installing by hand from the tarball and updating while agents run.

## Quickstart

From a shell inside your niri session, with the binary where you put it:

```sh
~/.local/bin/niri-computer-use status
```

`status` prints a readiness report as JSON; `niri.compat` should be `ok`. The install script offers to register the server in each agent it supports that it finds on `PATH` (Claude Code, Pi, Codex, Amp, GitHub Copilot CLI, Gemini CLI and Qwen Code), through the agent's own command. To do it yourself, here for Claude Code:

```sh
claude mcp add --scope user niri-computer-use -- ~/.local/bin/niri-computer-use serve
```

[Client setup](https://ayagmar.github.io/niri-computer-use/start/clients/) covers Codex, Pi and other MCP clients. None needs more than the command: the server finds your niri session itself when its environment lacks one.

Then bind the stop key in your niri config, with the binary's full path:

```kdl
binds {
    Mod+Shift+Escape allow-inhibiting=false allow-when-locked=true hotkey-overlay-title="Stop the AI agent (niri-computer-use)" { spawn "/home/you/.local/bin/niri-computer-use" "stop"; }
}
```

The key cancels the running action and takes the lease back; `~/.local/bin/niri-computer-use resume` (or `~/.cargo/bin/…` after `cargo install`) lets agents act again. [First session](https://ayagmar.github.io/niri-computer-use/start/first-session/) walks through the rest.

## Example

These are the eleven calls Claude Code made in the recording above, with long arguments cut:

```text
status
desktop_state
acquire_desktop
focus             {"window_id": 11, …}
click             {"x": 979, "y": 88, …}
key               {"keys": ["shift+Right", "ctrl+alt+n"], …}
click             {"x": 1061, "y": 88, "button": "right", …}
click             {"x": 1130, "y": 100, …}
key               {"keys": ["ctrl+a"], …}
type_text         {"text": "release-0.1", "submit": true, …}
release_desktop   {"restore_focus": true}
```

Each action returns `accepted` (niri took the request), `observed` (what niri saw afterwards) and `changes` (what niri saw change). [docs/results/readme-example.md](https://github.com/ayagmar/niri-computer-use/blob/main/docs/results/readme-example.md) has a graded test run with every call and its full result.

## Safety

| | |
|---|---|
| Lease | One agent at a time acts. |
| Stop key | Cancels the action and takes the lease back. The agent can't press it. |
| Lock | Nothing acts while the screen is locked. |
| Focus | Typing checks focus, unless told not to. |
| Deny list | No input while a denied app has focus. |
| Audit log | Every call that reaches a tool, without typed text. |

This is not a sandbox: agents see your screen, and with the lease they can do anything you can. [Safety](https://ayagmar.github.io/niri-computer-use/concepts/safety/#what-isnt-a-boundary) has how each guard works and what isn't protected. Read it before letting an agent act. Report a problem as [SECURITY.md](https://github.com/ayagmar/niri-computer-use/blob/main/SECURITY.md) says.

## Limits

- niri 26.04 only. With another major or minor release the server is read-only. niri built from `main` still reports 26.04, so the server acts on it, but it isn't a supported release: against `main` at `fc9e8c4`, typing around a keyboard layout switch went wrong in the nested checks.
- The pointer tools need one monitor at transform `Normal`. The keyboard, screenshots and the other tools work with any monitors.
- Release builds exist for x86_64 and aarch64 only.
- Accessibility varies by toolkit. GTK 4 works; GTK 3 and Qt apps that draw their own title bar give no element boxes, and niri 26.04 gives tiled windows none until they float. Activating an element and setting its text work in both cases.

## Documentation

- [Install](https://ayagmar.github.io/niri-computer-use/start/install/), [The install script](https://ayagmar.github.io/niri-computer-use/start/install-script/) and [Client setup](https://ayagmar.github.io/niri-computer-use/start/clients/): install, check, and register the server in your agent
- [Noctalia plugin](https://ayagmar.github.io/niri-computer-use/start/noctalia-plugin/): see which agent drives the desktop and what it does, and stop it from the bar
- [Safety](https://ayagmar.github.io/niri-computer-use/concepts/safety/): what the server enforces, what it doesn't, and how you take the desktop back
- [Configuration](https://ayagmar.github.io/niri-computer-use/concepts/configuration/): `policy.toml`, launch presets and desktop entries, the deny list, `unrestricted`, `confirm_lease`, the shared engine and tool profiles
- [Tools](https://ayagmar.github.io/niri-computer-use/tools/overview/): every tool, its arguments and results
- [Error reference](https://ayagmar.github.io/niri-computer-use/reference/errors/) and [Troubleshooting](https://ayagmar.github.io/niri-computer-use/reference/troubleshooting/): what each error means and what to do
- [CLI reference](https://ayagmar.github.io/niri-computer-use/reference/cli/): `serve`, `status`, `stop`, `resume`, `recover`, `watch`
- [For agents](https://ayagmar.github.io/niri-computer-use/reference/agents/): every page as Markdown, `llms.txt`, and the skill at stable URLs

## Development

See [docs/development.md](https://github.com/ayagmar/niri-computer-use/blob/main/docs/development.md) and [CONTRIBUTING.md](https://github.com/ayagmar/niri-computer-use/blob/main/CONTRIBUTING.md).

## License

MIT. See [LICENSE](https://github.com/ayagmar/niri-computer-use/blob/main/LICENSE). Built binaries include [`niri-ipc`](https://crates.io/crates/niri-ipc), which is GPL-3.0-or-later, so a distributed binary follows GPL-3.0 terms.
