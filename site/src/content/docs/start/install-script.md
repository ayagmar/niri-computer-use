---
title: The install script
description: What install.sh downloads, checks, writes and never touches, its variables, and how to update or uninstall.
sidebar:
  order: 2
---

`install.sh` is a POSIX shell script, published with every release. It is written to be read, and this page is only a summary of it. It needs `curl`, `tar`, `sha256sum`, `uname` and `mktemp`, and runs on x86_64 and aarch64 Linux, the machines there are release builds for. On anything else it says so and stops.

```sh
curl -fsSL https://github.com/ayagmar/niri-computer-use/releases/latest/download/install.sh | sh
```

To read it before it runs:

```sh
curl -fsSLO https://github.com/ayagmar/niri-computer-use/releases/latest/download/install.sh
less install.sh
sh install.sh
```

The whole script is one brace group, which `sh` reads to its end before running any of it. Piped from `curl`, a download cut short runs nothing, and an agent's `mcp add` that reads stdin can't take the script's own lines. Its questions are read from the terminal, `/dev/tty`, so piping it still asks before registering.

## What it downloads

Everything comes from the GitHub release at `https://github.com/ayagmar/niri-computer-use/releases`. Without `NCU_VERSION`, the script asks for `releases/latest` and reads the version from the tag that redirect ends at. It then downloads two files from `releases/download/v<version>/` into a temporary directory under `TMPDIR`:

- `niri-computer-use-<version>-<arch>-linux.tar.gz`, where `<arch>` is what `uname -m` prints
- `niri-computer-use-<version>-<arch>-linux.tar.gz.sha256`

Each download has 15 seconds to connect and 5 minutes to finish. The script makes no other network requests.

## What it checks

Before it writes anywhere it keeps, the script checks three things. Any failure ends with a message and nothing installed.

1. The tarball matches its `.sha256`. This catches a download that is broken or cut off. It isn't a signature: the checksum comes from the same release as the tarball and the script, so whoever can change the release's files can change all three. If you want more than that, read the script and compare the tarball with a build of your own.
2. The binary runs. The script runs `niri-computer-use --version` from the temporary directory. A binary this system can't run, such as one that needs a newer glibc than 2.39, fails here. If `TMPDIR` is mounted `noexec`, point `TMPDIR` at a directory programs can run from.
3. The copies succeed. The binary, the skill and the Noctalia plugin are each copied beside their destination first, as `.niri-computer-use.new`, so a copy that fails, on a full disk for example, leaves what was installed before as it was.

## What it writes

| Path | What |
|---|---|
| `~/.local/bin/niri-computer-use` | The binary, mode 755. The directory is created if it is missing. `NCU_BIN_DIR` changes the directory |
| `~/.local/share/niri-computer-use/skills/niri-computer-use/` | The agent skill: `SKILL.md`, `references/tools.md` and `references/errors.md` |
| `~/.claude/skills/niri-computer-use` | A symlink to the skill, for Claude Code |
| `~/.agents/skills/niri-computer-use` | A symlink to the skill, for Pi, Codex and other agents that read `~/.agents/skills` |
| `~/.local/share/niri-computer-use/noctalia/niri-computer-use/` | The [Noctalia plugin](../noctalia-plugin/), without its tests. A release from before the plugin leaves an installed one as it is |
| `~/.local/share/noctalia/plugins/niri-computer-use` | A symlink to the plugin, only when `noctalia` is on `PATH`. `XDG_DATA_HOME` replaces `~/.local/share` here, as it does for Noctalia |

`~/.claude/skills` and `~/.agents/skills` are created if they are missing. The binary goes into place with a rename, so a server that is running keeps the binary it started with until its clients restart. The temporary directory and the `.niri-computer-use.new` staging copies are removed when the script ends, however it ends.

If a link already exists at one of the skill or plugin paths and points somewhere else, such as into a clone, the script replaces it and prints a line naming what it pointed to. If something that isn't a link is there, a directory for example, the script leaves it alone, prints where the skill or the plugin is, and goes on.

When `noctalia` is on `PATH`, the next steps include enabling the plugin with `noctalia msg plugins enable ayagmar/niri-computer-use`; the script doesn't enable it. Once it has linked the plugin, it runs `noctalia msg plugins list`, for at most five seconds, and when Noctalia lists the plugin as incompatible it says the plugin needs Noctalia 5.4 or newer. After an update, the script lists each `niri-computer-use serve` still running the binary it replaced, by PID and the name of the agent that started it, so you know which agents to restart or reconnect.

## What it never does

