# Development

The repository pins Rust 1.99.0 with rustfmt and Clippy. Cargo builds both the main binary and the `harness` workspace member.

## Checks

- `make lint` runs rustfmt and Clippy. The pre-commit hook runs this target.
- `make check` runs three targets. `make check-static` runs formatting, Clippy, rustdoc, cargo-deny, cargo-machete, typos, and ShellCheck. `make test` runs every test target, with the protocol tests in standalone mode, and `make test-shared` runs the protocol tests in shared mode. CI runs the three as parallel jobs, so it runs the same commands as `make check`. A newer push to the same branch or pull request cancels CI's older run; the run a release calls is never cancelled.
- `make perf` runs the unit tests with `NCU_PERF=1`, which also checks how long a zero-length event wait takes. `make check` leaves that bound out, because a loaded machine can miss it without a bug.
- `make coverage` invokes cargo-llvm-cov. Install that optional tool before using the target.

Run `make check` before each commit. Enable the tracked hook with:

```sh
git config core.hooksPath .githooks
```

The dependency gate accepts MIT, Apache-2.0, BSD-2-Clause, BSD-3-Clause, ISC, Unicode-3.0, Zlib, and MPL-2.0 licenses. The one exception is GPL-3.0-or-later for `niri-ipc` (see [decisions](decisions.md)). It rejects wildcard requirements, advisories, unmaintained crates, yanked crates, Git dependencies, and registries other than crates.io. Duplicate versions are warnings until an explicit skip list is needed. cargo-deny doesn't check the `harness` crate's dependencies, because the harness isn't published.

## Unit tests against a fake niri

The native keyboard, held pointer modifiers, the crash guardian and the paste keeper talk to niri's Wayland display. Their unit tests run against `test_support::wayland::Compositor`, a display and a niri socket served from a thread of the test's own process. It models what niri's Smithay does with virtual keyboards, keymaps and data-control (see [decisions](decisions.md), 2026-10-12), and records what the focused client would have been sent: keys, modifiers, keymaps, buttons and the selection's types. It also serves niri's event stream, with one focused window whose app id is `test_support::wayland::APP_ID`, so a test can type through `type_input`, the native keyboard's entry point. A test can change niri's keyboard configuration, switch its layout, press a physical key, delay niri's acknowledgement of each key, or copy something, including while a client reads the selection.

## Protocol tests

`tests/protocol/` starts the server binary and speaks MCP to it over stdin and stdout, as a client would. `make check` runs it, and so does:

```sh
cargo test --locked --test protocol
```

Each test gets a directory of its own under `/tmp`, whatever `TMPDIR` says, because the fake sockets inside it must fit the 108-byte limit on Unix socket paths. It starts the server with only eight variables set, all pointing into it, and a ninth, `NIRI_COMPUTER_USE_SHARED=0`, in the standalone run:

- `PATH` holds fake `grim`, `loginctl` and `noctalia` scripts and nothing else, and `NIRI_COMPUTER_USE_DESKTOP_PATH` names the same directory, so the server never finds the host's `loginctl` or `noctalia`.
- `NIRI_SOCKET` is a fake niri that answers `Version`, `Outputs`, `FocusedOutput`, `Windows` and `Workspaces` and lets the test write each event stream line by line.
- `XDG_RUNTIME_DIR` and `WAYLAND_DISPLAY` lead to a fake Noctalia socket when the test starts one, and to the Wayland display, served by a fake clipboard owner (`tests/protocol/clipboard.rs`) with nothing copied, so the clipboard watcher `acquire_desktop` starts can bind: just enough of the Wayland wire protocol and wlr data-control to announce selections and answer their reads. The clipboard tests copy through it.
- `XDG_SESSION_ID` is `7`. The server ignores it: the lock state asks logind about niri's own session, which the lock tests set by running the fake niri as a process of its own (`tests/protocol/session.rs`).
- `XDG_STATE_HOME` keeps the audit log inside the directory, and `XDG_CONFIG_HOME` the policy file.

The element tests (`tests/protocol/elements.rs`) add `DBUS_SESSION_BUS_ADDRESS`: a `dbus-daemon` of the test's own (from `dbus`), listening only in the test's directory with no service directories, which serves as both the session bus and the accessibility bus. On it, `tests/protocol/atspi.rs` runs a mock application that answers AT-SPI as a toolkit does, and whose objects, and the next reply of each method, the test changes, holds or fails. Its names and text are synthetic.

Every reply the fakes give passes a named hold point (`tests/protocol/holds.rs`): `niri:Version` or `niri:Action:FocusWindow` for the fake niri, `noctalia:status` for the fake Noctalia, `loginctl` for the `loginctl` the lock tests start (`session::held_loginctl`), `wtype` for the explorer's `wtype` once it read its stroke, `atspi:GetRole:/button` for a method or property of one of the mock application's objects, and `niri:effect` for the events the fake niri can send once the server read its answer to an action. A fake program passes a point with the shell function `pass <point>` once the test calls `Fixture::serve_programs`. The points passed since the trace started are the trace of the calls in between, in order, and a test can hold any one of them back until it releases it.

