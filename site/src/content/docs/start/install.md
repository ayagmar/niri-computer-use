---
title: Install
description: Build niri-computer-use from source and check it against your niri session.
sidebar:
  order: 1
---

niri-computer-use is installed from source. There is no package yet.

## Requirements

- niri 26.04. With another major or minor version the server is read-only: it looks but refuses to act.
- Rust 1.99.0. The repository pins it in `rust-toolchain.toml`, so rustup installs it when you build.
- libxkbcommon, which the keyboard code links against (`libxkbcommon-dev` on Debian and Ubuntu, `libxkbcommon` on Arch).
- At run time: `grim` for screenshots, `wl-paste` from wl-clipboard to read the clipboard, `wtype` for the keyboard, and `loginctl` from systemd for the lock state.
- Optional: [Noctalia](../../concepts/noctalia/) 5.2 for its panels and lock state, and an accessibility bus (at-spi2-core) for [`elements`](../../tools/elements/).
- One monitor, for the pointer tools. `pointer_move`, `click`, `drag` and `scroll` run only with one enabled output at transform `Normal`; with a second monitor or a rotated one they refuse with [`untested_output_config`](../../reference/troubleshooting/#the-pointer-tools-fail-with-untested_output_config). The other tools, the keyboard and screenshots among them, work with any monitors.

## Build and install

```sh
git clone https://github.com/ayagmar/niri-computer-use.git
cargo install --locked --path niri-computer-use
```

The binary goes to `~/.cargo/bin/niri-computer-use`. To update, pull and run the same `cargo install` again.

Before updating, end every running `niri-computer-use` process: close the agent sessions that run `serve`, and with [`shared`](../../concepts/configuration/#shared) on, wait for the engine to exit, two seconds after its last client, or end it. This lists any still running:

```sh
pgrep -a '^niri-computer-u'
```

Older development builds keep the lease, the stop flag and the input-dirty marker under `XDG_RUNTIME_DIR` rather than beside niri's socket, and don't take the marker's lock, so an old and a new server on one niri would act as if the other weren't there. Start your clients again once nothing is listed.

## Check it

From a shell inside your niri session:

```sh
~/.cargo/bin/niri-computer-use status
```

It prints the readiness report as JSON. Look at these fields:

| Field | Healthy value |
|---|---|
| `niri.compat` | `ok`; `read_only` means this build doesn't support your niri |
| `niri.error` | `null`; `NIRI_SOCKET is not set` means the server found no running niri of yours |
| `lock.state` | `unlocked`; `unknown` refuses the lease, see [Troubleshooting](../../reference/troubleshooting/#the-lease-is-refused-with-screen_locked-while-the-screen-is-unlocked) |
| `outputs.pointer_supported` | `true` for the pointer tools |
| `binaries` | every program `true` |

Every field is described under [`status`](../../tools/looking/#status).

Next: [register the server in your agent](../clients/).
