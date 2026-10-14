---
title: The install script
description: What install.sh downloads, checks, writes and never touches, its variables, and how to update or uninstall.
sidebar:
  order: 2
---

`install.sh` is a POSIX shell script of about 110 lines, published with every release. Read it before you run it: it is written to be read, and this page is only a summary of it. It needs `curl`, `tar`, `sha256sum`, `uname` and `mktemp`, and runs on x86_64 and aarch64 Linux, the machines there are release builds for. On anything else it says so and stops.

```sh
curl -fsSLO https://github.com/ayagmar/niri-computer-use/releases/latest/download/install.sh
less install.sh
sh install.sh
```

## What it downloads

Everything comes from the GitHub release at `https://github.com/ayagmar/niri-computer-use/releases`. Without `NCU_VERSION`, the script asks for `releases/latest` and reads the version from the tag that redirect ends at. It then downloads two files from `releases/download/v<version>/` into a temporary directory under `TMPDIR`:

- `niri-computer-use-<version>-<arch>-linux.tar.gz`, where `<arch>` is what `uname -m` prints
- `niri-computer-use-<version>-<arch>-linux.tar.gz.sha256`

Each download has 15 seconds to connect and 5 minutes to finish. The script makes no other network requests.

## What it checks

Before it writes anywhere it keeps, the script checks three things. Any failure ends with a message and nothing installed.

1. The tarball matches its `.sha256`. This catches a download that is broken or cut off. It isn't a signature: the checksum comes from the same release as the tarball and the script, so whoever can change the release's files can change all three. If you want more than that, read the script and compare the tarball with a build of your own.
2. The binary runs. The script runs `niri-computer-use --version` from the temporary directory. A binary this system can't run, such as one that needs a newer glibc than 2.39, fails here. If `TMPDIR` is mounted `noexec`, point `TMPDIR` at a directory programs can run from.
3. The copies succeed. The binary and the skill are each copied beside their destination first, as `.niri-computer-use.new`, so a copy that fails, on a full disk for example, leaves what was installed before as it was.

## What it writes

| Path | What |
|---|---|
| `~/.local/bin/niri-computer-use` | The binary, mode 755. The directory is created if it is missing. `NCU_BIN_DIR` changes the directory |
| `~/.local/share/niri-computer-use/skills/niri-computer-use/` | The agent skill: `SKILL.md`, `references/tools.md` and `references/errors.md` |
| `~/.claude/skills/niri-computer-use` | A symlink to the skill, for Claude Code |
| `~/.agents/skills/niri-computer-use` | A symlink to the skill, for Pi, Codex and other agents that read `~/.agents/skills` |

`~/.claude/skills` and `~/.agents/skills` are created if they are missing. Each file goes into place with a rename, so a server that is running keeps the binary it started with until its clients restart. The temporary directory and the `.niri-computer-use.new` staging copies are removed when the script ends, however it ends.

If a link already exists at one of the two skill paths and points somewhere else, such as into a clone, the script replaces it and prints a line naming what it pointed to. If something that isn't a link is there, a directory for example, the script leaves it alone, prints where the skill is, and goes on.

## What it never does

- It doesn't run `sudo` or need root, and it writes nothing outside the paths above and `TMPDIR`.
- It doesn't change your `PATH` or any shell startup file. If `~/.local/bin` isn't on your `PATH`, the next steps it prints use the binary's full path.
- It doesn't edit your niri config, register the server in any agent, or start the server. The steps it prints at the end are for you to run.

## Variables

| Variable | Effect |
|---|---|
| `NCU_VERSION` | The release to install, such as `0.1.0`, instead of the latest. A leading `v` is accepted |
| `NCU_BIN_DIR` | Where the binary goes, instead of `~/.local/bin`. The skill and its links go to the same places as always |
| `NCU_RELEASES_URL` | Where the releases are, instead of this repository's on GitHub. The test suite points it at a local server |

```sh
NCU_VERSION=0.1.0 sh install.sh
NCU_BIN_DIR=~/bin sh install.sh
```

## Update

Run the script again. It replaces the binary and the skill the same way, and the two links keep pointing at the skill. The [Install](../install/#updating-while-agents-run) page says what happens to agents that are running while you update.

## Uninstall

The script has no uninstall mode; remove what it wrote. These two commands undo an install with the default `NCU_BIN_DIR`:

```sh
rm ~/.local/bin/niri-computer-use ~/.claude/skills/niri-computer-use ~/.agents/skills/niri-computer-use
rm -r ~/.local/share/niri-computer-use
```

The empty `~/.local/bin`, `~/.claude/skills` and `~/.agents/skills` directories stay, in case other programs use them. Things you set up afterwards aren't the script's, so they stay too: the server's entry in your agent (`claude mcp remove niri-computer-use` for Claude Code), the stop key bind in your niri config, the policy file at `~/.config/niri-computer-use/`, the audit log under `~/.local/state/niri-computer-use/`, and screenshots the agent saved under `~/Pictures/niri-computer-use`.