The stall explorer (`tests/protocol/stall.rs`) uses both. It calls each acting tool until two calls in a row pass the same points, which are its trace, then, for every point in that trace and every fact the tool's gates depend on, calls the tool again on a desktop of its own, holds the reply at that point (up to three calls, since a lock read made again because the last one had aged is in the trace only on a machine slow enough to need it), changes the fact, waits until the server can see the change, and releases the reply at once or after 300 ms, past the 250 ms bound on the age of what the gate read. The facts are the stop flag being set, the lease file replaced and locked by another server, another input's marker being set, the screen locking (Noctalia and logind both; in the full matrix those worlds run the fake niri behind `niri --session`, so logind's answer is a point too), keyboard focus, the window closing, the window's `app_id` becoming one the policy denies, and an element becoming a password field, being renamed, its parent renamed, moved to another window, its actions reordered, or removed. Then it checks what the fakes received: every call that acts after the change went out within 250 ms of it, and no more went out than with nothing held. A change before the first call that acts refuses the call with the fact's error, or lets it go out within those 250 ms. A change after it leaves the call unrefused: a stop or a replaced lease makes it `uncertain`, with `accepted` null until the reply came and true after it, no screenshot and nothing more asked, and any other change reports no focus from before the change as the focus afterwards. A tool with several calls that act, `type_text`, may have a later one refused with the fact's error, saying what went out before it, except for a stop or a replaced lease. A new await in the server is a new point in the trace, explored without new test code. It makes eleven calls: `activate_element`, `set_element_text`, `focus` with a window, `close_window`, `niri_action`, `launch`, `key`, `type_text`, `shell_panel` opening a panel and closing one, and `noctalia`. `type_text` types 150 characters and submits them: three `wtype` calls, each of which counts as going out once the fake `wtype` read it. The pointer tools and a paste act through Wayland, which only the nested runs have.

`make check` explores only `activate_element`, `focus` with a window, `key` and `shell_panel` opening a panel, one tool of each kind, against every fact it depends on. `make stall` explores all eleven, in both server modes. It runs `make stall-standalone` and `make stall-shared`, which CI runs as two more parallel jobs:

```sh
make stall
NCU_STALL=full cargo test --locked --test protocol stall::
```

Nothing reaches your desktop, clipboard, session or audit log.

The tests check:

- the handshake, version negotiation, the server's instructions, and requests before `initialize`
- the tool list with and without `noctalia` on `PATH`, with each tool's schema, title, annotations and `_meta`
- that every line on stdout is a JSON-RPC message
- both error shapes: argument mistakes as plain text, and execution failures with their stable name and upstream detail
- screenshots: the image block, its metadata, and grim's arguments for each target, format and scale
- cancellation of a running `grim`, clipboard read or niri request: the process group is killed or the connection closed, no response is sent, the audit log says `cancelled`, and the session keeps working
- deadlines, and calls running at the same time
- event-stream reconnects that never serve the previous desktop, and the malformed-event rule
- Noctalia running, stopped and absent, the lock state's sources, and the clipboard's outcomes: only a copy made during the lease read, a copy marked secret withheld unread, and the watcher's connection closed once the lease is gone
- the audit log's lines, modes and contents, and a failed write showing in `status`
- the element actions against the mock application: what reaches it, and that its error messages reach neither the result nor the audit log
- the acting tools against every stall point the explorer finds

The `grim` deadline test takes five seconds, the length of that deadline.

`make check` runs the suite twice: standalone, with `NIRI_COMPUTER_USE_SHARED=0` in every fixture, then in shared mode, where every server the tests start bridges to the fixture's shared engine:

```sh
NCU_PROTOCOL_MODE=shared cargo test --locked --test protocol
```

A few tests check what differs by design in shared mode: the lease holder's PID and the crash guardian belong to the engine, which `status.engine.pid` names; the engine's and its guardian's stderr is `engine.log` in the runtime directory; a client that leaves mid-action returns at once while the engine finishes the input cleanup; and a failed MCP handshake is the engine's, which logs it, while the bridge only relays. `tests/protocol/shared.rs` covers what only shared mode does, in either run: ten clients on one engine and one guardian, each client's own `unrestricted`, the fallbacks to standalone, a lost engine, an oversized client line, and fifty clients coming and going that leave the engine's sessions and file descriptors as they were.

## MCP Inspector

`scripts/inspector.sh` runs the server under the pinned MCP Inspector, 2.9.0, inside the niri session your shell is in. It needs Node.js 22.19 or newer for `npx`, and `jq`.

```sh
make inspect          # the Inspector's web UI, which opens in your browser
make inspect-check    # list every tool and call the read-only ones through the Inspector's CLI
```

The Inspector starts the server with a minimal environment, so the script passes on `NIRI_SOCKET`, `XDG_RUNTIME_DIR` and `WAYLAND_DISPLAY` from your shell, which pins the server to that session; without them the server would discover them, as it does for Codex. It finds the session bus under `XDG_RUNTIME_DIR`. It points `XDG_STATE_HOME` and the Inspector's own settings at a temporary directory, which is removed when the script exits, so your audit log isn't touched.

`make inspect-check` lists the tools with `--strict`, which also reports schema portability problems, and checks that exactly the observation tools are marked read-only. It expects `shell_panel` when `noctalia` is on `PATH`, `noctalia` too when `status.unrestricted.enabled` is true, and the element tools when `status` finds the accessibility bus. It then calls only read-only tools on your desktop, so it never takes the lease: `status`, `desktop_state` (with its outputs, and its `shell` when `noctalia` is on `PATH`), `elements` on the focused window when the accessibility bus is there, `clipboard_read`, which must refuse with `lease_required`, and three screenshots. It prints one line per check and never prints the image data, the clipboard text, window titles or accessible names. Finally it checks the audit log in the temporary directory: one line per call, mode `0600`, in a `0700` directory.

## Docs site

The user documentation is an Astro and Starlight site in `site/`. Build it with Node.js 22.12 or newer:

```sh
cd site
npm ci
npm run build
```

The pages are Markdown files in `site/src/content/docs/`, except the home page, `index.mdx`, which places the components in `site/src/components/`. There is one directory per sidebar group (`site/src/lib/groups.mjs`), and pages are ordered by `sidebar.order` in their front matter. The build goes to `site/dist/`. Preview it with:

```sh
npm run preview
```

`npm run build` also runs two checks, and fails on either:

- `scripts/check-reference.mjs`, before Astro: the error reference lists exactly the names in `ErrorName` (`src/error.rs`), and the configuration page exactly the keys of `Policy` and `Preset` (`src/policy.rs`). Change the page in the same commit as the enum or struct.
- `scripts/check-links.mjs`, after Astro: every internal link, image and `#fragment` resolves to a built file, and `llms.txt` and `llms-full.txt` cover every page.

Besides the HTML, the build writes a Markdown copy of every page (`<page>.md`), `llms.txt`, `llms-full.txt`, and the skill from `skills/niri-computer-use/` under `skill/`. Images for the site come from nested runs (`target/e2e/<run>/`), never from your own screen.

CI builds the site on every push and pull request. `.github/workflows/pages.yml` deploys it to GitHub Pages on every push to `main` that touches `site/` or `skills/`.

## Nested harness

The `harness` binary starts a nested niri inside a private headless cage by default. The nested session inherits no host display or bus connection, and the harness checks that the nested sockets, session bus and config are separate from your real session. Both headless and visible runs use C1's read-only host metadata snapshots as a contamination backstop. `VISIBLE=1` (or `harness run --visible`) selects the visible window for human sittings; `make sitting` selects it automatically. Run it from the repository root:

```sh
make nested                       # output scale 1
make nested SCALE=1.5
make nested NOCTALIA=1            # also start Noctalia in the nested session (C13)
make nested NOCTALIA=1 SCALE=1.5
```

You'll need cage 0.3.1 for headless runs, niri 26.04, `dbus-run-session` (from `dbus`), `grim`, `wev`, `wtype` 0.4 and `stdbuf` (from coreutils), and Noctalia 5.2.1 for `NOCTALIA=1`. `make nested` builds the server first, which `NOCTALIA=1` drives. With `tesseract` on `PATH`, `NOCTALIA=1` also reads the open control center's text (see below); without it, that check is skipped with a notice. Headless runs start cage through the runner with a deadline ten seconds longer than the nested suite. Its private runtime is `TEST_DIR/cage`, and its socket path must fit 108 bytes. Cage uses the headless backend without libinput devices and disables client decorations. Startup waits at most ten seconds, with no visible fallback. Cage's process group is stopped and reaped on completion or failure; `cage.log` stays with the artifacts.

In visible mode, the nested niri's window has the app-id `niri`. To keep it from moving your tiled layout or taking focus, add this rule to your own niri config:

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

1. Creates a fresh `TEST_DIR` at `$XDG_RUNTIME_DIR/ncu-test/<unix time>-<pid>/`, mode 0700, with `run/`, `state/`, `cache/`, `config/` and `data/`. The name is short so that the shared engine's socket under it fits a Unix socket's 108 bytes. The `ncu-test` directory itself stays after the run, empty.
2. Writes a niri config (no startup commands, animations, borders or Xwayland; a magenta background; a fixed 400x300 floating `wev`; one `Ctrl+Shift+F12` test bind) and checks it with `niri validate`.
3. Builds the environment for the nested niri from scratch. The XDG and Noctalia directories point into `TEST_DIR`, `WAYLAND_DISPLAY` is the absolute path of cage's private Wayland socket (your Wayland socket only in visible mode), and `HOME`, `PATH` and `LANG` are kept. `DBUS_SYSTEM_BUS_ADDRESS` points to `TEST_DIR/run/no-system-bus`, where nothing exists, so nothing in the nested session can reach your system bus. `NIRI_SOCKET`, `WAYLAND_SOCKET`, `DISPLAY`, `XDG_SESSION_ID` and `DBUS_SESSION_BUS_ADDRESS` are not set. The run stops if any other variable is present or a path points outside `TEST_DIR`.
   With `NOCTALIA=1` it also writes a Noctalia config that turns off the first-run setup wizard and weather, lists no plugin sources, and points the wallpaper directory at an empty `TEST_DIR/data/wallpapers`, so the wallpaper panel never lists your pictures, and checks it with `noctalia config validate`. That command exits 0 even when it warns, for example about an unknown key, so the harness requires its plain "Config is valid" line.
4. Records a host snapshot before starting cage or the nested session: `niri msg --json outputs`, `noctalia msg status`, the entries of `$XDG_RUNTIME_DIR` and `/tmp/.X11-unix`, and the modification time of `~/.config/dconf/user`. It also reads the harness's cgroup from `/proc/self/cgroup` and the user journal's current cursor (`journalctl --user --show-cursor -n 0`).
5. Runs `dbus-run-session --config-file=… -- niri -c … -- harness supervise …`. The private bus listens only in `TEST_DIR/run` and activates no services.
6. The supervisor runs inside the nested niri. It checks that `NIRI_SOCKET`, the Wayland socket and the D-Bus socket resolve, following symlinks, to paths under `TEST_DIR/run`, that the Noctalia socket path is under it too, and that the system bus address is still `TEST_DIR/run/no-system-bus` with nothing there, not even a symlink. Over one connection, it asks niri for its version and outputs and requires `winit` to be the only output. If the endpoints resolve elsewhere or niri reports any other output, the supervisor sends nothing more to that niri, starts nothing, and a 60-second deadline kills the whole process group instead. Otherwise it:
   - checks that `winit` has the configured scale and transform `Flipped180`, and saves a screenshot
   - starts `wev` and waits until niri reports it as a 400x300 floating window
   - captures the output with `grim` and checks that everything that isn't magenta is a box where niri placed `wev` (C3)
   - waits for `wev` to log `wl_keyboard.enter`, then checks 20 Ctrl+a calls and 20 stdin `Hello` calls (C5(a)); a failure stops the run before the remaining keyboard checks
   - holds `wtype` stdin open for a full two seconds, checks that the child stays alive and sends no keys or modifiers, then closes it and checks exactly one Ctrl+a pair (C5(b))
   - sends Ctrl+Shift+F12 through `wtype`, checks the decoded pair and modifiers, then watches for a full second that `bind-fired` stays absent (C9 virtual half; the physical control runs only in `make sitting`)
   - types the committed 100-character corpus five times and compares the decoded text exactly, recording each duration and min/median/max (C10)
   - captures the output 20 times, PNG and JPEG at `-s 1` and `-s 0.5`, and checks each image's size (C15)
   - stops `wev`
   - with `NOCTALIA=1`, starts Noctalia and `target/debug/niri-computer-use`, and drives C13 through the server's tools. It waits up to ten seconds for `status` to show Noctalia running and the screen unlocked, requires `desktop_state`'s `shell` to show no open panel, and takes the lease. `shell_panel` opening `control-center` must then be accepted and observed `opened`, and closing it observed `closed`, each call returning within two seconds, so each change was seen in `activePanelId` within two seconds of sending. Before each call the supervisor checks that Noctalia's socket at `TEST_DIR/run/noctalia-<nested display>.sock` resolves under `TEST_DIR/run`, so a panel command can't reach your Noctalia. Before the open it waits until Noctalia has drawn its background (less than half the output is still magenta) and fewer than 1% of the pixels changed since the previous capture, and keeps that capture. It then waits up to five seconds until the open panel changes at least 1% of the output's pixels compared with it, and fails at once if 90% or more changed, which means the baseline was wrong, saves a screenshot, and after the close waits until fewer than 1% of the pixels differ from the capture taken before the open (C13). With `tesseract` on `PATH`, while the control center is open it also captures the output at twice its scale, runs `tesseract --psm 11` on it, and requires the word `Home`, the title of the control center's first tab in Noctalia's English strings; only whether the word was found is logged. This is a dev-only check, not a tool. Without `NOCTALIA=1` it logs that C13 was skipped. Noctalia starts after the `wev` checks because its bar reserves space at the top of the output, which would move `wev`.
   - tells the nested niri to quit on the connection it identified
7. Stops cage in headless mode, then takes the host snapshot again and reports any difference, even when the nested run failed. C1 reads only outputs, Noctalia status, directory entry names and dconf modification time, never screenshot pixels, clipboard text or window titles. Host metadata changes from unrelated activity also fail C1 conservatively; a difference is not proof that the harness caused it. Next it reads the user journal's entries since that cursor from the harness's cgroup, which every process of the run stays in, asking only for each entry's program name and PID, never its message, and fails the run if there is any. Programs that log to journald connect to `/run/systemd/journal/socket` directly, which the nested environment doesn't replace: Qt does unless `QT_FORCE_STDERR_LOGGING=1` is set, and so does `dbus-broker-launch`. Anything else in the same cgroup that logs meanwhile, such as the terminal the harness runs in, fails the check too. Then it lists every running process whose command line names `TEST_DIR`, fails the run if there is one, and removes `TEST_DIR`. Everything the run started should be gone by then, but a program can move a child into a process group of its own, out of reach of the group kill. The harness reports such processes and doesn't kill them.

Every process the harness starts has a deadline, and a watchdog kills it when the deadline passes, even one like `wev` that runs in the background. Anything that ends after its deadline counts as timed out. `harness run` starts each process in its own process group and kills that group when the process exits, when the deadline passes, or when you press Ctrl+C. The supervisor's steps stay in the nested niri's group, so that kill covers them too. Before each one, the supervisor checks the nested endpoints again. Each wait polls every 50 ms until its deadline, ignores a result that arrives after it, and on timeout saves `failure-<step>.png`.

The runner can start a step with stdin held open. `feed` writes every byte and closes the pipe, within the child’s original deadline; a watchdog kills a child that stops reading. Keyboard checks use a three-second child deadline. `still_absent` polls for the full interval, including at its end, and saves a failure screenshot if an event appears.

Keyboard checks read the complete key and modifier records, including continuation lines, after an offset taken before each call. They check ordered press/release pairs, matching keycodes and symbols, decoded text, and chord modifiers at the keys and after release. The corpus is `harness/corpus.txt`: 100 Unicode scalar values, 104 UTF-8 bytes, with ASCII, `é`, `ß` and `→`, and no trailing newline. No separate stdin-gate probe is needed.

The pointer checks of M0 (C4's accuracy, a click and C12's scroll order) ran through the `vpointer` probe, which M4 deleted once the server's own pointer replaced it. They run through the server in `make nested-input`.

Each run keeps its files in `target/e2e/<unix time>-<pid>/`:

| File | Contents |
|---|---|
| `harness.log` | the terminal output |
| `niri.kdl` | the generated niri config |
| `niri.log` | output of `dbus-run-session` and the nested niri |
| `supervise.log` | the nested environment, each check and its result, and the server's results |
| `supervise.status` | `pass`, or `fail: <reason>` |
| `wev.log` | everything `wev` printed |
| `noctalia.toml` | the generated Noctalia config, with `NOCTALIA=1` |
| `noctalia.log` | everything Noctalia printed, with `NOCTALIA=1` |
| `success-verify-niri.png` | the nested output before `wev` starts |
| `success-c3.png` | the nested output with `wev` |
| `server-harness-c13.log` | the server's replies, with `NOCTALIA=1` |
| `success-control-center.png` | the nested output with Noctalia's control center open |
| `failure-<step>.png` | the nested output when a wait timed out |

## Nested control checks

`make nested-control` runs M2's acceptance in a nested niri. It builds the server, starts the nested niri as `make nested` does, and starts Noctalia inside it, because a nested niri sets no logind lock hint and the lease needs a lock source that says unlocked. The supervisor then drives `target/debug/niri-computer-use` against the nested niri:

1. Waits until a server's `status` shows Noctalia running and the screen unlocked.
2. Starts server A, which takes the lease and keeps its stdin open for 25 seconds, and waits for its record in `lease.json`.
3. Starts server B, whose `acquire_desktop` must fail with `lease_held` naming server A.
4. Runs `recover`, which must refuse because a server holds the lease.
5. Asks niri to `spawn` `niri-computer-use stop`, as the stop keybind does, and waits for the flag and for server A to give the lease up. Server B must then get `stopped`; after `resume` it must take the lease.
6. Starts a child that runs until it is killed, writes an input-dirty marker naming it, and pipes `yes` into `recover`, which must end the child and clear the marker.

Every server talks MCP over a shell pipeline (`printf` of the requests, then `sleep` to keep stdin open), and every flag and marker lives in `TEST_DIR`. The run has a 90-second deadline. Its files are in `target/e2e/<run>/`, with `server-a.log` holding server A's replies.

## Nested action checks

`make nested-actions` runs M3's acceptance in a nested niri, set up as `make nested-control` is, with the nested Noctalia as the lock source. Before the server starts, the supervisor writes a policy file to `TEST_DIR/config/niri-computer-use/policy.toml` whose presets all start `harness window`, a fixture app in the harness, with a different `app_id` each:

| Preset | Fixture |
|---|---|
| `late` | sets its `app_id` 400 ms after its window is mapped |
| `two` | maps two windows |
| `reuse`, `plain` | one ordinary window |
| `keep` | ignores close requests, as an app asking about unsaved changes does |
| `slow` | writes a marker file at once and maps its window 2.5 seconds later |
| `sized` | draws at the size niri configures (`--resize`) |

`harness window` checks that its environment is the nested one before it connects, maps black 320x240 toplevels through `xdg_wm_base`, with `--resize` redraws at each size niri configures, with `--animate` attaches and damages its buffer again on every frame callback, and exits when its windows close, when niri goes away, or after 90 seconds. niri starts it from the preset with niri's own environment, so it never reaches the host compositor.

One server holds the lease for the whole run. The supervisor keeps its stdin open and reads its replies back from `server-harness-m3.log`. Through it, the run checks:

1. `launch late` gives `one`, and `desktop_state` shows the window with its late `app_id`.
2. `launch two` gives `ambiguous` with two windows.
3. `launch reuse` with `reuse: true` gives `one`; after `focus` on another window, the same call gives `focused` on that window and starts nothing; after a second plain launch, it gives `ambiguous` with `accepted: false`, and still nothing new starts.
4. `focus` on a window, then on the empty workspace, with no window focused, and back, with focus on the window it left, each `focused`.
5. `close_window` on a `plain` window gives `closed`; on a `keep` window, `pending` with a screenshot of the output in the result.
6. `launch slow`, and as soon as the marker appears the supervisor focuses the `late` window through its own niri connection: the launch gives `interrupted` naming that window, with a screenshot.
7. On a `sized` window, `niri_action` `FullscreenWindow` gives `changed` with `window.window_size` equal to the output's logical size, and again gives the tiled size back; `ToggleWindowFloating` gives `is_floating: true`; `SetWindowWidth` with `SetFixed: 400` gives a window 400 wide. `Spawn` is refused with `unrestricted_required`.
8. After the first server stops, the supervisor rewrites the policy file to `unrestricted = true` and starts a second server, which spawns `harness window … spawned` through `niri_action` `Spawn`; `wait_for` sees the window, and `close_window` closes it. Through the same server, `noctalia` `["status"]` returns the nested Noctalia's unlocked status as its reply, and `["no-such-command"]` gives `upstream_error` with Noctalia's message.

Each action's `changes` must also tell what it did: the window `launch late` opened and focused, both windows of `launch two`, the focus `launch reuse` moved, the workspace and focus `focus` on a workspace changed each way, the window `close_window` closed, where the interrupted launch's focus went, the window's new size after each `FullscreenWindow`, `floating` after `ToggleWindowFloating` and a width of 400 after `SetWindowWidth`. The `pending` close's and the `Spawn`'s reports are only logged. Each report is logged as `changes: <what>: checked <bool>, <bytes> bytes: <json>`.

The run has a 130-second deadline. Its files are in `target/e2e/<run>/`, including `policy.toml`, the server's replies and Noctalia's log.

## Nested input checks

`make nested-input` runs M4's acceptance of the pointer and keyboard tools in a nested niri, set up as `make nested-control` is, with the nested Noctalia as the lock source. `SCALE` sets the nested output's scale as for `make nested`; the plan's exit runs it at 1 and at 1.5. One server holds the lease for the whole run, and its replies are read back from `server-harness-m4.log`. `wev` starts after Noctalia, whose bar moves floating windows down when it appears, and the supervisor reads `wev`'s surface position from niri's window layout. Each check takes `wev`'s log length before the call and reads only what comes after, because the server's event times come from the monotonic clock and can't be chosen. Every expected position is worked out by the supervisor from the screenshot's metadata, independently of the server's mapping. Headless runs cannot receive physical host pointer events. In visible mode, keep your mouse off the nested window while it runs: the checks read the first pointer position `wev` logs after each call, and your own pointer crossing the window would be logged too, which fails the run; it can't make a check pass. The steps:

1. Accuracy: a screenshot at the output's own scale (`max_width` 4000) and one at a lowered scale (`max_width` 700). For each of C4's five points on `wev`'s surface, the supervisor picks the image pixel that holds it and calls `pointer_move`; `wev` must log the pointer within ±0.05 px of that pixel's centre.
2. `click` left once and right twice: exactly a press and a release of 272, then two of 273, and no input-dirty marker afterwards.
3. `drag` from (50, 50) to (350, 250) on the surface: the press within ±0.05 px of the start, the release of the end, motions while held, and no marker afterwards.
4. `scroll` two notches down, then one left: one wheel frame each, with `axis_value120` 240 and `axis` 30, then −120 and −15.
5. `type_text` of `Hello, wörld →` with `expect` naming `wev`'s `app_id`: `focus: matched`, and `wev` decodes exactly that text, one press and release per character.
6. `type_text` with `expect` naming a window that doesn't exist: `focus_mismatch`, and `wev` logs no key or modifier for one second.
7. Key routing: `key` `ctrl+shift+F12` with `expect` naming `wev`'s window id: `wev` logs exactly one F12 with Control and Shift and modifiers back at 0, and niri's own `Ctrl+Shift+F12` bind, which touches `bind-fired`, doesn't fire for one second.
8. Stop mid-drag: once `wev` logs the drag's press, the supervisor runs `niri-computer-use stop`. The call must end `uncertain`, with the stop in its `detail`, since the press went out, `wev` must log the release, and the marker must be gone; then `resume`, and `acquire_desktop` must succeed again. A stop that lands after the drag finished proves nothing, so the check tries up to three times.
9. Stop mid-typing: a 100-character `type_text`, and a stop once `wev` logs its first key. The call must end `stopped`, and `wev` must still decode the whole text, because a `wtype` already typing finishes; then the marker must be gone, and the lease is taken again after `resume`. Like the drag, it tries up to three times for a stop that lands while wtype types.
10. The two-server crash, after the main server gives the lease up: server A takes the lease and starts a 100-character `type_text`; once `wev` logs its first key, the supervisor kills A's process group. `wtype` runs in a group of its own, so it types the whole text. A new server B must get `recovery_required` from `acquire_desktop`; `recover` with `yes` must say the child has already exited and clear the marker, and B must then take the lease.
11. The same with a drag: once `wev` logs the press, A is killed. niri releases nothing when a pointer goes, so A's crash guardian must release the button, `wev` must log it and the marker must gain `released`, all without `recover`; the log has the times from the kill. B must still get `recovery_required`, and `recover` must send the release again before B takes the lease. Then a third drag's server is killed together with its crash guardian, the guardian first, as a signal to a whole service would end both: the next server's guardian must release the button as it starts, `wev` must log the release and the marker must gain `released`, all without `recover`, and that server must still get `recovery_required` until `recover`.
12. Scrolling a real client: after `wev` is gone, the supervisor starts `kitty --config NONE --hold` running `seq 1 500`, with `wheel_scroll_multiplier=5` and its remote control socket in `TEST_DIR`. The nested niri config floats kitty at 560x360 in the top-left corner, because niri reports no position for tiled windows. `scroll` over kitty's middle, two notches up, must move the first line on kitty's screen (read with `kitty @ get-text`) up by exactly ten lines, and one notch down must move it back by five.

13. With Python 3, PyGObject and GTK 4 available, starts a real GTK button (the GTK fixture's `button` mode) in the nested session, clicks it 100 times and requires its `clicked` callback's counter to advance exactly once after each call. This asserts application activation, not only Wayland delivery. Logs nearest-rank p50/p95/p99 for tool start to observed counter (including harness observation delay). Missing GTK is an explicit skip; Qt, browser, drag/drop and hover-menu activation are not covered by this fixture.
14. M7's native checks run through a second server started with `NIRI_COMPUTER_USE_KEYBOARD=native`. For the native C10, `harness keymaps` connects to the nested niri as an unfocused client and saves every keymap its `wl_keyboard` receives as `keymaps-<name>/keymap-<n>.xkb`. Five `type_text` calls of the C10 corpus must each decode exactly in `wev` and send exactly two maps: one that differs from the compositor's, then the compositor's byte for byte. A later `Hello` must send no map for 300 ms. A server killed while typing `é` leaves the extended map in clients; before `recover` runs, its crash guardian must send the compositor's back byte for byte. Killed while typing `A`, and during a drag holding Ctrl+Shift, the guardian must release the key, button and modifiers and note `released` in the marker without `recover`, and the marker must still block until `recover` sends the releases again; the log has the times from the kill. A drag holding Ctrl+Shift whose serving process is stopped right after the press, while niri's config gains a second layout and niri switches to it, must end `sent`, with the marker gone, clients holding niri's new keymap and `wev` left in the second layout; `wev` must have printed the new keymap before the release, or the change didn't land mid-gesture. For comparison only, the log records whether wtype's C10 call leaves the compositor's map in place. Native typing sends 1000 keys in tens of milliseconds, faster than the 50 ms poll can catch, so before the native focus takeover, stop, cancel and SIGKILL checks the supervisor opens `harness window org.ncu.Pacer --animate`, floating in the bottom-right corner, and gives focus back to `wev`. While niri renders every frame, a native key's round trip waits for a nested frame (about 15 ms), so those checks act while a call is still typing. The pacer closes before the wtype comparisons.
15. With Python 3, PyGObject and GTK 4 available, the same GTK fixture in its `entry` mode shows one focused text entry and reports its text after every change, and each activation with the text it submitted, to files in `TEST_DIR`. For wtype through the main server and then native through a new server, three rounds each type ASCII, then the C10 corpus, and then a line with `submit: true`, all with `expect` naming the entry's `app_id`. The entry must report each text exactly, `ctrl+a` then `BackSpace` must empty it, and the submitted line must arrive once and whole before the entry clears. The log has tool-start-to-report times per backend; the supervisor polls every 50 ms, so they are quantized. Missing GTK is an explicit skip.
16. A paste, `type_text` with `via: "paste"`, into the same entry, after the wtype rounds and again after the native ones. First, before anything was ever copied in the session, `clipboard_read` must say `copied_before_lease`, and a paste must reach the entry, report `paste: {"read": true, "clipboard": "cleared"}`, and leave `wl-paste --list-types` saying nothing is copied for a second; once something was copied, Noctalia's clipboard service adopts the last selection whenever the clipboard goes empty. Then `harness clipboard` takes the selection through ext-data-control with three types of different contents: text, HTML and binary bytes, and `clipboard_read` must return the text, not the HTML. A paste whose `expect` names another app must end with `focus_mismatch`, and one under the stop flag with `stopped`; after both, the owner must still hold the selection, with the same types and bytes per `wl-paste`, and the entry must be empty. Then 4000 characters pasted with `ctrl+v` must appear in the entry exactly, with `{"read": true, "clipboard": "restored"}`, the owner must lose the selection, and `wl-paste` must list the same three types with the same bytes. The log has tool-start-to-entry times. After the wtype paste, `harness clipboard --secret` offers the same three types plus `x-kde-passwordManagerHint` set to `secret`, as a password manager marks its copies: a paste must fail with `clipboard_unsaved`, the owner must keep the selection with all four types and bytes, never cancelled, and the entry must stay empty; `clipboard_read` must return `reason: "secret"`, with the owner's text in neither its result nor the audit log. Last, a server killed while its paste key waits in a `wtype` wrapper must leave the agent's text on the clipboard, marked as a paste's with `application/x-niri-computer-use-paste`; after `recover` and a new `acquire_desktop`, `clipboard_read` must withhold it as `copied_before_lease` the same way, and the next paste must replace it, reach the entry, report `{"read": true, "clipboard": "cleared"}` with a `detail` saying the earlier paste's text was dropped, and leave no paste's type on the clipboard.
17. The paste keeper against other clipboard clients, started directly as `niri-computer-use paste-keeper` with its stdin held by the supervisor. In each of eight rounds the supervisor pauses the keeper with `SIGSTOP`, lets `harness clipboard` take the selection, waiting on `wl-paste --list-types` alone because a read would wait for the paused keeper, closes the keeper's stdin and resumes it. The replacement and the end of stdin are then both waiting, and the keeper must report `replaced` and leave the owner's three types and bytes in place, with the owner never cancelled. Then, once the keeper holds 256 KiB of text, `harness slow-reader` asks for it through ext-data-control, writes a note once niri has passed the request on, and reads it a second later. Meanwhile `harness clipboard` takes the selection, which ends the keeper's part: it must report `replaced`, and the slow reader must still get all 256 KiB, because the keeper ends only once each accepted transfer is done or reaches its two-second deadline. Last, with `harness clipboard`'s copy on the clipboard, a keeper takes it and gets no command for 13 seconds: it must report `restored` once its ten-second wait ends, then answer a `k` with no `armed` within a second, and the clipboard must still offer the owner's copy. Then `harness clipboard --hold` copies and answers no read of its binary type until another client copies; Noctalia's clipboard service reads only the text, so the keeper is the one held. Once a read is held, a second `harness clipboard` copies. The keeper must report `refused` saying something was copied while it saved, and the newer copy must stay on the clipboard, never cancelled.
18. With Python 3, PyGObject and GTK 4 available, the GTK fixture's `copy` mode shows a `Copy link` button, floating at the top left as the `button` mode's does, that puts a synthetic link on the clipboard and counts its copies in `TEST_DIR/copies`. Without the lease, `clipboard_read` must refuse with `lease_required`. `harness clipboard` copies before `acquire_desktop`; after it, `clipboard_read` must withhold that copy as `copied_before_lease`, with the owner's text in neither its result nor the audit log. A `click` on the button must take the selection from the owner, and `clipboard_read` must then return the link. After `release_desktop` it must refuse with `lease_required` again.

19. Native input in other toolkits, through a server started with `NIRI_COMPUTER_USE_KEYBOARD=native`: a Qt Quick text field (`harness/fixtures/qt-input.qml`, run with `qml6` as the accessibility fixture is, reporting its text in `TEST_DIR/qt-input-text`), and kitty (`--class org.ncu.KittyInput`) running `cat` on a raw terminal into `TEST_DIR/kitty-input`. In each, `type_text` of an ASCII line and of the C10 corpus must arrive exactly, and so must 4000 characters pasted with `ctrl+v` in Qt and `ctrl+shift+v` in kitty, with `harness clipboard`'s three types and bytes on the clipboard afterwards. kitty must report `{"read": true, "clipboard": "restored"}`. Qt must report `{"read": false, "clipboard": "restored"}`: it reads the clipboard's text whenever the clipboard changes and keeps what it read, so it reads nothing after the key.
20. Held modifiers, in a headless run only: the supervisor creates a virtual keyboard on cage, so the nested niri gets its keys from its winit backend as it would from a host keyboard, and handles them as physical. In the Qt field, for wtype through the main server and then native, it holds Shift on that keyboard, types `abc` with `type_text`, presses `a` on that keyboard and releases Shift; then the same with Caps Lock turned on and off again. `abc` must arrive exactly every time. The `a` must arrive as `A` after wtype, whose map makes niri send its own with the held modifiers along with the next physical key, and as `a` after native, which leaves niri's map active, so niri sends none and the field keeps the call's zero modifiers. A visible run's parent is your compositor, which the checks never send input to, so it logs the step as skipped.

The run has a 240-second deadline. Its files are in `target/e2e/<run>/`, including `wev.log`, each server's replies, and Noctalia's, kitty's and the optional GTK fixture's logs. Besides the tools of `make nested`, it needs `kitty`, `wl-clipboard` for the paste checks, `qml6` with Qt Quick Controls, and `xkbcli` for the held modifiers.

## Nested shell checks

`make nested-shell` runs M5's acceptance of the Noctalia integration in a nested niri, with Noctalia started as for `make nested-control`. One server holds the lease, and its replies are read back from `server-harness-m5.log`. The nested niri runs without `--session`, so logind can't answer for it. The run checks:

1. `status` shows Noctalia running and the lock state `unlocked` with `source: noctalia` and logind's reason for not answering.
2. For `control-center`, `wallpaper` and `tray-drawer` in turn, the open and close cycle of C13 above, each panel drawn and then gone. With no tray items in the nested session the tray drawer is one icon wide, under the 1% a drawn panel must change, so for it only `activePanelId` is checked.
3. `shell_panel`, opening and closing, on `session`, `launcher`, `polkit`, `clipboard`, `setup-wizard`, `test` and a panel Noctalia doesn't have all give `panel_not_allowed`, and `desktop_state`'s `shell` still shows no open panel.
4. With Noctalia stopped, `status` reports it `not_running` and the lock state `unknown`, `desktop_state`'s `shell` is `noctalia_unavailable`, and `shell_panel` gives `screen_locked`, because no source can say the screen is unlocked.
5. A second server started with `PATH` set to an empty directory still finds Noctalia in the desktop's directories: it lists `shell_panel` and reports Noctalia as `not_running`. A third, started with `NIRI_COMPUTER_USE_DESKTOP_PATH` set to an empty directory, lists no `shell_panel` and reports Noctalia as `not_installed`.

The run has a 130-second deadline. Its files are in `target/e2e/<run>/`, including a `success-<panel>.png` for each panel.

## Nested accessibility checks

`make nested-a11y` runs M9's and M9b's checks in a nested niri with a private accessibility bus. `SCALE` sets the nested output's scale as for `make nested`, and `SSD=1` adds `prefer-no-csd` to the nested niri's config, so apps that honour it leave their decorations to niri (which draws none, borders being off). The supervisor:

1. Starts `/usr/lib/at-spi-bus-launcher --launch-immediately` (without `--a11y=1`, which the launcher rejects with glib 2.90) with `ATSPI_DBUS_IMPLEMENTATION=dbus-daemon`: its first choice under systemd, `dbus-broker-launch`, writes `Ready` to your journal, while dbus-daemon without `<syslog/>` logs to stderr only. It claims `org.a11y.Bus` on the nested session bus and starts a bus of its own at `$XDG_RUNTIME_DIR/at-spi/bus`. The supervisor waits until the name has an owner, reads the address with `GetAddress`, and requires its socket to resolve under `TEST_DIR/run`.
2. Starts `/usr/lib/at-spi2-registryd` itself. The nested session bus has no service directories, and the accessibility bus would activate the registry through systemd, which it can't reach, so nothing is activated from the host.
3. Checks that every application the registry lists (`GetChildren` on its root) is a process of the nested session: the bus daemon's `GetConnectionUnixProcessID` for each one, followed through `/proc/<pid>/stat` to the nested niri. It checks before any fixture starts, when the list is empty, and again with each fixture registered.
4. Starts the fixtures one at a time, each a 400x300 window the nested config floats at (20, 20), and waits for its window in niri and for its process on the bus:
   - GTK 4: `harness/fixtures/gtk.py` in `a11y` mode (`org.ncu.A11y`): a label, the `Primary` and `Second` buttons, which count their activations in `TEST_DIR/primary-count` and `second-count`, the `Plain entry`, which reports its text in `TEST_DIR/a11y-entry-text`, the `Password entry` (`GtkPasswordEntry`) and `Pin entry` (an entry for a PIN that hides it), which count their changes in `password-changes` and `pin-changes` without their text, the `Hidden entry` (a `Gtk.Entry` with `visibility` off and a free-form purpose, holding synthetic text), an `Open dialog` button that opens a 240x120 `Dialog fixture` window over it, a `Vanish` button that removes itself, a `Dismiss` button that closes the window, and a `Rename` button that changes the window's Wayland `app_id` to `org.ncu.Denied`.
   - GTK 3: `harness/fixtures/gtk3.py` (`org.ncu.Gtk3`): a label, a button counting into `TEST_DIR/gtk3-count`, a `Plain entry` reporting its text in `gtk3-entry-text`, an entry that hides its text and counts its changes in `gtk3-password-changes`, and a `Dismiss` button that closes the window.
   - Qt Quick: `qml6 harness/fixtures/qt.qml` (`org.qt-project.qml6` since Qt 6.12, `org.qt-project.qml` before): a label, a button counting into `TEST_DIR/qt-count`, a `Plain entry` text field reporting its text in `qt-entry-text`, a password field counting its changes in `qt-password-changes`, the `Hidden entry`, a text field in `NoEcho` mode holding synthetic text and counting its changes in `qt-hidden-changes`, and a `Dismiss` button that closes the window. It runs with `QT_LINUX_ACCESSIBILITY_ALWAYS_ON=1`, which Qt needs to register on the bus, `QT_FORCE_STDERR_LOGGING=1`, which keeps its logs out of your journal (Qt otherwise writes to journald's socket, which the nested environment doesn't replace), and `QML_XHR_ALLOW_FILE_WRITE=1`, so the QML can write its counter.

5. Writes a policy file that denies `org.ncu.Denied`, starts the nested Noctalia as the lock source and one `niri-computer-use serve`, which must report `accessibility.available` and list `elements`, and takes the lease. Then, through `elements` and `click` with `element`, using the unmodified pointer path:
   - GTK 4: waits a second for niri's view to settle, then clicks `Primary` 100 times, each counted exactly once; moves the window by (+37, +11) with niri's `MoveFloatingWindow`, where a click on the element listed before the move must fail right away with a detail starting `not_showing: the view may still be moving`, and a second later must hit; moves the window to the scrolling layout with `MoveWindowToTiling`, where `elements` must list `Primary` with no box and `unmappable: tiled` and a click on the ref listed before must fail with `element_unmappable` and a detail starting `tiled:`, with the counter unchanged for half a second, then moves it back with `MoveWindowToFloating`, lists it again and, a second later, clicks it, which must hit; opens niri's overview, where, after a second, `Primary` must be `not_showing` and a click on its ref must fail with `element_unmappable` and a detail starting `not_showing: the element or its window isn't showing`, with the counter unchanged for half a second, then closes it, waits a second, lists it again and clicks it, which must hit; focuses the empty workspace below with `FocusWorkspaceDown`, where niri still reports the window's place, and after a second requires `Primary` to be `not_showing` and a click on the ref listed before the moves to fail the same way, with the counter unchanged for half a second, then goes back up, where a click on that ref right away must fail with a detail starting `not_showing: the view may still be moving`, and a second later clicks it, which must hit; starts the GTK 4 `button` fixture as a decoy, makes it a fullscreen tiled window and focuses it, and requires `Primary` to be `not_showing` and a click on its ref to fail with a detail starting `not_showing: a tiled window as large as the output`, with the counter unchanged for half a second, then stops the decoy and clicks `Primary`, which must hit; clicks `Vanish` with `element_changes`, whose report must list it removed, waits until `elements` no longer lists it, and requires `element_stale` for its ref.
   - With the GTK 4 window still open, starts Qt, stops it with `kill -STOP`, and requires `elements` on the GTK window to answer within 3 s and on the Qt window to fail with `deadline_exceeded` within 3 s, then sends `kill -CONT`.
   - GTK 3 and Qt: with `SSD=1`, 100 counted clicks each. Without it, their accessible frame is larger than niri's window, so `elements` must report `unmappable: frame_size_mismatch`, a click must fail with `element_unmappable`, and the counter must stay at 0 for half a second.
   - `close_window` on the GTK 4 window, then a click on its `Primary` ref must fail with `element_stale` in under 50 ms by the audit log's `duration_ms`. The audit log is copied to the artifacts.
   - Then, for each toolkit in turn, with its window focused, through `activate_element` and `set_element_text`: it logs each element's role, states and actions (never names); for GTK 4 and Qt, it walks the fixture's tree with `busctl` on the nested bus and logs the `Hidden entry`'s, the `Plain entry`'s and the password field's role, states, interfaces, attributes, default text attributes, text and actions; GTK 4's hidden entry must have the plain entry's role, states, interfaces, attributes, text attributes and actions, and Qt's the role `password_text`, as the `secret_field` docs say; activates the counted button 20 times, listing it again before each since each activation relabels it, each counted exactly once and observed `present`, the first with `element_changes`, whose report must list a button with its old name as `was`, then requires `element_stale` for the ref the last activation relabelled, with the counter unchanged; sets the plain entry's text to a synthetic string with `element_changes`, which must be `matched` and read back whole by the fixture, and logs the report; for GTK 4, activates `Open dialog` with `element_changes`, whose report must have the dialog opened and focus moved to it, and closes the dialog, whose report must have it closed; requires `secret_field` for each password field, Qt's hidden entry among them, whose change counter must stay at 0; focuses a GTK 4 `button` fixture as a decoy and requires `focus_mismatch`; sets the stop flag and requires `stopped`, then resumes and takes the lease again; releases the lease and requires `lease_required`; and activates `Dismiss` with `element_changes`, which must be observed `gone`, with the window `closed` and its elements `unknown: window_gone` in the report, after which the window's other refs must fail with `element_stale`. Each refusal must leave the counter unchanged for half a second. GTK 4 also takes 64 KiB, which the entry keeps 65534 bytes of, so `differs`, and refuses a byte more with `text_too_long`. With or without `SSD=1`, since nothing is aimed.
   - A new GTK 4 window activates `Rename`, waits until niri reports the denied `app_id`, and requires `app_denied` for `Primary`, with its counter at 0.
   - Two list fixtures, one after the other: the GTK fixture's `list` window, two `Gtk.ListView`s of 200 synthetic records side by side, one with unnamed rows (`Record`) and one whose rows are labelled with their record through `GtkListItem:accessible-label` (`Labelled`), and `qml6 harness/fixtures/qt-list.qml`, three Qt Quick `ListView`s with `reuseItems`, of 200 records each, whose rows are plain layouts (`Record`), unnamed list items (`Item`) and list items named after their record (`Named`). Every row has its record's label and a `Delete` button that removes the record the row shows and appends it to `list-deletes` or `qt-list-deletes`; a press on a row that shows no record appends `a row with no record`. In each list, it lists the rows and deletes the third record through its ref; then that ref again, the fifth record's after the rows below moved up, the first's after `Step` scrolls 120 pixels and the seventh's after `Jump` goes a hundred rows down; then, after another `Step`, the first three showing rows' `Delete` through fresh refs, each again through the same ref at once and once more after a `Step`. An accepted press must delete its own record and a refused one nothing. The listing must mark GTK 4's `Record` and Qt's `Item` rows' `Delete` `unbound: unnamed_row` and Qt's `Record` rows' `unbound: repeated`; a press through a marked ref must fail with `element_unbound` and the detail telling the agent to aim the pointer tools at its pixels, and a fresh ref the listing didn't mark must be accepted. In GTK 4 every old ref must be refused. It logs the listing and, with `busctl`, every object's path, role, name and parent before and after each change, how many paths were kept and which kept path now has another name or a parent with another name.
   - The audit log, copied to the artifacts as `audit-m9b.jsonl`, must have every element action that went through with its role and its action or text length, an action only as its kind (`click`, `press`, `activate`, `toggle` or `other`), and no element action entry may hold the text set, an element's name or the dialog's title. At least one entry must have the last gate's `gate` times, which the log sums up: how many calls ran a second pass or were `element_busy`, and the spread of the passes' times and of the answers' age when sent.

   It checks the registry once more after the server's checks.

Every call on a bus from the harness goes through `busctl --address=… --json=short` with the step deadline. The run has a 240-second deadline. It needs at-spi2-core (`at-spi-bus-launcher`, `at-spi2-registryd`), `busctl` from systemd, Python 3 with PyGObject, GTK 4 and GTK 3, and `qml6` with Qt Quick Controls (qt6-declarative).

## Nested shared-engine checks

`SHARED=1` (or `harness run --shared`) runs any of the server checks in shared mode: the nested session's environment leaves `NIRI_COMPUTER_USE_SHARED` unset, so every server the supervisor starts bridges its client to the nested niri's one engine. Without it the variable is `0` and every server serves its client itself. The kill checks of `make nested-input` then kill the engine that `status.engine.pid` names, since its guardian is the one that releases input, and the clients connected to it must report `engine_lost` once before their next call reaches a new engine. After the nested session, the run fails if any server's log says it served its client standalone, or if the engine's socket is still there five seconds later.

```sh
make nested-actions SHARED=1
make nested-input SHARED=1
make nested-control SHARED=1
make nested-a11y SHARED=1
```

`make nested-engine` runs the shared engine's own checks, always in shared mode, with Noctalia as the lock source and `wev` to see the input. It has a 150-second deadline. The supervisor starts ten clients, then checks:

- E1: every client's `status` names the same engine, which serves 10 sessions with its event stream connected, and one process runs `guard <engine>`.
- E3: client A's bridge is killed during a drag. `wev` sees the button released, client B's `status` and `desktop_state` are answered meanwhile, B can take the lease within a second of the kill, and the input-dirty marker is gone.
- E4: `niri-computer-use stop` while A types 100 characters and eight other clients each have `status`, `desktop_state` and a screenshot in flight. A's call returns `stopped`, the lease is free within a second, every other call is answered, `wtype` still types the whole text, and after `resume` A takes the lease again.
- E5: the engine is killed with `SIGKILL` during A's drag. A's call returns `engine_lost`, the guardian releases the button, every other client gets `engine_lost` once and then reaches one new engine, and B is refused with `recovery_required` until `recover`.
- E6: once the ten clients are gone and their engine has exited, client X starts in a scope of its own, `systemd-run --user --scope`, and starts the engine; client Y joins it. The engine and its guardian must run in one scope, described `niri-computer-use shared engine`, that is neither X's nor the harness's. After `systemctl --user stop` on X's scope, Y must still be served by the same engine, and once Y leaves, the engine exits and its scope is collected. The nested session can't reach the user's systemd manager otherwise, so for E6 only `TEST_DIR/run/systemd/private` links to `/run/user/<uid>/systemd/private`. Without that socket, E6 is skipped and says so.

`make nested-measure` measures what the servers cost, with a release build and Noctalia running. It has a 300-second deadline and writes `measure.json` to the run's artifacts. `SERVER=<path>` measures another build, such as one of an older commit, with the current harness. Both modes:

```sh
make nested-measure
make nested-measure SHARED=1
```

For 1, 3 and 10 clients that each call `status`, `desktop_state` and `screenshot` and then idle, it takes every process's PSS, RSS and open files (the median of three samples a second apart) and its CPU time over ten idle seconds: servers or bridges, the engine and the guardians. It also takes the median and 95th percentile round trip of 50 `status` and 50 `screenshot` calls with 1 and 10 clients connected, the first client's start up to its `initialize` reply, and a `status` round trip while another client's screenshot runs. Round trips are timed by reading the client's log every millisecond. In shared mode it also runs 200 clients one after another, with and without one client connected throughout, sampling the engine's PSS and open files, and checks that no engine, guardian or socket is left afterwards. In shared mode the engine exits between measurements, so each one starts cold, as a standalone server does.

`make nested-latency` times each action tool from request to response, with a release build, Noctalia and the GTK 4 accessibility fixture. It has a 240-second deadline and writes `latency.json` to the run's artifacts. `SERVER=<path>` measures another build, as for `nested-measure`:

```sh
make nested-latency
make nested-latency SERVER=/path/to/older/niri-computer-use
```

One server holds the lease, and 30 rounds each call `focus` on the empty workspace below the fixture's and back on the fixture, `click` on its entry by element, `type_text` of two characters and `key` with two `BackSpace`s into it, `activate_element` on its `Primary` button, `niri_action` setting its height, and `shell_panel` opening and closing the control center, none with a screenshot. Before the click it waits 1.2 seconds, outside the timing, because the server refuses an element aim for a second after a workspace switch or a move or resize of its window. For each call it gives the median and 95th percentile round trip, how often each `observed` came back, and how often each set of `changes` keys came back, with the fields of its `windows` entries, such as `focus+workspace`, `windows[size]` or `none`. The results are in `docs/results/action-latency.md`.

## Skill evals

`make nested-eval` runs an agent against one scenario in a nested niri and grades what it did. It needs `claude` on `PATH` and logged in, and it spends tokens:

```sh
make nested-eval SCENARIO=compose-message SKILL=skills/niri-computer-use MODEL=sonnet
```

`SKILL=none` runs without a skill. The scenarios are `compose-message`, `key-screenshots`, `click-elsewhere`, `stopped`, `no-preset`, `dialog-midway` (a window takes focus as soon as the agent takes the lease) and `errand` (launch, send, close, and back to the user's window). The harness starts Noctalia, sets up the scenario's windows (`wev`, a `notes` preset window or a `home` window), copies the skill into `TEST_DIR/agent/.claude/skills/`, and runs `claude -p` there with only the nested server and the `Skill` and `Read` tools. It then grades the nested server's audit log, the fixtures' logs and the transcript: for example, no screenshot that overlaps an action, no action sent in the same turn as another desktop call, Enter pressed once and only after all the text went out, focus back on the user's window, and no more calls after a `stopped` refusal.

The run's files are in `target/e2e/<run>/`: `transcript.jsonl` (the agent's stream), `audit.jsonl`, `answer.md` (its final answer), and `grading.json` and `timing.json` (with the number of calls and agent turns) in the skill-creator's format. The run has a 15-minute deadline. Claude Code also keeps its usual transcript under `~/.claude/projects/`.

## Demo recordings

The site's recordings are real: a Claude Code session drives a real niri desktop through the installed server, and the screen is recorded meanwhile. Nothing in them is scripted or staged after the prompt.

Set the scene on an empty workspace with the apps the prompt needs, and nothing private on that output: notifications off (Noctalia's Do Not Disturb), no other workspace shown. Start the agent in a terminal there, with Claude Code's status line off so it shows no account details, and the prompt as its first message:

```sh
scripts/demo-record.sh target/demo/errand &
kitty -o font_size=14 -o confirm_os_window_close=0 --directory ~/projects/ncu-demo \
    claude --model claude-opus-5-5 --effort low --dangerously-skip-permissions \
    --settings '{"statusLine":{"type":"command","command":"true"}}' \
    "In Files, put the two screenshots into a new folder called release-0.1, then bring me back here."
touch target/demo/errand/STOP   # once the agent has answered
```

`scripts/demo-record.sh` captures the focused output, cursor included, about 30 times a second with `grim`, into `<run>/frames/`, each JPEG named by its capture time in nanoseconds, until `<run>/STOP` exists. It only reads the screen. Anything you do yourself during the take, such as the stop command, goes in `<run>/user.jsonl` as `{"user": [program, [args]], "start_ns": …, "end_ns": …, "narration": …}`.

`scripts/demo-timeline.py` takes the calls from the agent's transcript under `~/.claude/projects/`, which has each call's arguments and full result, times them by the server's audit log, and cuts the time the agent spent thinking: wherever no call is in flight for more than `--gap` seconds (1.2 by default), the stretch is cut down to that, and the end is held for `--tail` (2.5). Calls play in real time. `--exclude-session` leaves out the audit entries of the session that set the scene up. It writes `<run>/cut/`, which `scripts/demo-encode.py` turns into the site's files. The pages that show the videos say that thinking time was shortened.

```sh
python3 -I scripts/demo-timeline.py target/demo/errand ~/.claude/projects/<demo dir>/<session>.jsonl --exclude-session claude-code/<pid>
python3 -I scripts/demo-encode.py target/demo/errand/cut --width 1920 --name errand --out site/public/demo --gif 0 18.7 --still 16.5 --og 16.5
```

`scripts/demo-encode.py` needs ffmpeg with libx264 and libsvtav1, `rsvg-convert` (librsvg) and `avifenc` (libavif). It scales the desktop to `--width` and draws a caption band under it: the prompt, each call as it went out, a summary of its answer when it came back with its round trip, and the lease's state. For `desktop_state` it names only the focused workspace's windows. After a user command, which prints nothing, the band shows the record's narration marked `#`. It writes an MP4 and an AV1 WebM, `<name>.json` with the calls for the page's text version, optional stills as PNG and AVIF, a GIF for the README, and `og.png`, the link-preview card. Times are seconds into the cut recording.

`make nested-demo SCENE=<scene>` still plays a scripted scene in a headless nested niri (`errand`, `layout`, `keyboard` and `stop`, against `harness/fixtures/chat.py`, a GTK 4 chat window) and writes the same `frames/` and `timeline.jsonl` to `target/e2e/<run>/`, which `scripts/demo-encode.py` takes at its default width of 1280. It makes the same calls each take, which helps when comparing builds; the site no longer uses it.

## Real-session pointer check

`scripts/real-pointer-check.py` is M4's supervised accuracy run on a real monitor. It sends real pointer motion to your session, so run it only yourself, at the machine, with your hands off the mouse:

```sh
python3 -I scripts/real-pointer-check.py target/debug/niri-computer-use /tmp/ncu-m4-real-1
```

It opens `wev` on the focused workspace, floats it at 400x300 at (200, 200) through niri IPC, and with the server's `pointer_move` moves the pointer to five points on `wev`'s surface, from a screenshot of DP-1 at its own scale and one 1280 pixels wide. It sends no click, key or scroll. Each motion must land within ±0.05 px of the pixel's centre; `run.log` in the directory you name says where each one did. It expects one monitor named DP-1 at transform `Normal`.

## Supervised sitting

`make sitting` opens the nested output at scale 1.5 and runs C6, C7 and C9. It starts a focused wev and a separate unfocused observer. Noctalia does not run in this mode. The automatic path keeps its existing deadlines; the sitting allows 30 minutes overall, 29 minutes for wev and two minutes for each human action or confirmation. All wtype children keep a three-second deadline.

The supervising agent must confirm that you are present before starting. Follow one cue at a time from `supervise.log`:

1. Click the checkerboard in the nested window at bottom-right. Do not press a key yet. Confirm the click and that you see the checkerboard.
2. When cued, click the checkerboard and press and release physical `x` once. Confirm what you did and saw.
3. When cued, click the checkerboard and press and release physical `a` once. Confirm what you did and saw.
4. When cued, click the checkerboard and press and release Shift once. Confirm what you did and saw.
5. When cued, click the checkerboard, hold Ctrl and Shift, press and release F12 once, then release Shift and Ctrl. Confirm the chord, release of all keys and what you saw.

wev shows a checkerboard, not typed text. The supervisor checks raw events and requires the human's confirmation separately. Only after that confirmation, the supervising agent creates `target/e2e/<run>/confirm-N.txt` containing one line beginning `confirmed: ` followed by the human's statement. A missing confirmation times out; a malformed statement fails. Never create these files in advance or infer a human confirmation from logs.

Its artifacts include `wev-unfocused.log`, `confirm-N.txt`, `sitting-ready.png` and `success-c9.png`. C8, the interrupted pointer, needed the `vpointer` probe, and M4 deleted it: its evidence stays in `docs/results/m0.md`, and its automatic half, a button left pressed by a killed pointer and released from a fresh one, runs in `make nested-input`'s crash check. C14 is a separate host read; the sitting never locks the host.

## Host capture

C15 also measures captures of a real output. This reads `niri msg --json outputs` and runs `grim` 20 times on the output you name. The images stay in memory and only their sizes and timings are kept:

```sh
make host-capture OUTPUT=DP-1
```

The report goes to the terminal and to `target/e2e/<unix time>-<pid>/harness.log`.

## Releases

The crate holds only `src/`, `README.md`, `LICENSE` and `CHANGELOG.md` (`include` in `Cargo.toml`). To see what it would hold and check that it builds on its own:

```sh
cargo package --list -p niri-computer-use
cargo publish --dry-run -p niri-computer-use --locked
```

Pushing a `v<version>` tag runs `.github/workflows/release.yml`. Only repository admins can create `v*` tags, and no one can move or delete one (two tag rulesets). The workflow fails unless the tagged commit is on `main`, then runs the whole CI workflow on that commit itself and builds only if it passed. A tag runs the release workflow of the commit it names, so tag a commit that has this one: older commits' release workflows have none of these gates. It fails unless the tag matches the crate's version and `CHANGELOG.md` has a `## <version> ` section. It builds `x86_64-unknown-linux-gnu` with `--locked --release`, packs the binary with the README, license, changelog, `skills/niri-computer-use/` and a `THIRD-PARTY-LICENSES.txt` that cargo-about writes from `.github/release/` as `niri-computer-use-<version>-x86_64-linux.tar.gz` with a `.sha256` beside it, and hands those files to a second job, which creates a draft GitHub release with that section as its notes. That job runs in the `release` environment, so it waits until the repository owner approves it in the run's page. Only that second job can write to the repository, and it runs no build code. Nothing is published until someone publishes the draft.