- It doesn't run `sudo` or need root, and it writes nothing outside the paths above and `TMPDIR`.
- It doesn't change your `PATH` or any shell startup file. If `~/.local/bin` isn't on your `PATH`, the next steps it prints use the binary's full path.
- It doesn't edit your niri or Noctalia config, enable the plugin, start the server or stop one that runs. It registers the server in an agent only through that agent's own `mcp add` command, and only when you say yes at its prompt or set `NCU_REGISTER=1`; see [Registering in your agents](#registering-in-your-agents). The other steps it prints at the end are for you to run.

## Variables

| Variable | Effect |
|---|---|
| `NCU_VERSION` | The release to install, such as `0.1.0`, instead of the latest. A leading `v` is accepted |
| `NCU_BIN_DIR` | Where the binary goes, instead of `~/.local/bin`. The skill and its links go to the same places as always |
| `NCU_RELEASES_URL` | Where the releases are, instead of this repository's on GitHub. The test suite points it at a local server |
| `NCU_REGISTER` | `1` registers the server in every agent found on `PATH` without asking, `0` registers in none. Unset, the script asks per agent when its output is a terminal it can open, and registers in none otherwise |

```sh
NCU_VERSION=0.1.0 sh install.sh
NCU_BIN_DIR=~/bin sh install.sh
```

## Registering in your agents

After the files are in place, the script looks for `claude`, `pi`, `codex`, `amp`, `copilot`, `gemini` and `qwen` on `PATH`. For each one it finds it asks, when its output is a terminal it can open (`/dev/tty`), whether to register the server now, and shows the command it would run:

```sh
claude mcp add --scope user niri-computer-use -- ~/.local/bin/niri-computer-use serve
pi mcp add niri-computer-use --exposure direct -- ~/.local/bin/niri-computer-use serve
codex mcp add niri-computer-use -- ~/.local/bin/niri-computer-use serve
amp mcp add niri-computer-use -- ~/.local/bin/niri-computer-use serve
copilot mcp add niri-computer-use -- ~/.local/bin/niri-computer-use serve
gemini mcp add --scope user niri-computer-use ~/.local/bin/niri-computer-use serve
qwen mcp add --scope user niri-computer-use ~/.local/bin/niri-computer-use serve
```

These are the agents' own commands, the same ones [Client setup](../clients/) gives, so no config file is edited by the script. Each command writes the agent's user config, not a project's, whichever directory you run the script from. Claude Code is asked first with `claude mcp get`, and an existing registration is left as it is. Gemini CLI and Qwen Code replace an existing entry with the same command. Amp refuses a name it already has, so a second install reports that refusal for Amp; the registration from the first stays. An agent that refuses the command doesn't fail the install: the script prints the agent's last line of output and lists the command under the next steps for you to run. With `NCU_REGISTER=1` it registers in every agent found without asking, for unattended installs; with `NCU_REGISTER=0`, or when the output isn't a terminal and the variable is unset, it registers in none and prints the commands instead.

## Update

Run the script again. It replaces the binary, the skill and the plugin the same way, and the links keep pointing at them. The [Install](../install/#updating-while-agents-run) page says what happens to agents that are running while you update.

## Uninstall

The script has no uninstall mode; remove what it wrote. If you enabled the plugin, run `noctalia msg plugins disable ayagmar/niri-computer-use` first. These two commands then undo an install with the default `NCU_BIN_DIR`:

```sh
rm -f ~/.local/bin/niri-computer-use ~/.claude/skills/niri-computer-use ~/.agents/skills/niri-computer-use "${XDG_DATA_HOME:-$HOME/.local/share}/noctalia/plugins/niri-computer-use"
rm -r ~/.local/share/niri-computer-use
```

To unregister the server from an agent the script registered it in, use the agent's own command: `claude mcp remove --scope user niri-computer-use`, `pi mcp remove niri-computer-use`, `codex mcp remove niri-computer-use`, `amp mcp remove niri-computer-use`, `copilot mcp remove niri-computer-use`, `gemini mcp remove --scope user niri-computer-use` or `qwen mcp remove niri-computer-use`. Amp's `remove` looks in the workspace settings first and falls back to `~/.config/amp/settings.json`, so run it from outside any project with its own `niri-computer-use` entry.

If the script said it replaced or left one of the skill or plugin paths, that path wasn't its own: leave it out of the first `rm`, and point a replaced link back where the script said it pointed.

The directories the script created around these paths, such as `~/.local/bin` and `~/.claude/skills`, stay, in case other programs use them. Things you set up afterwards aren't the script's, so they stay too: the server's entry in your agent (`claude mcp remove niri-computer-use` for Claude Code), the stop key bind in your niri config, the policy file at `~/.config/niri-computer-use/`, the audit log under `~/.local/state/niri-computer-use/`, and screenshots the agent saved under `~/Pictures/niri-computer-use`.
