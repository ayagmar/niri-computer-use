---
title: Install
description: Install niri-computer-use with the install script or with cargo, add the agent skill, and check it against your niri session.
sidebar:
  order: 1
---

## Requirements

- niri 26.04. With another major or minor version the server is read-only: it looks but refuses to act.
- For a release build, x86_64 or aarch64 Linux with glibc 2.39 or newer; both builds have the same requirements. To build it yourself, Rust 1.99 or newer.
- libxkbcommon, which the keyboard code links against. To build, its headers too (`libxkbcommon-dev` on Debian and Ubuntu, `libxkbcommon` on Arch).
- At run time: `grim` for screenshots and `loginctl` from systemd for the lock state.
- Optional: [Noctalia](../../concepts/noctalia/) 5.2 for its panels and lock state, and an accessibility bus (at-spi2-core) for [`elements`](../../tools/elements/).
- One monitor, for the pointer tools. `pointer_move`, `click`, `drag` and `scroll` run only with one enabled output at transform `Normal`; with a second monitor or a rotated one they refuse with [`untested_output_config`](../../reference/troubleshooting/#the-pointer-tools-fail-with-untested_output_config). The other tools, the keyboard and screenshots among them, work with any monitors.

## Two ways to install

Both give you the same server. The script is quicker and also installs the agent skill; cargo builds the crate on your machine and leaves the skill to you.

| | Install script | Cargo |
|---|---|---|
| Gets | The release binary for your machine, checked against its `.sha256` | The crate from crates.io, built on your machine |
| Binary | `~/.local/bin/niri-computer-use` | `~/.cargo/bin/niri-computer-use` |
| Skill | Installed and linked for Claude Code, Pi and Codex | From the [skills CLI](#the-skill) or the clone |
| Update | Run the script again | Run `cargo install` again |

### With the install script

Download it, read it, then run it:

```sh
curl -fsSLO https://github.com/ayagmar/niri-computer-use/releases/latest/download/install.sh
less install.sh
sh install.sh
```

It downloads the release tarball for your machine, checks it, puts the binary in `~/.local/bin`, copies the skill to `~/.local/share/niri-computer-use` and links it from `~/.claude/skills` and `~/.agents/skills`. Then it offers to register the server in each of Claude Code, Pi and Codex it finds on `PATH`, through that agent's own `mcp add` command, and ends by printing the [next steps](#check-it). [The install script](../install-script/) lists every file it writes, what it checks, `NCU_VERSION` and `NCU_BIN_DIR`, and how to uninstall.

### With cargo

With Rust 1.99 or newer and libxkbcommon's headers, cargo builds the crate from crates.io into `~/.cargo/bin`:

```sh
cargo install --locked niri-computer-use
```

The crate holds no skill, so install [the skill](#the-skill) next. To update, run the same command again.

To build from source instead, run `cargo install` inside a clone. rustup takes the Rust version the repository pins from `rust-toolchain.toml` in the current directory, so outside the clone it builds with your default toolchain instead. The skill is in the clone at `skills/niri-computer-use`:

```sh
git clone https://github.com/ayagmar/niri-computer-use.git
cd niri-computer-use
cargo install --locked --path .
```

To update a source build, run `git pull` and the same `cargo install` again, inside the clone.

## The skill

The skill tells an agent how to use the tools: start with `status`, prefer structured data to screenshots, read `accepted` and `observed`, and when to stop and hand back. Claude Code reads it from `~/.claude/skills`; Pi and Codex read `~/.agents/skills`. The install script puts it there. After `cargo install`, do one of these.

The [skills CLI](https://skills.sh) installs it from the repository: it copies the skill into `~/.agents/skills` and links it from `~/.claude/skills`. Run the same command to update it:

```sh
npx skills add ayagmar/niri-computer-use --skill niri-computer-use --agent claude-code --agent pi --agent codex -g
```

Or link the clone's copy, from inside the clone, so `git pull` updates it:

```sh
mkdir -p ~/.claude/skills ~/.agents/skills
ln -s "$PWD/skills/niri-computer-use" ~/.claude/skills/niri-computer-use
ln -s "$PWD/skills/niri-computer-use" ~/.agents/skills/niri-computer-use
```

The same files are on this site under [For agents](../../reference/agents/).

## By hand

Without the script, download your machine's tarball, check it, and put the binary in `~/.local/bin` or another directory on your `PATH`. `uname -m` prints `x86_64` or `aarch64` on the machines there are builds for:

```sh
arch=$(uname -m)
curl -LO "https://github.com/ayagmar/niri-computer-use/releases/download/v0.1.1/niri-computer-use-0.1.1-$arch-linux.tar.gz"
curl -LO "https://github.com/ayagmar/niri-computer-use/releases/download/v0.1.1/niri-computer-use-0.1.1-$arch-linux.tar.gz.sha256"
sha256sum -c "niri-computer-use-0.1.1-$arch-linux.tar.gz.sha256"
tar -xzf "niri-computer-use-0.1.1-$arch-linux.tar.gz"
install -Dm755 -t ~/.local/bin "niri-computer-use-0.1.1-$arch-linux/niri-computer-use"
~/.local/bin/niri-computer-use --version
```

The `.sha256` catches a download that is broken or cut off. It isn't a signature: it comes from the same release as the tarball, so whoever can change the release's files can change both.

The tarball also holds the skill, with the README, the changelog and the licenses of the crates built into the binary. Copy the skill somewhere it stays and link it where your agents look for skills:

```sh
mkdir -p ~/.local/share/niri-computer-use ~/.claude/skills ~/.agents/skills
cp -r "niri-computer-use-0.1.1-$arch-linux/skills" ~/.local/share/niri-computer-use/
ln -s ~/.local/share/niri-computer-use/skills/niri-computer-use ~/.claude/skills/niri-computer-use
ln -s ~/.local/share/niri-computer-use/skills/niri-computer-use ~/.agents/skills/niri-computer-use
```

To update, install the new release's binary the same way. Remove the old skill before copying the new one, or files the new release dropped stay behind; the links keep working:

```sh
rm -rf ~/.local/share/niri-computer-use/skills
cp -r "niri-computer-use-0.1.1-$arch-linux/skills" ~/.local/share/niri-computer-use/
```

## Updating while agents run

You can update from 0.1.0 or 0.1.1 while agents run. Before each release, a test installs every earlier release with the install script, leaves agents running on it, and updates over it while one of them is mid-input; these steps are what it checks.

1. Install the new release the way you installed the old one, with the [install script](../install-script/#update) or by hand. The binary is replaced by a rename, so every server, bridge and [shared](../../concepts/configuration/#shared) engine already running keeps the build it started with, and its clients' `status.engine.replaced` turns `true`.
2. Restart every MCP client, that is every agent session, so that its server runs the new build. Until you do, old and new servers share the lease, the stop flag and the input-dirty marker, and each reads the other's marker. The old engine keeps serving the clients it has, and exits two seconds after the last of them ends; the next client of the new build then starts an engine of the new build. A client started after the update is served standalone meanwhile, and its `status.engine.fallback` says the old engine refused it, names that engine's PID, and says to restart the clients it serves. A client of the old build whose engine has ended starts an engine of the old build again if no engine of the new build runs, and the new clients wait it out standalone as before. If one does run, it refuses the old client: after one `engine_lost`, its calls fail with `engine_unavailable`, with a `detail` saying to restart that client. Either way, restarting the old clients is what ends the old build.
3. Check that nothing of the old build still runs. The kernel marks a process whose file the update replaced with `(deleted)` after its path:

   ```sh
   pgrep -a '^niri-computer-u'
   for pid in $(pgrep '^niri-computer-u'); do ls -l "/proc/$pid/exe"; done
   ```

4. If an agent still gets `recovery_required` once the old clients have ended, run `niri-computer-use recover` from the new build. It reads the markers 0.1.0 and 0.1.1 write, releases the keys and buttons they name, and ends the `wtype` child a 0.1.0 marker names, which can still be running after its server was killed, then zeroes the modifiers that child may have held. 0.1.1 read 0.1.0's marker as unreadable and left that child running.

A policy file written for 0.1.0 or 0.1.1 loads unchanged. A key added later, such as `turn_on_accessibility`, makes an older build read the whole file as invalid, which leaves its servers read-only, so add new keys once nothing of the older build runs. Going back to an older release isn't covered by the test.

Older development builds keep the lease, the stop flag and the input-dirty marker under `XDG_RUNTIME_DIR` rather than beside niri's socket, and don't take the marker's lock, so an old and a new server on one niri would act as if the other weren't there. When updating from one of those, end every running `niri-computer-use` process first, and start your clients again once `pgrep` lists nothing.

## Check it

From a shell inside your niri session, with the binary where you put it:

```sh
~/.local/bin/niri-computer-use status
```

It prints the readiness report as JSON. Look at these fields:

| Field | Healthy value |
|---|---|
| `niri.compat` | `ok`; `read_only` means this build doesn't support your niri |
| `niri.error` | `null`; a `detail` of `NIRI_SOCKET is not set` means the server found no running niri of yours |
| `lock.state` | `unlocked`; `unknown` refuses the lease, see [Troubleshooting](../../reference/troubleshooting/#the-lease-is-refused-with-screen_locked-while-the-screen-is-unlocked) |
| `outputs.pointer_supported` | `true` for the pointer tools |
| `binaries` | `grim` `true`; `notify-send` matters only with [`confirm_lease`](../../concepts/configuration/#confirm_lease) |
| `desktop_programs.loginctl` | a path such as `/usr/bin/loginctl` |
| `desktop_programs.noctalia` | with Noctalia installed, a path such as `/usr/bin/noctalia`; `null` when it is installed elsewhere, such as a Nix user profile or `~/.local/bin`: set [`NIRI_COMPUTER_USE_DESKTOP_PATH`](../../concepts/configuration/#environment) to its directories, the same for every client |

Every field is described under [`status`](../../tools/looking/#status).

Next: [register the server in your agent](../clients/).
