# Development

The repository pins Rust 1.99.0 with rustfmt and Clippy. Cargo builds both the main binary and the `harness` workspace member.

## Checks

- `make lint` runs rustfmt and Clippy. The pre-commit hook runs this target.
- `make check` runs formatting, Clippy, rustdoc, tests, cargo-deny, cargo-machete, typos, and ShellCheck.
- `make coverage` invokes cargo-llvm-cov. Install that optional tool before using the target.

Run `make check` before each commit. Enable the tracked hook with:

```sh
git config core.hooksPath .githooks
```

The dependency gate accepts MIT, Apache-2.0, BSD-2-Clause, BSD-3-Clause, ISC, Unicode-3.0, Zlib, and MPL-2.0 licenses. It rejects wildcard requirements, advisories, unmaintained crates, yanked crates, Git dependencies, and registries other than crates.io. Duplicate versions are warnings until an explicit skip list is needed.

## Nested harness

The `harness` binary starts a nested niri in a window on your desktop and checks that its sockets, its session bus and its config are separate from your real session. Run it from the repository root:

```sh
make nested            # output scale 1
make nested SCALE=1.5
```

You'll need niri 26.04, `dbus-run-session` (from `dbus`) and `grim`. The nested niri window opens and closes within a second or two.

The nested niri's window has the app-id `niri`. To keep it from moving your tiled layout or taking focus, add this rule to your own niri config:

```kdl
window-rule {
    match app-id="^niri$"
    open-floating true
    open-focused false
    default-column-width { fixed 960; }
    default-window-height { fixed 720; }
    default-floating-position x=16 y=16 relative-to="bottom-right"
}
```

Later steps need the nested output to be at least 400x300 logical pixels at scale 1.5, which 960x720 gives.

What a run does:

1. Creates a fresh `TEST_DIR` at `$XDG_RUNTIME_DIR/niri-desktop-mcp-test/<unix time>-<pid>/`, mode 0700, with `run/`, `state/`, `cache/`, `config/` and `data/`. The `niri-desktop-mcp-test` directory itself stays after the run, empty.
2. Writes a niri config (no startup commands, animations, borders or Xwayland; a magenta background; a fixed 400x300 floating `wev`; one `Ctrl+Shift+F12` test bind) and checks it with `niri validate`.
3. Builds the environment for the nested niri from scratch. The XDG and Noctalia directories point into `TEST_DIR`, `WAYLAND_DISPLAY` is the absolute path of your Wayland socket, and `HOME`, `PATH` and `LANG` are kept. `NIRI_SOCKET`, `WAYLAND_SOCKET`, `DISPLAY`, `XDG_SESSION_ID` and `DBUS_SESSION_BUS_ADDRESS` are not set. The run stops if any other variable is present or a path points outside `TEST_DIR`.
4. Records a host snapshot: `niri msg --json outputs`, `noctalia msg status`, the entries of `$XDG_RUNTIME_DIR` and `/tmp/.X11-unix`, and the modification time of `~/.config/dconf/user`.
5. Runs `dbus-run-session --config-file=… -- niri -c … -- harness supervise …`. The private bus listens only in `TEST_DIR/run` and activates no services.
6. The supervisor runs inside the nested niri. It checks that `NIRI_SOCKET`, the Wayland socket and the D-Bus socket resolve, following symlinks, to paths under `TEST_DIR/run`. Over one connection, it asks niri for its version and outputs and requires `winit` to be the only output. It checks that `winit` has the configured scale and transform `Flipped180`, saves a screenshot with `grim`, and tells the nested niri to quit on that same connection. If the endpoints resolve elsewhere or niri reports any other output, the supervisor sends nothing more to that niri, and a 60-second deadline kills the whole process group instead.
7. Takes the host snapshot again, reports any difference, and removes `TEST_DIR`.

Every process the harness starts has a deadline. `harness run` starts each one in its own process group and kills that group when the process exits, when the deadline passes, or when you press Ctrl+C. The supervisor's steps stay in the nested niri's group, so that kill covers them too.

Each run keeps its files in `target/e2e/<unix time>-<pid>/`:

| File | Contents |
|---|---|
| `harness.log` | the terminal output |
| `niri.kdl` | the generated niri config |
| `niri.log` | output of `dbus-run-session` and the nested niri |
| `supervise.log` | the nested environment and the checks the supervisor ran |
| `supervise.status` | `pass`, or `fail: <reason>` |
| `success-verify-niri.png` | the nested output |
