---
title: Install
description: Install niri-computer-use from the release tarball, crates.io or source, and check it against your niri session.
sidebar:
  order: 1
---

## Requirements

- niri 26.04. With another major or minor version the server is read-only: it looks but refuses to act.
- For a release build, x86_64 or aarch64 Linux with glibc 2.39 or newer; both builds have the same requirements. To build it yourself, Rust 1.99 or newer.
- libxkbcommon, which the keyboard code links against. To build, its headers too (`libxkbcommon-dev` on Debian and Ubuntu, `libxkbcommon` on Arch).
- At run time: `grim` for screenshots, `wtype` for the keyboard, and `loginctl` from systemd for the lock state.
- Optional: [Noctalia](../../concepts/noctalia/) 5.2 for its panels and lock state, and an accessibility bus (at-spi2-core) for [`elements`](../../tools/elements/).
- One monitor, for the pointer tools. `pointer_move`, `click`, `drag` and `scroll` run only with one enabled output at transform `Normal`; with a second monitor or a rotated one they refuse with [`untested_output_config`](../../reference/troubleshooting/#the-pointer-tools-fail-with-untested_output_config). The other tools, the keyboard and screenshots among them, work with any monitors.

## Install the release

The install script picks the tarball for your machine, checks it against its `.sha256`, puts the binary in `~/.local/bin`, copies the skill that tells your agents how to use the server to `~/.local/share/niri-computer-use`, and links it from `~/.claude/skills` and `~/.agents/skills`. Download it, read it, then run it:

```sh
curl -fsSLO https://github.com/ayagmar/niri-computer-use/releases/latest/download/install.sh
less install.sh
sh install.sh
```

| Variable | Effect |
|---|---|
| `NCU_VERSION` | The release to install, such as `0.1.0`, instead of the latest |
| `NCU_BIN_DIR` | Where the binary goes, instead of `~/.local/bin` |

It refuses a tarball that doesn't match its `.sha256`, and a binary that doesn't run on your system, such as one that needs a newer glibc, before it changes anything. It copies the new binary and skill beside the old ones before replacing either, so a copy that fails, on a full disk for example, leaves the previous install as it was.

Run it again to update: it replaces the binary and the skill. It points `~/.claude/skills/niri-computer-use` and `~/.agents/skills/niri-computer-use` at the skill it installed. A link there that points elsewhere, such as into a clone, is replaced and named in the output; a directory there is left alone.

The `.sha256` catches a download that is broken or cut off. It isn't a signature: it comes from the same release as the tarball and the script, so whoever can change the release's files can change all three.

### By hand

Download your machine's tarball, check it, and put the binary in `~/.local/bin` or another directory on your `PATH`. `uname -m` prints `x86_64` or `aarch64` on the machines there are builds for:

```sh
arch=$(uname -m)
curl -LO "https://github.com/ayagmar/niri-computer-use/releases/download/v0.1.0/niri-computer-use-0.1.0-$arch-linux.tar.gz"
curl -LO "https://github.com/ayagmar/niri-computer-use/releases/download/v0.1.0/niri-computer-use-0.1.0-$arch-linux.tar.gz.sha256"
sha256sum -c "niri-computer-use-0.1.0-$arch-linux.tar.gz.sha256"
tar -xzf "niri-computer-use-0.1.0-$arch-linux.tar.gz"
install -Dm755 -t ~/.local/bin "niri-computer-use-0.1.0-$arch-linux/niri-computer-use"
~/.local/bin/niri-computer-use --version
```

The tarball also holds the skill, with the README, the changelog and the licenses of the crates built into the binary. Copy the skill somewhere it stays and link it where your agents look for skills:

```sh
mkdir -p ~/.local/share/niri-computer-use ~/.claude/skills ~/.agents/skills
cp -r "niri-computer-use-0.1.0-$arch-linux/skills" ~/.local/share/niri-computer-use/
ln -s ~/.local/share/niri-computer-use/skills/niri-computer-use ~/.claude/skills/niri-computer-use
ln -s ~/.local/share/niri-computer-use/skills/niri-computer-use ~/.agents/skills/niri-computer-use
```

To update, install the new release's binary the same way. Remove the old skill before copying the new one, or files the new release dropped stay behind; the links keep working:

```sh
rm -rf ~/.local/share/niri-computer-use/skills
cp -r "niri-computer-use-0.1.0-$arch-linux/skills" ~/.local/share/niri-computer-use/
```

## Install with cargo

From crates.io, into `~/.cargo/bin`:

```sh
cargo install --locked niri-computer-use
```

The crate has no skill; take it from the release tarball or a clone.

From source, run `cargo install` inside the clone. rustup takes the Rust version the repository pins from `rust-toolchain.toml` in the current directory, so outside the clone it builds with your default toolchain instead:

```sh
git clone https://github.com/ayagmar/niri-computer-use.git
cd niri-computer-use
cargo install --locked --path .
```

The skill is in the clone at `skills/niri-computer-use`. To update, run `git pull` and the same `cargo install` again, inside the clone.

Updating while agents run is safe. A [shared](../../concepts/configuration/#shared) engine keeps serving the clients it has with the build it started with, and their `status.engine.replaced` turns `true`. Clients started after the update are served standalone, sharing the lease, the stop flag and the input-dirty marker, and their `status.engine.fallback` names the old engine's PID. Once the old engine's last client has ended it exits, two seconds later, and the next client starts an engine of the new build. Restart your agent sessions to move all of them to the new build. This lists the processes still running:

```sh
pgrep -a '^niri-computer-u'
```

Older development builds keep the lease, the stop flag and the input-dirty marker under `XDG_RUNTIME_DIR` rather than beside niri's socket, and don't take the marker's lock, so an old and a new server on one niri would act as if the other weren't there. When updating from one of those, end every running `niri-computer-use` process first, and start your clients again once nothing is listed.

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
| `binaries` | `grim` and `wtype` `true`; `notify-send` matters only with [`confirm_lease`](../../concepts/configuration/#confirm_lease) |
| `desktop_programs.loginctl` | a path such as `/usr/bin/loginctl` |
| `desktop_programs.noctalia` | with Noctalia installed, a path such as `/usr/bin/noctalia`; `null` when it is installed elsewhere, such as a Nix user profile or `~/.local/bin`: set [`NIRI_COMPUTER_USE_DESKTOP_PATH`](../../concepts/configuration/#environment) to its directories, the same for every client |

Every field is described under [`status`](../../tools/looking/#status).

Next: [register the server in your agent](../clients/).
