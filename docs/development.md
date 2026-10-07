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

You'll need niri 26.04, `dbus-run-session` (from `dbus`), `grim`, `wev`, `wtype` 0.4 and `stdbuf` (from coreutils). `make nested` builds the `vpointer` probe first. The nested niri window stays open while the checks run, then closes.

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
6. The supervisor runs inside the nested niri. It checks that `NIRI_SOCKET`, the Wayland socket and the D-Bus socket resolve, following symlinks, to paths under `TEST_DIR/run`. Over one connection, it asks niri for its version and outputs and requires `winit` to be the only output. If the endpoints resolve elsewhere or niri reports any other output, the supervisor sends nothing more to that niri, starts nothing, and a 60-second deadline kills the whole process group instead. Otherwise it:
   - checks that `winit` has the configured scale and transform `Flipped180`, and saves a screenshot
   - starts `wev` and waits until niri reports it as a 400x300 floating window
   - captures the output with `grim` and checks that everything that isn't magenta is a box where niri placed `wev` (C3)
   - moves the pointer into `wev` with the `vpointer` probe and waits until `wev` logs `wl_pointer.enter` at that point, or that motion if the pointer was already inside, then moves it to five surface points and one image pixel, and checks where `wev` saw each motion (C4)
   - clicks, and scrolls one wheel notch, and checks what `wev` logged (C12)
   - waits for `wev` to log `wl_keyboard.enter`, then checks 20 Ctrl+a calls and 20 stdin `Hello` calls (C5(a)); a failure stops the run before the remaining keyboard checks
   - holds `wtype` stdin open for a full two seconds, checks that the child stays alive and sends no keys or modifiers, then closes it and checks exactly one Ctrl+a pair (C5(b))
   - sends Ctrl+Shift+F12 through `wtype`, checks the decoded pair and modifiers, then watches for a full second that `bind-fired` stays absent (C9 virtual half; the physical control is unverified)
   - types the committed 100-character corpus five times and compares the decoded text exactly, recording each duration and min/median/max (C10)
   - captures the output 20 times, PNG and JPEG at `-s 1` and `-s 0.5`, and checks each image's size (C15)
   - stops `wev` and tells the nested niri to quit on the connection it identified
7. Takes the host snapshot again, reports any difference, and removes `TEST_DIR`.

Every process the harness starts has a deadline, and a watchdog kills it when the deadline passes, even one like `wev` that runs in the background. Anything that ends after its deadline counts as timed out. `harness run` starts each process in its own process group and kills that group when the process exits, when the deadline passes, or when you press Ctrl+C. The supervisor's steps stay in the nested niri's group, so that kill covers them too. Before each one, the supervisor checks the nested endpoints again. Each wait polls every 50 ms until its deadline, ignores a result that arrives after it, and on timeout saves `failure-<step>.png`.

The runner can start a step with stdin held open. `feed` writes every byte and closes the pipe, within the child’s original deadline; a watchdog kills a child that stops reading. Keyboard checks use a three-second child deadline. `still_absent` polls for the full interval, including at its end, and saves a failure screenshot if an event appears.

Keyboard checks read the complete key and modifier records, including continuation lines, after an offset taken before each call. They check ordered press/release pairs, matching keycodes and symbols, decoded text, and chord modifiers at the keys and after release. The corpus is `probes/keyboard/corpus.txt`: 100 Unicode scalar values, 104 UTF-8 bytes, with ASCII, `é`, `ß` and `→`, and no trailing newline. No separate stdin-gate probe is needed.

Each pointer probe call sends its own event time, and niri passes that time on to `wev`'s events. The checks only read events with their own time, so motion from your own mouse over the nested window isn't mistaken for the probe's.

Each run keeps its files in `target/e2e/<unix time>-<pid>/`:

| File | Contents |
|---|---|
| `harness.log` | the terminal output |
| `niri.kdl` | the generated niri config |
| `niri.log` | output of `dbus-run-session` and the nested niri |
| `supervise.log` | the nested environment, each check and its result, and what the probe sent |
| `supervise.status` | `pass`, or `fail: <reason>` |
| `wev.log` | everything `wev` printed |
| `success-verify-niri.png` | the nested output before `wev` starts |
| `success-c3.png` | the nested output with `wev` |
| `failure-<step>.png` | the nested output when a wait timed out |

## Probes

Probes are small standalone programs in `probes/`, outside the Cargo workspace, so `make check` doesn't cover them. Run their tests with Cargo:

```sh
cargo test --locked --manifest-path probes/vpointer/Cargo.toml
```

`vpointer` creates a virtual pointer bound to one output, sends one action and exits. Run it only through `make nested`: the supervisor passes it `winit`, and only after the endpoint and output checks. The probe itself only checks that the output it was given exists before it creates the pointer. For a motion it prints the `motion_absolute` arguments it encoded and where niri's own mapping puts them, and refuses to send one that lands more than 0.002 px from the target.

## Host capture

C15 also measures captures of a real output. This reads `niri msg --json outputs` and runs `grim` 20 times on the output you name. The images stay in memory and only their sizes and timings are kept:

```sh
make host-capture OUTPUT=DP-1
```

The report goes to the terminal and to `target/e2e/<unix time>-<pid>/harness.log`.
