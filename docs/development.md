# Development

The repository pins Rust 1.99.0 with rustfmt and Clippy. Cargo builds both the main binary and the `harness` workspace member.

## Checks

- `make lint` runs rustfmt and Clippy. The pre-commit hook runs this target.
- `make check` runs three targets. `make check-static` runs formatting, Clippy, rustdoc, cargo-deny, cargo-machete, typos, and ShellCheck. `make test` runs every test target, with the protocol tests in standalone mode, and `make test-shared` runs the protocol tests in shared mode. CI runs the three as parallel jobs, so it runs the same commands as `make check`. A newer push to the same branch or pull request cancels CI's older run; the run a release calls is never cancelled.
- CI also runs `make test` and `make test-shared` on an aarch64 runner (`ubuntu-24.04-arm`). The static checks don't depend on the architecture.
- `make perf` runs the unit tests with `NCU_PERF=1`, which also checks how long a zero-length event wait takes. `make check` leaves that bound out, because a loaded machine can miss it without a bug.
- `make coverage` invokes cargo-llvm-cov. Install that optional tool before using the target.

Run `make check` before each commit. Enable the tracked hook with:

```sh
git config core.hooksPath .githooks
```

The dependency gate accepts MIT, Apache-2.0, BSD-2-Clause, BSD-3-Clause, ISC, Unicode-3.0, Zlib, and MPL-2.0 licenses. The one exception is GPL-3.0-or-later for `niri-ipc` (see [decisions](decisions.md)). It rejects wildcard requirements, advisories, unmaintained crates, yanked crates, Git dependencies, and registries other than crates.io. Duplicate versions are warnings until an explicit skip list is needed. cargo-deny doesn't check the `harness` crate's dependencies, because the harness isn't published.

## Property tests

Some unit tests are property tests, written with `proptest`, over input from agents and users that plain serde doesn't check: screenshot and element ref ids, pixels and zoom regions, pointer targets, key combinations, `type_text`'s trailing tags, `save_path`, the policy file and `niri_action`'s JSON. They live in the test modules of `refs.rs`, `coords.rs`, `policy.rs`, `input/keyboard.rs` and `input/leaked_tags.rs`. Each runs 256 to 1024 cases, about half a second in all on one thread. `test_support::properties` sets the count and turns off proptest's file of failing inputs, so a failure writes nothing into the tree. It prints the minimised input instead, which becomes a regression test of its own, as `refs::tests::an_id_names_a_ref_only_as_it_was_issued` did.

## Unit tests against a fake niri

The native keyboard, held pointer modifiers, the crash guardian and the paste keeper talk to niri's Wayland display. Their unit tests run against `test_support::wayland::Compositor`, a display and a niri socket served from a thread of the test's own process. It models what niri's Smithay does with virtual keyboards, keymaps and data-control (see [decisions](decisions.md), 2026-10-12), and records what the focused client would have been sent: keys, modifiers, keymaps, buttons and the selection's types. It also serves niri's event stream, with one focused window whose app id is `test_support::wayland::APP_ID`, so a test can type through `type_input`, the native keyboard's entry point. A test can change niri's keyboard configuration, switch its layout, press a physical key, delay niri's acknowledgement of each key, send a test's waiter an event of niri's at a chosen key press, or copy something, including while a client reads the selection.

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

Every reply the fakes give passes a named hold point (`tests/protocol/holds.rs`): `niri:Version` or `niri:Action:FocusWindow` for the fake niri, `noctalia:status` for the fake Noctalia, `loginctl` for the `loginctl` the lock tests start (`session::held_loginctl`), `atspi:GetRole:/button` for a method or property of one of the mock application's objects, and `niri:effect` for the events the fake niri can send once the server read its answer to an action. A fake program passes a point with the shell function `pass <point>` once the test calls `Fixture::serve_programs`. The points passed since the trace started are the trace of the calls in between, in order, and a test can hold any one of them back until it releases it.

The stall explorer (`tests/protocol/stall.rs`) uses both. It calls each acting tool until two calls in a row pass the same points, which are its trace, then, for every point in that trace and every fact the tool's gates depend on, calls the tool again on a desktop of its own, holds the reply at that point (up to three calls, since a lock read made again because the last one had aged is in the trace only on a machine slow enough to need it), changes the fact, waits until the server can see the change, and releases the reply at once or after 300 ms, past the 250 ms bound on the age of what the gate read. The facts are the stop flag being set, the lease file replaced and locked by another server, another input's marker being set, the screen locking (Noctalia and logind both; in the full matrix those worlds run the fake niri behind `niri --session`, so logind's answer is a point too), keyboard focus, the window closing, the window's `app_id` becoming one the policy denies, and an element becoming a password field, being renamed, its parent renamed, moved to another window, its actions reordered, or removed. Then it checks what the fakes received: every call that acts after the change went out within 250 ms of it, and no more went out than with nothing held. A change before the first call that acts refuses the call with the fact's error, or lets it go out within those 250 ms. A change after it leaves the call unrefused: a stop or a replaced lease makes it `uncertain`, with `accepted` null until the reply came and true after it, no screenshot and nothing more asked, and any other change reports no focus from before the change as the focus afterwards. A new await in the server is a new point in the trace, explored without new test code. It makes nine calls: `activate_element`, `set_element_text`, `focus` with a window, `close_window`, `niri_action`, `launch`, `shell_panel` opening a panel and closing one, and `noctalia`. The keyboard tools, the pointer tools and a paste act through Wayland, which only the nested runs have; the native keyboard's checks before each key are unit tests against a fake compositor (`src/test_support/wayland.rs`).

`make check` explores only `activate_element`, `focus` with a window and `shell_panel` opening a panel, one tool of each kind, against every fact it depends on. `make stall` explores all nine, in both server modes. It runs `make stall-standalone` and `make stall-shared`, which CI runs as two more parallel jobs:

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

The pages are Markdown files in `site/src/content/docs/`, except the home page, `site/src/pages/index.astro`, which sits outside Starlight, is set as the server's man page (`site/src/styles/man.css`), and takes its words from `site/src/lib/home.ts`. It opens on the errand drawn in the agent's screenshot pixels (`site/src/components/Stage.astro`), which `site/src/scripts/stage.ts` plays at the calls' real times from `site/src/lib/stage.ts`. `site/src/scripts/less.ts` adds less's keys, search and status line, and copy buttons. Without the scripts every page reads in full. `site/src/pages/404.astro` answers a missing page as man does, with every page of the manual. `site/src/components/Slide.astro` slides between the home page and the docs. Its HISTORY section is read from `CHANGELOG.md` when the site is built, and the clip's length and size from the MP4. The docs pages share its colours (`site/src/styles/tokens.css`) and get a man page's running head and foot from the `PageTitle` and `Footer` overrides in `site/src/components/`, with each page's man name from `site/src/lib/man.ts`. There is one directory per sidebar group (`site/src/lib/groups.mjs`), and pages are ordered by `sidebar.order` in their front matter. The build goes to `site/dist/`. Preview it with:

```sh
npm run preview
```

`npm run build` also runs `npm test` and two checks, and fails on any of them:

- `npm test`, before anything else: Node's own test runner on `src/**/*.test.ts`, with Node stripping the types itself (`--experimental-strip-types`, which Node 22.12 has), which checks how the home page reads the changelog and the clip's length, how pages get their man names, when the replay shows a call as in flight, what the drawn errand shows at each moment of its calls, where a search matches, and that the keys turn off where the browser refuses storage.

- `scripts/check-reference.mjs`, before Astro: the error reference lists exactly the names in `ErrorName` (`src/error.rs`), and the configuration page exactly the keys of `Policy` and `Preset` (`src/policy.rs`). Change the page in the same commit as the enum or struct.
- `scripts/check-links.mjs`, after Astro: every internal link, image and `#fragment` resolves to a built file, and `llms.txt` and `llms-full.txt` cover every page.

Besides the HTML, the build writes a Markdown copy of every page (`<page>.md`), `llms.txt`, `llms-full.txt`, and the skill from `skills/niri-computer-use/` under `skill/`. The home page's recordings and their stills are real sessions on a desktop set up for them, as [Demo recordings](#demo-recordings) describes; other images come from nested runs (`target/e2e/<run>/`).

CI builds the site on every push and pull request. `.github/workflows/pages.yml` deploys it to GitHub Pages on every push to `main` that touches `site/`, `skills/` or `Cargo.toml`, since the home page shows the crate's version.

## Nested harness

The `harness` binary starts a nested niri inside a private headless cage by default. The nested session inherits no host display or bus connection, and the harness checks that the nested sockets, session bus and config are separate from your real session. Both headless and visible runs use C1's read-only host metadata snapshots as a contamination backstop. `VISIBLE=1` (or `harness run --visible`) selects the visible window for human sittings; `make sitting` selects it automatically. Run it from the repository root:

```sh
make nested                       # output scale 1
make nested SCALE=1.5
make nested NOCTALIA=1            # also start Noctalia in the nested session (C13)
make nested NOCTALIA=1 SCALE=1.5
```

You'll need cage 0.3.1 for headless runs, niri 26.04, `dbus-run-session` (from `dbus`), `grim`, `wev` and `stdbuf` (from coreutils), and Noctalia 5.2.1 for `NOCTALIA=1`. `make nested` builds the server first, which `NOCTALIA=1` drives. With `tesseract` on `PATH`, `NOCTALIA=1` also reads the open control center's text (see below); without it, that check is skipped with a notice. Headless runs start cage through the runner with a deadline ten seconds longer than the nested suite. Its private runtime is `TEST_DIR/cage`, and its socket path must fit 108 bytes. Cage uses the headless backend without libinput devices and disables client decorations. Startup waits at most ten seconds, with no visible fallback. Cage's process group is stopped and reaped on completion or failure; `cage.log` stays with the artifacts.

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
2. Writes a niri config (no startup commands, animations, borders or Xwayland; a magenta background; a fixed 400x300 floating `wev`; one `Ctrl+Shift+F12` test bind) and checks it with `niri validate`, run by the niri the session will nest.
3. Builds the environment for the nested niri from scratch. The XDG and Noctalia directories point into `TEST_DIR`, `WAYLAND_DISPLAY` is the absolute path of cage's private Wayland socket (your Wayland socket only in visible mode), and `HOME`, `PATH` and `LANG` are kept, with `--niri`'s directory put first on `PATH`. `DBUS_SYSTEM_BUS_ADDRESS` points to `TEST_DIR/run/no-system-bus`, where nothing exists, so nothing in the nested session can reach your system bus. `NIRI_SOCKET`, `WAYLAND_SOCKET`, `DISPLAY`, `XDG_SESSION_ID` and `DBUS_SESSION_BUS_ADDRESS` are not set. The run stops if any other variable is present or a path points outside `TEST_DIR`.
   With `NOCTALIA=1` it also writes a Noctalia config that turns off the first-run setup wizard and weather, lists no plugin sources, and points the wallpaper directory at an empty `TEST_DIR/data/wallpapers`, so the wallpaper panel never lists your pictures, and checks it with `noctalia config validate`. That command exits 0 even when it warns, for example about an unknown key, so the harness requires its plain "Config is valid" line.
4. Records a host snapshot before starting cage or the nested session: `niri msg --json outputs`, `noctalia msg status`, the entries of `$XDG_RUNTIME_DIR` and `/tmp/.X11-unix`, and the modification time of `~/.config/dconf/user`. It also reads the harness's cgroup from `/proc/self/cgroup` and the user journal's current cursor (`journalctl --user --show-cursor -n 0`).
5. Runs `dbus-run-session --config-file=… -- niri -c … -- harness supervise …`. The private bus listens only in `TEST_DIR/run` and activates no services.
6. The supervisor runs inside the nested niri. It checks that `NIRI_SOCKET`, the Wayland socket and the D-Bus socket resolve, following symlinks, to paths under `TEST_DIR/run`, that the Noctalia socket path is under it too, and that the system bus address is still `TEST_DIR/run/no-system-bus` with nothing there, not even a symlink. Over one connection, it asks niri for its version and outputs and requires `winit` to be the only output. If the endpoints resolve elsewhere or niri reports any other output, the supervisor sends nothing more to that niri, starts nothing, and a 60-second deadline kills the whole process group instead. Otherwise it:
   - checks that `winit` has the configured scale and transform `Flipped180`, and saves a screenshot
   - starts `wev` and waits until niri reports it as a 400x300 floating window
   - captures the output with `grim` and checks that everything that isn't magenta is a box where niri placed `wev` (C3)
   - captures the output 20 times, PNG and JPEG at `-s 1` and `-s 0.5`, and checks each image's size (C15)
   - stops `wev`
   - with `NOCTALIA=1`, starts Noctalia and `target/debug/niri-computer-use`, and drives C13 through the server's tools. It waits up to ten seconds for `status` to show Noctalia running and the screen unlocked, requires `desktop_state`'s `shell` to show no open panel, and takes the lease. `shell_panel` opening `control-center` must then be accepted and observed `opened`, and closing it observed `closed`, each call returning within two seconds, so each change was seen in `activePanelId` within two seconds of sending. Before each call the supervisor checks that Noctalia's socket at `TEST_DIR/run/noctalia-<nested display>.sock` resolves under `TEST_DIR/run`, so a panel command can't reach your Noctalia. Before the open it waits until Noctalia has drawn its background (less than half the output is still magenta) and fewer than 1% of the pixels changed since the previous capture, and keeps that capture. It then waits up to five seconds until the open panel changes at least 1% of the output's pixels compared with it, and fails at once if 90% or more changed, which means the baseline was wrong, saves a screenshot, and after the close waits until fewer than 1% of the pixels differ from the capture taken before the open (C13). With `tesseract` on `PATH`, while the control center is open it also captures the output at twice its scale, runs `tesseract --psm 11` on it, and requires the word `Home`, the title of the control center's first tab in Noctalia's English strings; only whether the word was found is logged. This is a dev-only check, not a tool. Without `NOCTALIA=1` it logs that C13 was skipped. Noctalia starts after the `wev` capture because its bar reserves space at the top of the output, which would move `wev`.
   - tells the nested niri to quit on the connection it identified
7. Stops cage in headless mode, then takes the host snapshot again and reports any difference, even when the nested run failed. C1 reads only outputs, Noctalia status, directory entry names and dconf modification time, never screenshot pixels, clipboard text or window titles. Host metadata changes from unrelated activity also fail C1 conservatively; a difference is not proof that the harness caused it. Next it reads the user journal's entries since that cursor from the harness's cgroup, which every process of the run stays in, asking only for each entry's program name and PID, never its message, and fails the run if there is any. Programs that log to journald connect to `/run/systemd/journal/socket` directly, which the nested environment doesn't replace: Qt does unless `QT_FORCE_STDERR_LOGGING=1` is set, and so does `dbus-broker-launch`. Anything else in the same cgroup that logs meanwhile, such as the terminal the harness runs in, fails the check too. Then it lists every running process whose command line names `TEST_DIR`, fails the run if there is one, and removes `TEST_DIR`. Everything the run started should be gone by then, but a program can move a child into a process group of its own, out of reach of the group kill. The harness reports such processes and doesn't kill them.

Every process the harness starts has a deadline, and a watchdog kills it when the deadline passes, even one like `wev` that runs in the background. Anything that ends after its deadline counts as timed out. `harness run` starts each process in its own process group and kills that group when the process exits, when the deadline passes, or when you press Ctrl+C. The supervisor's steps stay in the nested niri's group, so that kill covers them too. Before each one, the supervisor checks the nested endpoints again. Each wait polls every 50 ms until its deadline, ignores a result that arrives after it, and on timeout saves `failure-<step>.png`.

The runner can start a step with stdin held open. `feed` writes every byte and closes the pipe, within the child’s original deadline; a watchdog kills a child that stops reading. `still_absent` polls for the full interval, including at its end, and saves a failure screenshot if an event appears.

Keyboard checks read the complete key and modifier records, including continuation lines, after an offset taken before each call. They check ordered press/release pairs, matching keycodes and symbols, decoded text, and chord modifiers at the keys and after release. The corpus is `harness/corpus.txt`: 100 Unicode scalar values, 104 UTF-8 bytes, with ASCII, `é`, `ß` and `→`, and no trailing newline.

The pointer checks of M0 (C4's accuracy, a click and C12's scroll order) ran through the `vpointer` probe, which M4 deleted once the server's own pointer replaced it. The keyboard checks of M0 (C5's calls and held stdin, C9's virtual chord and C10's corpus) ran through `wtype`, which the native keyboard replaced. Both run through the server in `make nested-input`: C9's virtual chord as step 7's key routing, and C10's corpus in step 12's native C10.

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

1. Waits until a server's `status` shows Noctalia running and the screen unlocked. That `status` and the `status` subcommand must both show `stop_key: bound`: the nested niri's config, which it was started with through `-c`, binds `Ctrl+Shift+F11` to `spawn` the server under test with `stop`, and nothing presses it.
2. Starts server A, which takes the lease and keeps its stdin open for 25 seconds, and waits for its record in `lease.json`.
3. Starts server B, whose `acquire_desktop` must fail with `lease_held` naming server A.
4. Runs `recover`, which must refuse because a server holds the lease.
5. Asks niri to `spawn` `niri-computer-use stop`, as the stop keybind does, and waits for the flag and for server A to give the lease up. Server B must then get `stopped`; after `resume` it must take the lease.
6. Starts a child that runs until it is killed, writes an input-dirty marker naming it as a 0.1.0 marker names its `wtype` child, and pipes `yes` into `recover`, which must end the child, send zero modifiers and clear the marker.

Every server talks MCP over a shell pipeline (`printf` of the requests, then `sleep` to keep stdin open), and every flag and marker lives in `TEST_DIR`. The run has a 90-second deadline. Its files are in `target/e2e/<run>/`, with `server-a.log` holding server A's replies.

## Nested action checks

`make nested-actions` runs M3's acceptance in a nested niri, set up as `make nested-control` is, with the nested Noctalia as the lock source. Before the server starts, the supervisor writes a policy file to `TEST_DIR/config/niri-computer-use/policy.toml`, with `notify_on_lease = false` so the nested Noctalia shows no lease toast (M3 checks that `status` reads it), whose presets all start `harness window`, a fixture app in the harness, with a different `app_id` each:

| Preset | Fixture |
|---|---|
| `late` | sets its `app_id` 400 ms after its window is mapped |
| `two` | maps two windows |
| `reuse`, `plain` | one ordinary window |
| `keep` | ignores close requests, as an app asking about unsaved changes does |
| `slow` | writes a marker file at once and maps its window 2.5 seconds later |
| `sized` | draws at the size niri configures (`--resize`) |

`harness window` checks that its environment is the nested one before it connects, maps black 320x240 toplevels through `xdg_wm_base`, with `--resize` redraws at each size niri configures, with `--animate` attaches and damages its buffer again on every frame callback, with `--flicker <rows>` also paints its top rows a new shade of grey each time, and exits when its windows close, when niri goes away, or after 90 seconds. niri starts it from the preset with niri's own environment, so it never reaches the host compositor.

One server holds the lease for the whole run. The supervisor keeps its stdin open and reads its replies back from `server-harness-m3.log`. Through it, the run checks:

1. `launch late` gives `one`, and `desktop_state` shows the window with its late `app_id`.
2. `launch two` gives `ambiguous` with two windows.
3. `launch reuse` with `reuse: true` gives `one`; after `focus` on another window, the same call gives `focused` on that window and starts nothing; after a second plain launch, it gives `ambiguous` with `accepted: false`, and still nothing new starts.
4. `focus` on a window, then on the empty workspace, with no window focused, and back, with focus on the window it left, each `focused`.
5. `close_window` on a `plain` window gives `closed`; on a `keep` window, `pending` with a screenshot of the output in the result.
6. `launch slow`, and as soon as the marker appears the supervisor focuses the `late` window through its own niri connection: the launch gives `interrupted` naming that window, with a screenshot.
7. On a `sized` window, `niri_action` `FullscreenWindow` gives `changed` with `window.window_size` equal to the output's logical size, and again gives the tiled size back; `ToggleWindowFloating` gives `is_floating: true`; `SetWindowWidth` with `SetFixed: 400` gives a window 400 wide. `Spawn` is refused with `unrestricted_required`.
8. After the first server stops, the supervisor rewrites the policy file to `unrestricted = true` and starts a second server, which spawns `harness window … spawned` through `niri_action` `Spawn`; `wait_for` sees the window, and `close_window` closes it. Through the same server, `noctalia` `["status"]` returns the nested Noctalia's unlocked status as its reply, and `["no-such-command"]` gives `upstream_error` with Noctalia's message.
9. The supervisor rewrites the policy file to `launch_desktop_entries = true`, writes the desktop entry `org.example.Entry` to `TEST_DIR/data/applications`, and starts a third server with `XDG_DATA_DIRS` naming an empty directory under `TEST_DIR`, so it reads none of the host's entries. The user owns the entry, so `status.desktop_entries` must have `count` 0 and `user_writable` 1, and `launch org.example.Entry` must fail with `entry_refused` naming the rule that a user-writable entry needs a preset. A nested run can't make a file root owns, so starting an entry is covered by the unit tests, which trust the test user's files.

Each action's `changes` must also tell what it did: the window `launch late` opened and focused, both windows of `launch two`, the focus `launch reuse` moved, the workspace and focus `focus` on a workspace changed each way, the window `close_window` closed, where the interrupted launch's focus went, the window's new size after each `FullscreenWindow`, `floating` after `ToggleWindowFloating` and a width of 400 after `SetWindowWidth`. The `pending` close's and the `Spawn`'s reports are only logged. Each report is logged as `changes: <what>: checked <bool>, <bytes> bytes: <json>`.

The run has a 130-second deadline. Its files are in `target/e2e/<run>/`, including `policy.toml`, the server's replies and Noctalia's log.

## Nested input checks

`make nested-input` runs M4's acceptance of the pointer and keyboard tools in a nested niri, set up as `make nested-control` is, with the nested Noctalia as the lock source. `SCALE` sets the nested output's scale as for `make nested`; the plan's exit runs it at 1 and at 1.5. One server holds the lease for the whole run, and its replies are read back from `server-harness-m4.log`. `wev` starts after Noctalia, whose bar moves floating windows down when it appears, and the supervisor reads `wev`'s surface position from niri's window layout. Each check takes `wev`'s log length before the call and reads only what comes after, because the server's event times come from the monotonic clock and can't be chosen. Every expected position is worked out by the supervisor from the screenshot's metadata, independently of the server's mapping. Each screenshot a pointer check aims through is taken 1.1 seconds after the step before it, since the server refuses a pixel aim through a screenshot taken within a second of niri moving the windows on its output. Headless runs cannot receive physical host pointer events. In visible mode, keep your mouse off the nested window while it runs: the checks read the first pointer position `wev` logs after each call, and your own pointer crossing the window would be logged too, which fails the run; it can't make a check pass. The steps:

1. Accuracy: a screenshot at the output's own scale (`max_width` 4000) and one at a lowered scale (`max_width` 700). For each of C4's five points on `wev`'s surface, the supervisor picks the image pixel that holds it and calls `pointer_move`; `wev` must log the pointer within ±0.05 px of that pixel's centre.
2. `click` left once and right twice: exactly a press and a release of 272, then two of 273, and no input-dirty marker afterwards. Then a stale ref: after a new screenshot, the supervisor moves `wev` 250 px right with niri's `MoveFloatingWindow`, and a `click` at the pixel that showed `wev`'s centre must fail with `ref_invalid` `layout_changed`, with no button in `wev` for one second; `wev` then moves back.
3. `drag` from (50, 50) to (350, 250) on the surface: the press within ±0.05 px of the start, the release of the end, motions while held, and no marker afterwards.
4. `scroll` two notches down, then one left: one wheel frame each, with `axis_value120` 240 and `axis` 30, then −120 and −15.
5. `type_text` of `Hello, wörld →` with `expect` naming `wev`'s `app_id`: `focus: matched`, and `wev` decodes exactly that text, one press and release per character.
6. `type_text` with `expect` naming a window that doesn't exist: `focus_mismatch`, and `wev` logs no key or modifier for one second.
7. Key routing: `key` `ctrl+shift+F12` with `expect` naming `wev`'s window id: `wev` logs exactly one F12 with Control and Shift and modifiers back at 0, and niri's own `Ctrl+Shift+F12` bind, which touches `bind-fired`, doesn't fire for one second.
8. Stop mid-drag: once `wev` logs the drag's press, the supervisor runs `niri-computer-use stop`. The call must end `uncertain`, with the stop in its `detail`, since the press went out, `wev` must log the release, and the marker must be gone; then `resume`, and `acquire_desktop` must succeed again. A stop that lands after the drag finished proves nothing, so the check tries up to three times.
9. The two-server crash, after the main server gives the lease up: server A takes the lease and starts a drag; once `wev` logs the press, the supervisor kills A's process group. niri releases nothing when a pointer goes, so A's crash guardian must release the button, `wev` must log it and the marker must gain `released`, all without `recover`; the log has the times from the kill. A new server B must get `recovery_required` from `acquire_desktop`, and `recover` with `yes` must send the release again and clear the marker before B takes the lease. Then a third drag's server is killed together with its crash guardian, the guardian first, as a signal to a whole service would end both: the next server's guardian must release the button as it starts, `wev` must log the release and the marker must gain `released`, all without `recover`, and that server must still get `recovery_required` until `recover`.
10. Scrolling a real client: after `wev` is gone, the supervisor starts `kitty --config NONE` running `seq 1 500` and then `sleep`, not a shell, whose prompt could move the screen, with `wheel_scroll_multiplier=5` and its remote control socket in `TEST_DIR`. The nested niri config floats kitty at 560x360 in the top-left corner, because niri reports no position for tiled windows. `scroll` over kitty's middle, two notches up, must move the first line on kitty's screen (read with `kitty @ get-text`) up by exactly ten lines, and one notch down must move it back by five.

11. With Python 3 at `/usr/bin/python3` (the harness runs the fixtures with it, not a `python3` on `PATH`, which may be a version manager's shim), PyGObject and GTK 4 available, starts a real GTK button (the GTK fixture's `button` mode) in the nested session, clicks it 100 times and requires its `clicked` callback's counter to advance exactly once after each call. This asserts application activation, not only Wayland delivery. Logs nearest-rank p50/p95/p99 for tool start to observed counter (including harness observation delay). Missing GTK is an explicit skip; Qt, browser, drag/drop and hover-menu activation are not covered by this fixture.
12. M7's checks of the native keyboard run through a second server. For the native C10, `harness keymaps` connects to the nested niri as an unfocused client and saves every keymap its `wl_keyboard` receives as `keymaps-<name>/keymap-<n>.xkb`. Five `type_text` calls of the C10 corpus must each decode exactly in `wev` and send exactly two maps: one that differs from the compositor's, then the compositor's byte for byte. One `type_text` of 24 Greek letters and six arrows, more symbols the nested layout lacks than it has spare keys, must decode exactly in `wev` and send one extended map per part, each another than the one before, then the compositor's byte for byte. A later `Hello` must send no map for 300 ms. In a headless run, with the pacer described below open, a physical F13 (evdev 183) pressed on a virtual keyboard on cage, as in step 18, during a `type_text` of that Greek-and-arrows text, once its first extended map, which borrows `<FK13>`, reached clients and with the serving process stopped until `wev` has the key, must decode as `XF86Tools` in `wev`. niri must have sent the compositor's map before it, and the call must still type the text exactly and end with the compositor's map byte for byte. A server killed while typing `é` leaves the extended map in clients; before `recover` runs, its crash guardian must send the compositor's back byte for byte. Killed while typing `A`, and during a drag holding Ctrl+Shift, the guardian must release the key, button and modifiers and note `released` in the marker without `recover`, and the marker must still block until `recover` sends the releases again; the log has the times from the kill. A drag holding Ctrl+Shift whose serving process is stopped right after the press, while niri's config gains a second layout and niri switches to it, must end `sent`, with the marker gone, clients holding niri's new keymap and `wev` left in the second layout; `wev` must have printed the new keymap before the release, or the change didn't land mid-gesture. With niri's config given a second layout, Russian, and niri switched to it, `key` `ctrl+a`, whose keysym only the first layout has, must end `sent`, `wev` must decode `a` with Control in the first layout and be left in the second, and niri's `KeyboardLayouts` must still report the second as current. Native typing sends 1000 keys in tens of milliseconds, faster than the 50 ms poll can catch, so before the native focus takeover, stop, cancel and SIGKILL checks the supervisor opens `harness window org.ncu.Pacer --animate`, floating in the bottom-right corner, and gives focus back to `wev`. While niri renders every frame, a native key's round trip waits for a nested frame (about 15 ms), so those checks act while a call is still typing. The pacer closes once they are done.
13. With Python 3, PyGObject and GTK 4 available, the same GTK fixture in its `entry` mode shows one focused text entry and reports its text after every change, and each activation with the text it submitted, to files in `TEST_DIR`. Through the main server, three rounds each type ASCII, then the C10 corpus, then 24 Greek letters and six arrows, which take the native keyboard three extended maps, and then a line with `submit: true`, all with `expect` naming the entry's `app_id`. The entry must report each text exactly, `ctrl+a` then `BackSpace` must empty it, and the submitted line must arrive once and whole before the entry clears. After the rounds, with step 12's Russian layout active, `type_text` of `abc` and then `ctrl+a` and `BackSpace` must leave the entry empty, with niri still in the Russian layout. The log has tool-start-to-report times; the supervisor polls every 50 ms, so they are quantized. Missing GTK is an explicit skip.
14. A paste, `type_text` with `via: "paste"`, into the same entry, after the rounds. First, before anything was ever copied in the session, `clipboard_read` must say `copied_before_lease`, and a paste must reach the entry, report `paste: {"read": true, "clipboard": "cleared"}`, and leave `wl-paste --list-types` saying nothing is copied for a second; once something was copied, Noctalia's clipboard service adopts the last selection whenever the clipboard goes empty. Then `harness clipboard` takes the selection through ext-data-control with three types of different contents: text, HTML and binary bytes, and `clipboard_read` must return the text, not the HTML. A paste whose `expect` names another app must end with `focus_mismatch`, and one under the stop flag with `stopped`; after both, the owner must still hold the selection, with the same types and bytes per `wl-paste`, and the entry must be empty. Then 4000 characters pasted with `ctrl+v` must appear in the entry exactly, with `{"read": true, "clipboard": "restored"}`, the owner must lose the selection, and `wl-paste` must list the same three types with the same bytes. The log has tool-start-to-entry times. After it, `harness clipboard --secret` offers the same three types plus `x-kde-passwordManagerHint` set to `secret`, as a password manager marks its copies: a paste must fail with `clipboard_unsaved`, the owner must keep the selection with all four types and bytes, never cancelled, and the entry must stay empty; `clipboard_read` must return `reason: "secret"`, with the owner's text in neither its result nor the audit log. Then a paste whose call goes away once its key went out, by a stop, by the client cancelling the request and by the client's end, while the entry's process is stopped for a second so it reads the clipboard late, must leave the agent's text in the entry and the user's copy offered again. Last, a keeper started directly that admits the key with `armed` and then sees its stdin end, as when its server dies before saying whether the key went out, must keep the agent's text on the clipboard for three seconds, marked as a paste's with `application/x-niri-computer-use-paste`. A native key goes out right after the admission, so a server can't be killed in between on purpose. After a new `acquire_desktop`, `clipboard_read` must withhold the text as `copied_before_lease` the same way, and the next paste must replace it, reach the entry, report `{"read": true, "clipboard": "cleared"}` with a `detail` saying the earlier paste's text was dropped, and leave no paste's type on the clipboard.
15. The paste keeper against other clipboard clients, started directly as `niri-computer-use paste-keeper` with its stdin held by the supervisor. In each of eight rounds the supervisor pauses the keeper with `SIGSTOP`, lets `harness clipboard` take the selection, waiting on `wl-paste --list-types` alone because a read would wait for the paused keeper, closes the keeper's stdin and resumes it. The replacement and the end of stdin are then both waiting, and the keeper must report `replaced` and leave the owner's three types and bytes in place, with the owner never cancelled. Then, once the keeper holds 256 KiB of text, `harness slow-reader` asks for it through ext-data-control, writes a note once niri has passed the request on, and reads it a second later. Meanwhile `harness clipboard` takes the selection, which ends the keeper's part: it must report `replaced`, and the slow reader must still get all 256 KiB, because the keeper ends only once each accepted transfer is done or reaches its two-second deadline. Last, with `harness clipboard`'s copy on the clipboard, a keeper takes it and gets no command for 13 seconds: it must report `restored` once its ten-second wait ends, then answer a `k` with no `armed` within a second, and the clipboard must still offer the owner's copy. Then `harness clipboard --hold` copies and answers no read of its binary type until another client copies; Noctalia's clipboard service reads only the text, so the keeper is the one held. Once a read is held, a second `harness clipboard` copies. The keeper must report `refused` saying something was copied while it saved, and the newer copy must stay on the clipboard, never cancelled.
16. With Python 3, PyGObject and GTK 4 available, the GTK fixture's `copy` mode shows a `Copy link` button, floating at the top left as the `button` mode's does, that puts a synthetic link on the clipboard and counts its copies in `TEST_DIR/copies`. Without the lease, `clipboard_read` must refuse with `lease_required`. `harness clipboard` copies before `acquire_desktop`; after it, `clipboard_read` must withhold that copy as `copied_before_lease`, with the owner's text in neither its result nor the audit log. A `click` on the button must take the selection from the owner, and `clipboard_read` must then return the link. After `release_desktop` it must refuse with `lease_required` again.

17. Input in other toolkits, through the main server: a Qt Quick text field (`harness/fixtures/qt-input.qml`, run with `qml6` as the accessibility fixture is, reporting its text in `TEST_DIR/qt-input-text`), and kitty (`--class org.ncu.KittyInput`) running `cat` on a raw terminal into `TEST_DIR/kitty-input`. In each, `type_text` of an ASCII line, of the C10 corpus and of the 24 Greek letters and six arrows that take three extended maps must arrive exactly, and so must 4000 characters pasted with `ctrl+v` in Qt and `ctrl+shift+v` in kitty, with `harness clipboard`'s three types and bytes on the clipboard afterwards. kitty must report `{"read": true, "clipboard": "restored"}`. Qt must report `{"read": false, "clipboard": "restored"}`: it reads the clipboard's text whenever the clipboard changes and keeps what it read, so it reads nothing after the key.
18. Held modifiers, in a headless run only: the supervisor creates a virtual keyboard on cage, so the nested niri gets its keys from its winit backend as it would from a host keyboard, and handles them as physical. In the Qt field, through the main server, it holds Shift on that keyboard, types `abc` with `type_text`, presses `a` on that keyboard and releases Shift; then the same with Caps Lock turned on and off again, and with Num Lock turned on and off again, pressing keypad `1` and then `a`. `abc` must arrive exactly every time. The call leaves niri's map active, so niri sends none and the field keeps the call's zero modifiers and no locks: the `a` must arrive as `a`, and keypad `1` acts as End, so only `a` arrives. A visible run's parent is your compositor, which the checks never send input to, so it logs the step as skipped.
19. With Python 3, PyGObject and GTK 4 available, a modal dialog that takes focus while `type_text` types, through the main server: the eval's `draft` fixture (`harness/fixtures/workflow.py`) opens a modal `Autosave failed` window as soon as its entry first holds text, and records in `draft-events.jsonl` each key the dialog got and whether niri sent it to the dialog's own surface or to the form's, which GTK passes on to its modal window. Each round starts the fixture, types 990 characters with `expect` naming the Draft window, presses Escape in the dialog and stops the fixture. A round in which niri sent any key to the dialog's surface must end `interrupted`, with a `detail` saying some of the text may have reached it; every round's `changes` must show the dialog opening and focus leaving the Draft window. GTK maps the dialog only once it has handled the keys already sent, so in some rounds niri sends every key to the form and the call is `sent`. Rounds run until niri sent a key to the dialog's surface, twelve at most; none in twelve fails the run.

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
   - GTK 4: waits a second for niri's view to settle, then clicks `Primary` 100 times, each counted exactly once; moves the window by (+37, +11) with niri's `MoveFloatingWindow`, where a click on the element listed before the move must fail right away with a detail starting `not_showing: the view may still be moving`, and a second later must hit; moves the window to the scrolling layout with `MoveWindowToTiling`, where `elements` must list `Primary` with no box and `unmappable: tiled` and a click on the ref listed before must fail with `element_unmappable` and a detail starting `tiled:`, with the counter unchanged for half a second, then moves it back with `MoveWindowToFloating`, lists it again and, a second later, clicks it, which must hit; opens niri's overview, where, after a second, `Primary` must be `not_showing` and a click on its ref must fail with `element_unmappable` and a detail starting `not_showing: the element or its window isn't showing`, with the counter unchanged for half a second, then closes it, waits a second, lists it again and clicks it, which must hit; focuses the empty workspace below with `FocusWorkspaceDown`, where niri still reports the window's place, and after a second requires `Primary` to be `not_showing` and a click on the ref listed before the moves to fail the same way, with the counter unchanged for half a second, then goes back up, where a click on that ref right away must fail with a detail starting `not_showing: the view may still be moving`, and a second later clicks it, which must hit; starts the GTK 4 `button` fixture as a decoy, makes it a fullscreen tiled window and focuses it, and requires `Primary` to be `not_showing` and a click on its ref to fail with a detail starting `not_showing: a tiled window as large as the output`, with the counter unchanged for half a second, then stops the decoy and clicks `Primary`, which must hit; clicks `Vanish` with `element_changes`, whose report must list it removed, waits with `wait_for`'s `absent` until no whole walk lists it, and requires `element_stale` for its ref.
   - With the GTK 4 window still open, starts Qt, stops it with `kill -STOP`, and requires `elements` on the GTK window to answer within 3 s and on the Qt window to fail with `deadline_exceeded` within 3 s, then sends `kill -CONT`.
   - GTK 3 and Qt: with `SSD=1`, 100 counted clicks each. Without it, their accessible frame is larger than niri's window, so `elements` must report `unmappable: frame_size_mismatch`, a click must fail with `element_unmappable`, and the counter must stay at 0 for half a second.
   - `close_window` on the GTK 4 window, then a click on its `Primary` ref must fail with `element_stale` in under 50 ms by the audit log's `duration_ms`. The audit log is copied to the artifacts.
   - Then, for each toolkit in turn, with its window focused, through `activate_element` and `set_element_text`: it logs each element's role, states and actions (never names); for GTK 4 and Qt, it walks the fixture's tree with `busctl` on the nested bus and logs the `Hidden entry`'s, the `Plain entry`'s and the password field's role, states, interfaces, attributes, default text attributes, text and actions; GTK 4's hidden entry must have the plain entry's role, states, interfaces, attributes, text attributes and actions, and Qt's the role `password_text`, as the `secret_field` docs say; activates the counted button 20 times, listing it again before each since each activation relabels it, each counted exactly once and observed `present` with `element.name_changed: true` and `effect: seen`, the first with `element_changes`, whose report must list a button with its old name as `was`, then requires `element_stale` for the ref the last activation relabelled, with the counter unchanged; sets the plain entry's text to a synthetic string with `element_changes`, which must be `matched` with `effect: seen` and read back whole by the fixture, and logs the report; for Qt, activates the `Spin` spin box's `Increase` action, which must be `present` with `element.value_changed` and `effect: seen` while the fixture reports the value going from 2 to 3, and sets the `Upper entry`, which upper-cases its text, to a synthetic string, which must be `differs` with `effect: differs` at the string's own character count, with no upper-cased text in the result, while the fixture reports the upper-cased text; for GTK 4, activates `Open dialog` with `element_changes`, whose report must have the dialog opened and focus moved to it, and closes the dialog, whose report must have it closed; requires `secret_field` for each password field, Qt's hidden entry among them, whose change counter must stay at 0; focuses a GTK 4 `button` fixture as a decoy and requires `focus_mismatch`; sets the stop flag and requires `stopped`, then resumes and takes the lease again; releases the lease and requires `lease_required`; and activates `Dismiss` with `element_changes`, which must be observed `gone`, with the window `closed` and its elements `unknown: window_gone` in the report, after which the window's other refs must fail with `element_stale`. Each refusal must leave the counter unchanged for half a second. GTK 4 also takes 64 KiB, which the entry keeps 65534 bytes of, so `differs` with `effect: differs`, and refuses a byte more with `text_too_long`. With or without `SSD=1`, since nothing is aimed.
   - A new GTK 4 window activates `Rename`, waits until niri reports the denied `app_id`, and requires `app_denied` for `Primary`, with its counter at 0.
   - Two list fixtures, one after the other: the GTK fixture's `list` window, two `Gtk.ListView`s of 200 synthetic records side by side, one with unnamed rows (`Record`) and one whose rows are labelled with their record through `GtkListItem:accessible-label` (`Labelled`), and `qml6 harness/fixtures/qt-list.qml`, three Qt Quick `ListView`s with `reuseItems`, of 200 records each, whose rows are plain layouts (`Record`), unnamed list items (`Item`) and list items named after their record (`Named`). Every row has its record's label and a `Delete` button that removes the record the row shows and appends it to `list-deletes` or `qt-list-deletes`; a press on a row that shows no record appends `a row with no record`. In each list, it lists the rows and deletes the third record through its ref; then that ref again, the fifth record's after the rows below moved up, the first's after `Step` scrolls 120 pixels and the seventh's after `Jump` goes a hundred rows down; then, after another `Step`, the first three showing rows' `Delete` through fresh refs, each again through the same ref at once and once more after a `Step`. An accepted press must delete its own record and a refused one nothing. The listing must mark GTK 4's `Record` and Qt's `Item` rows' `Delete` `unbound: unnamed_row` and Qt's `Record` rows' `unbound: repeated`; a press through a marked ref must fail with `element_unbound` and the detail telling the agent to aim the pointer tools at its pixels, and a fresh ref the listing didn't mark must be accepted. In GTK 4 every old ref must be refused. It logs the listing and, with `busctl`, every object's path, role, name and parent before and after each change, how many paths were kept and which kept path now has another name or a parent with another name.
   - The audit log, copied to the artifacts as `audit-m9b.jsonl`, must have every element action that went through with its role and its action or text length, an action only as its kind (`click`, `press`, `activate`, `toggle` or `other`), and no element action entry may hold the text set, an element's name or the dialog's title. At least one entry must have the last gate's `gate` times, which the log sums up: how many calls ran a second pass or were `element_busy`, and the spread of the passes' times and of the answers' age when sent.

   It checks the registry once more after the server's checks.

Every call on a bus from the harness goes through `busctl --address=… --json=short` with the step deadline. The run has a 240-second deadline. It needs at-spi2-core (`at-spi-bus-launcher`, `at-spi2-registryd`), `busctl` from systemd, Python 3 with PyGObject, GTK 4 and GTK 3, and `qml6` with Qt Quick Controls (qt6-declarative).

## Nested big-container checks

`make nested-big` lists windows with one big container through a release build of the server, as users run it, on the nested accessibility bus `make nested-a11y` starts, with the nested Noctalia as the lock source. Each window is `harness/fixtures/big.py`, a `Save` button and a `Search` entry above the container and an `Apply` button and a `Notes` entry below it:

- GTK 4, a box of 3,000 `Open <n>` buttons (6,026 objects);
- GTK 4, a list box of 1,500 rows, each named after its record and holding its label and a `Delete` button (7,526 objects);
- GTK 3, a box of 3,000 buttons (3,020 objects);
- GTK 3, a tree view of 5,000 rows of three text columns (15,022 objects);
- GTK 3, a tree view of 8,000 rows, past the walk's 20,000 objects;
- GTK 3, a tree view of 22,000 rows, whose table has 66,003 children, more than at-spi2-atk lists.

Each listing of the first four must read the whole window, with `capped` false, and `elements` must list `Save`, `Search`, `Apply` and `Notes` with a ref and no `unbound`; `activate_element` on `Apply`'s ref must be accepted, and the fixture must count the click in `big-apply` within five seconds. The fifth must be cut with `capped_reason: node_cap` and the last with `capped_reason: children_refused`, and both must still list all four, read before the tree's cells, each with `unbound: cut_short`. It logs each listing's time and how many objects it walked. The run has a 240-second deadline and needs what `make nested-a11y` needs, without Qt.

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
- E4: `niri-computer-use stop` while A types 1000 characters, with the pacer window open so the typing outlasts the poll, and eight other clients each have `status`, `desktop_state` and a screenshot in flight. A's call returns `stopped`, the lease is free within a second, every other call is answered, the marker is gone, `wev` decoded some but not all of the text with no modifier left held, and after `resume` A takes the lease again.
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

One server holds the lease, and 30 rounds each call `focus` on the empty workspace below the fixture's and back on the fixture, `click` on its entry by element, `type_text` of two characters and `key` with two `BackSpace`s into it, `activate_element` on its `Primary` button, `niri_action` setting its height, and `shell_panel` opening and closing the control center, none with a screenshot. Before the click it waits 1.2 seconds, outside the timing, because the server refuses an element aim for a second after a workspace switch or a move or resize of its window. For each call it gives the median and 95th percentile round trip, how often each `observed` came back, and how often each set of `changes` keys came back, with the fields of its `windows` entries, such as `focus+workspace`, `windows[size]` or `none`. After the rounds, it times screenshots that wait for the screen to settle: five rounds of `type_text` `"a"` and `key` `BackSpace` into the fixture's entry, each with `screenshot: true` and `expect` naming the fixture, with nothing else moving; then ten more with the M7 pacer, a 320x240 floating window in the bottom-right corner, open with `--flicker 3`, so that 960 pixels, 0.14 % of the 960x720 output, change on every frame, as a clock would; then three with `--flicker 240`, about 11 %, as a video would. `latency.json` has them under `settle`, each with how often `settled` came back true and false and the largest `changing` reported. The results of the rounds are in `docs/results/action-latency.md`.

## Nested upgrade checks

`make nested-upgrade` checks an upgrade from each published release to this checkout's release build, in shared mode, with Noctalia as the lock source and `wev` to type into, and then the release build reinstalled over itself. It runs one nested session per entry in `UPGRADE_FROM`, by default every `v*` tag in the clone, oldest first, then `current`, each with a 240-second deadline, and needs the network to download the releases:

```sh
make nested-upgrade
make nested-upgrade UPGRADE_FROM=0.1.1
make nested-upgrade UPGRADE_FROM=current
```

Every command it runs gets `HOME=TEST_DIR/home` and `TMPDIR=TEST_DIR/tmp`, so neither install touches your HOME, `~/.local/bin` or runtime directory. The supervisor:

1. Packs the release build as the release workflow packs one, with `skills/niri-computer-use`, the Noctalia plugin from `noctalia/niri-computer-use` without its tests, and a `.sha256`, under `TEST_DIR/releases` as release `current`.
2. Installs the old release with `install.sh`, `NCU_VERSION=<release>`, `NCU_BIN_DIR=TEST_DIR/home/.local/bin` and `NCU_REGISTER=0`, from GitHub, which checks the tarball against its `.sha256`. The binary must be in place and the skill linked from `.claude/skills` and `.agents/skills`.
3. Writes a policy file every release reads. For each key added since 0.1.0 (`NEWER_POLICY` in `harness/src/upgrade.rs`), an old binary from before it must report a policy file with only that key `invalid` in its `status`: 0.1.0 and 0.1.1 for `turn_on_accessibility = false`, 0.1.0 to 0.1.5 for `notify_on_lease = false` and for `launch_desktop_entries = true`. A release that already reads a key skips its check. Add a key there in the same commit as the key itself. Then it starts two clients of the old binary, A and B. Both must be on one shared engine whose `status.engine.version` is the release's, and A takes the lease. The audit log's lines so far are kept.
4. A types 1000 characters into `wev`. Once the marker names a key, or the `wtype` child of a 0.1.0 marker, the old engine is stopped with `SIGSTOP`, and `current` is installed the same way, from `file://TEST_DIR/releases`; the engine then continues, and A's `type_text` must finish without an error, or stop with `lock_stale`, and the marker go. The engine was stopped through the whole install, so its lock reading may be too old when the next keys go out, and every release stops there rather than type on; which comes first is up to the scheduler. The binary and the skill's `SKILL.md` must then be the build's, byte for byte, and the old engine must still run its deleted file and report `replaced: true`.
5. Starts client N of the new binary. Refused by the old engine with `engine_version`, it must be served standalone, saying so in `status.engine.fallback`, which must name the old engine's PID and say to restart the clients that engine serves, and its `acquire_desktop` must fail with `lease_held` naming A.
6. A gives the lease up and N takes it. N types 1000 characters into `wev`; once the marker names a key, N's server is stopped with `SIGSTOP` while B's `status` reads the marker, which must read as a marker, not as unreadable. Typing takes less than a tenth of a second, so the marker is looked for every millisecond. N then finishes, the marker goes, and A takes the lease again.
7. A types the same way. Once the marker names a key, or the `wtype` child of a 0.1.0 marker, the old engine is stopped. N's `status` must read the marker whole, and N's `acquire_desktop` must fail with `recovery_required` saying another server's input may still be finishing. The old engine is then killed with `SIGKILL`; once the engine's crash guardian, which holds the lease while it sends the releases the marker names, has exited, N must get `recovery_required`, and the new binary's `recover`, answered `yes`, must exit 0 and clear the marker, saying it ended or found exited the input child a 0.1.0 marker names and sent zero modifiers. N then takes the lease.
8. Starts client S of the new binary, which must be served by a shared engine, with no fallback, as no old engine runs any more and B hasn't called since its engine ended, so it hasn't started one of the old build again. B's next call reaches that engine, which refuses it: it must fail with `engine_unavailable`, its `detail` saying to restart this MCP client.
9. The audit log must keep every line the old build wrote, in order, followed by the new build's, each one JSON. N's `status.policy.state` must be `loaded`.
10. Ends B, N and S, and waits for the new engine to exit.

The supervisor log has what each side read and what `recover` said. The run doesn't check that every server bridged to the engine, as other shared runs do: N is served standalone on purpose.

With `UPGRADE_FROM=current`, the supervisor packs the build as in step 1 and installs it, then reinstalls it over itself, as a reinstall of the same release writes a new file:

1. Starts clients A and B, which must share one engine, then installs `current` again. A's `status` must report `replaced: true`.
2. Starts client N, which must be served by a new shared engine, with no fallback, no `unscoped` reason, so in a scope of its own, and `replaced: false`; the old engine must exit within two seconds.
3. A's and B's next calls must fail with `engine_lost`, and the calls after with `engine_unavailable`, saying to restart this MCP client. The audit log must have both `engine_lost` lines, which their bridges wrote.
4. Ends A, B and N, and waits for the new engine to exit.

Releases up to 0.1.5 don't hand over, so the runs from their tags keep checking that N is served standalone beside the old engine.

## Nested multi-output checks

`make nested-multi` runs the pointer tools on two outputs. niri's `winit` backend has one output and its headless backend has no command-line switch, so the target builds niri v26.04 from source with `harness/niri-test/headless-outputs.patch`. The patch makes niri start its headless backend when `NIRI_TEST_HEADLESS_OUTPUTS` is set, with one output per `WxH` size it lists, named `headless-1`, `headless-2` and so on. It needs what `make niri-matrix` needs to build niri, and what `make nested-input` needs except cage:

```sh
make nested-multi
```

`scripts/niri-build.sh` applies the patch when `NIRI_PATCH` names it, and keeps that build apart from the unpatched one, under `bin/<commit>-<first 12 hex digits of the patch's SHA-256>/niri`, so the first run takes a niri build and later runs reuse it. The run is local and opt-in; CI doesn't run it.

The nested niri draws nowhere, so no cage starts and `--visible` is refused. `WAYLAND_DISPLAY` still names cage's socket path with nothing at it, so a niri built without the patch fails to connect there and exits. Before anything else, the supervisor requires every output to be `headless-<n>` made by `niri`, model `headless`; C2's single-output check is skipped. With the nested Noctalia as the lock source and one server holding the lease, it:

1. requires niri's outputs to be `headless-1`, 1280x720 at scale 1 at (0, 0), and `headless-2`, 1920x1080 at scale 1.5 at (-1280, 100), so one output's origin is negative;
2. requires `status`'s `outputs.pointer_outputs` to list both;
3. moves `wev` to each output with `MoveWindowToMonitor` and runs C4's five points through `pointer_move`, from a screenshot of that output at its own scale and one lowered to 700 pixels wide, each within ±0.05 px of the pixel's centre;
4. moves the GTK 4 fixture to each output and clicks its button 20 times there from a screenshot of that output; each click must count once.

It shows that niri maps the pointer through the bound output at mixed scales and a negative origin. It doesn't plug or unplug an output mid-gesture: the unit tests against the fake niri cover that. The results are in `docs/results/multi-output.md`.

## Nested Chromium checks

`make nested-chromium` types and pastes text through the server into a Chromium text area and compares what arrived with what was sent. It runs in the headless nested niri, with Noctalia as the lock source, and has a 240-second deadline. It needs a Chromium-based browser: `NCU_CHROMIUM=<path>` picks one, which must then be an executable file, and otherwise it takes the first of `chromium`, `chromium-browser`, `google-chrome-stable`, `google-chrome`, `brave` and `helium-browser` on `PATH`. The path is used as found, links unresolved, since some launchers pick what to run from their own name. A browser under `/snap` is refused, because a snap keeps its data in `~/snap` whatever `HOME` is. With neither, the run logs that it is skipped and exits 0, so it needs nothing in CI.

```sh
make nested-chromium
NCU_CHROMIUM=/path/to/chrome make nested-chromium
make nested-chromium SHARED=1
make nested-chromium-headless
```

`make nested-chromium-headless` runs the same suite without cage, on niri's own headless backend: it builds and caches the patched niri v26.04 as `make nested-multi` does, and runs `harness run --niri <that niri> --headless-niri --chromium`. `--headless-niri` gives niri one 1280x800 output, `headless-1`, and refuses `--visible` and every suite but `--chromium`. That backend sends a window frame callbacks only from niri's fallback timer, about once a second, since it draws nothing, so a page there draws its next frame up to a second late. Only the masked field below depends on frames, and it lost keys the same way on a real display, in the use `docs/results/chromium.md` describes.

The browser opens `harness/fixtures/chromium.html` in app mode (`--app`) with `--ozone-platform=wayland`, inside the nested niri. Its profile (`--user-data-dir`), its `HOME`, its `TMPDIR` and every XDG directory are under `TEST_DIR`, so it never reads or writes your profile or leaves its singleton socket's directory in `/tmp`, and first-run UI, sync, background networking, component updates, extensions and the crash reporter are off. After every input the page sets its title to `ncu-chromium:` and the field's value as UTF-16 code units, four hex digits each, and the supervisor reads it from niri's window list. A browser that exits before its window opens fails the run at once, with its exit status and the path of `chromium.log`. A title is the one channel that needs nothing beside the browser: the page can't write files, and accessibility would test AT-SPI as well. The code units are needed because Chromium collapses and trims whitespace in titles. The page also logs each keydown's DOM `code` and `key` to the console, which `--enable-logging=stderr` copies into `chromium.log`. The run lists the ones whose `key` isn't one ASCII character, which shows the keycode each symbol the layout lacks went out on.

Each case is typed with `type_text`, then pasted with `via: "paste"`, and the field is emptied with `ctrl+a` and `BackSpace` between them. A result counts once the field has stopped changing for a second, even when it shows the text sent, so a key repeated or delivered late after the text is caught. The cases are ASCII, the C10 corpus, a space or period first and fourteenth (the places wtype's Escape and Backspace codes used to drop or delete them), three, fourteen and thirty symbols a US layout lacks, accented Latin, Cyrillic and two emoji. Typed text the project says arrives exactly, and every paste, must arrive exactly, or the run fails after logging every case. Typed emoji are known to arrive cut to 16 bits in Chromium, so the emoji case passes when its text arrives either exactly, which the log calls a fix, or with each emoji cut to its lower 16 bits and the rest intact. Anything else fails, since the text around the emoji is still promised.

Then three form fields, each in a browser of its own opening `harness/fixtures/chromium-forms.html?field=<name>`, which publishes the field's value in its title as the text area's page does: `number`, a plain `<input type=number>`; `controlled`, a number input driven as React drives a controlled one, each input going through a reducer and a synchronous render that spends 8 ms busy; and `masked`, a text input as `controlled` behind the check `@react-input/core` 2.0.2 puts in front of masked fields such as `@react-input/number-format`'s. That check undoes an input that comes before a `setTimeout(0)` chain has run since the previous one, and the page logs each one it undoes. `100`, `25`, `10` and `1234567890` are typed into each and pasted into each. The `number` and `controlled` fields must get every key, every paste must arrive exactly, and the `masked` field's typed text passes when it is the text's first character and then some of the others in order, with the page logging an undo for each key missing, which the log calls a known loss, or the whole text, which it calls a fix. The results are in `docs/results/chromium.md`.

`make nested-chromium-a11y` checks what Chromium needs before `elements` can read its pages. It finds the browser as `make nested-chromium` does, skips the same way without one, and has the same 240-second deadline:

```sh
make nested-chromium-a11y
make nested-chromium-a11y SSD=1 SCALE=1.5
```

`SCALE` sets the nested output's scale, 1 by default, and `SSD=1` adds `prefer-no-csd` to the nested niri's config. Before anything else, it checks that `DBUS_SESSION_BUS_ADDRESS` names a socket under the run's directory, so `org.a11y.Status` is the nested session's and never yours; the run only reads it, with `busctl`. It starts `at-spi2-registryd` as `make nested-a11y` does, then Noctalia and one server holding the lease. Each browser opens `harness/fixtures/chromium-a11y.html` with a profile of its own under `TEST_DIR`: a labelled field, a button that writes `sent` into it, and with `?rows=N`, N table rows of a link, a check box and a button. The page's title carries the row count and the field's value. The run fails unless:

1. The first browser's window process maps `icudtl.dat` or `v8_context_snapshot.bin`, as its `/proc/<pid>/maps` shows, which is how the server tells a Chromium-family app.
1. `IsEnabled` is false before the server, and a browser started then is refused with `not_accessible`, whose detail says the server turned accessibility on, after which `IsEnabled` is true.
2. A browser started after that is on the registry, `elements` lists its field and button, `set_element_text` on the field fails `text_not_settable`, since the field has no `EditableText`, with a detail that names `activate_element` and `type_text`, and `activate_element` on the button changes the page's title to the field's `sent`.
3. Another browser's window, moved to the floating layout, lists its button with a `layout_box` under `SSD=1`, and `click` with the button's `element_ref` changes the page's title to `sent`; without `SSD=1` the button is `unmappable: frame_size_mismatch`, since Chromium's frame then includes its shadow.
4. The first browser is still refused, with a detail that says accessibility is on.
5. On pages of 300 and 2,000 rows, the first listing says `building: true`, and the page's field is listed within 30 seconds.
6. After the server ends, `IsEnabled` is false again.

It also logs what isn't promised: the frame's extents beside niri's window size, whether each browser joined or left the bus three seconds after accessibility turned on or off, and each long page's listings with their times, refs and `unbound` reasons. `docs/decisions.md` has the numbers.

## Skill evals

`make nested-eval` runs an agent against one scenario in a nested niri and grades what it did. It needs `claude` on `PATH` and logged in, and it spends tokens:

```sh
make nested-eval SCENARIO=compose-message SKILL=skills/niri-computer-use MODEL=sonnet
```

`SKILL=none` runs without a skill. The scenarios are `compose-message`, `key-screenshots`, `click-elsewhere`, `stopped`, `no-preset`, `dialog-midway` (a window takes focus as soon as the agent takes the lease), `errand` (launch, send, close, and back to the user's window), and four everyday workflows:

- `form`: fill in a GTK 4 sign-up form (two entries, a check box and a drop-down) and submit it. Graded from what the form recorded: one submission, every field as asked.
- `rearrange`: put the `notes` and `calendar` windows side by side, each half the screen, and move `music` to workspace 2. Graded from niri's final layout: neighbouring columns on one workspace, each 40 to 60 % of the output's width, `music` on workspace 2 of the same output, no window opened or closed. niri gives no position for tiled windows, so whether both are in view isn't graded.
- `copy-files`: in Files (Nautilus, with a home of its own under `TEST_DIR`), move two screenshots into a new folder `release-0.1`. Graded from the files on disk: both screenshots in the folder, byte for byte, and the rest of `Pictures` untouched.
- `modal-recovery`: type a message into a GTK 4 message box and send it; as soon as the box first holds text, a modal `Autosave failed` dialog opens and blocks the form until its `OK` button, Escape or a close request dismisses it; keys typed into it are lost. Graded from what the fixture recorded: the dialog shown and dismissed, the whole message sent once, the window left open, and the user told about the failed autosave.

The harness starts Noctalia, and for `form`, `copy-files` and `modal-recovery` an accessibility bus, as a desktop has, so `elements` is listed. It sets up the scenario's windows (`wev`, `harness window` fixtures such as `notes`, `harness/fixtures/workflow.py` or Nautilus), copies the skill into `TEST_DIR/agent/.claude/skills/`, and runs `claude -p` there with only the nested server and the `Skill` and `Read` tools. It then grades the nested server's audit log, the fixtures' logs and records, niri's final state, the files on disk and the transcript: for example, no screenshot that overlaps an action, no action sent in the same turn as another desktop call, Enter pressed once and only after all the text went out, focus back on the user's window, and no more calls after a `stopped` refusal.

The run's files are in `target/e2e/<run>/`: `transcript.jsonl` (the agent's stream), `audit.jsonl`, `answer.md` (its final answer), and `grading.json` and `timing.json` (with the number of calls and agent turns) in the skill-creator's format. `rearrange` adds `layout.json`, niri's windows, workspaces and outputs at the end, and `form` and `modal-recovery` copy the fixture's records (`signups.jsonl`, `sent.jsonl`, `draft-events.jsonl`). The run has a 15-minute deadline. Claude Code also keeps its usual transcript under `~/.claude/projects/`.

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

The home page replays one recording's calls, `errand`'s, and links `site/public/clips/errand.mp4`, the desktop without the band, which plays in place. Cut the band off the MP4 and keep the 16:9 desktop:

```sh
ffmpeg -i site/public/demo/errand.mp4 -vf "crop=iw:iw*9/16:0:0,scale=1280:720:flags=lanczos" -an \
    -c:v libx264 -preset slow -crf 30 -tune animation -pix_fmt yuv420p -movflags +faststart site/public/clips/errand.mp4
```

The README links `errand.gif`, `errand.png` and `errand.mp4` in `site/public/demo/`, and link previews use `og.png`, a screenshot of the home page at 1280×640. The home page's EXAMPLE lists each call's `start` and `ms` from `errand.json`, copied into `site/src/lib/home.ts`; nothing reads the JSON itself, the WebM or another session's files, so don't commit them.

`make nested-demo SCENE=<scene>` still plays a scripted scene in a headless nested niri (`errand`, `layout`, `keyboard` and `stop`, against `harness/fixtures/chat.py`, a GTK 4 chat window) and writes the same `frames/` and `timeline.jsonl` to `target/e2e/<run>/`, which `scripts/demo-encode.py` takes at its default width of 1280. It makes the same calls each take, which helps when comparing builds; the site no longer uses it.

## Real-session pointer check

`scripts/real-pointer-check.py` is M4's supervised accuracy run on a real monitor. It sends real pointer motion to your session, so run it only yourself, at the machine, with your hands off the mouse:

```sh
python3 -I scripts/real-pointer-check.py target/debug/niri-computer-use /tmp/ncu-m4-real-1
```

It opens `wev` on the focused workspace and floats it at 400x300 through niri IPC. Then, for each output named with `--output` (DP-1 if none is), it moves `wev` to that output at (200, 200) and with the server's `pointer_move` moves the pointer to five points on `wev`'s surface, from a screenshot of that output at its own scale and one 1280 pixels wide. It sends no click, key or scroll. Each motion must land within ±0.05 px of the pixel's centre; `run.log` in the directory you name says where each one did. Each output must be at transform `Normal`; with a laptop and an external monitor, add `--output` once for each.

## Supervised sitting

`make sitting` opens the nested output at scale 1.5 and runs C9's physical positive control: a chord pressed by hand must fire niri's `Ctrl+Shift+F12` test bind, which the server's same chord must not (step 7 of `make nested-input`). It starts a focused wev. Noctalia does not run in this mode. The automatic path keeps its existing deadlines; the sitting allows 30 minutes overall, 29 minutes for wev and two minutes for each human action or confirmation.

The supervising agent must confirm that you are present before starting. Follow one cue at a time from `supervise.log`:

1. Click the checkerboard in the nested window at bottom-right. Do not press a key yet. Confirm the click and that you see the checkerboard.
2. When cued, click the checkerboard, hold Ctrl and Shift, press and release F12 once, then release Shift and Ctrl. Confirm the chord, release of all keys and what you saw.

wev shows a checkerboard, not typed text. The supervisor checks raw events and requires the human's confirmation separately. Only after that confirmation, the supervising agent creates `target/e2e/<run>/confirm-N.txt` containing one line beginning `confirmed: ` followed by the human's statement. A missing confirmation times out; a malformed statement fails. Never create these files in advance or infer a human confirmation from logs.

Its artifacts include `wev.log`, `confirm-N.txt`, `sitting-ready.png` and `success-c9.png`. C8, the interrupted pointer, needed the `vpointer` probe, and M4 deleted it: its evidence stays in `docs/results/m0.md`, and its automatic half, a button left pressed by a killed pointer and released from a fresh one, runs in `make nested-input`'s crash check. C14 is a separate host read; the sitting never locks the host.

## Host capture

C15 also measures captures of a real output. This reads `niri msg --json outputs` and runs `grim` 20 times on the output you name. The images stay in memory and only their sizes and timings are kept:

```sh
make host-capture OUTPUT=DP-1
```

The report goes to the terminal and to `target/e2e/<unix time>-<pid>/harness.log`.

## niri compatibility matrix

`make niri-matrix` builds niri from source at each git ref in `NIRI_VERSIONS` and runs this checkout's server against each build in the headless nested session. It writes the table to `docs/results/niri-matrix.md` and exits non-zero when a release does other than what is expected of it:

```sh
make niri-matrix
make niri-matrix NIRI_VERSIONS="v26.04 main"
```

`NIRI_VERSIONS` defaults to `v25.08 v25.11 v26.04 main`. Any tag, branch or commit of niri works.

`scripts/niri-build.sh` does the builds. It clones niri from `NIRI_REPO` (default `https://github.com/YaLTeR/niri.git`) into `NIRI_MATRIX_DIR` (default `~/.cache/niri-computer-use/niri-matrix`), fetches it again on each run, and builds each commit in its own worktree with `cargo build --release --locked`, sharing one Cargo target directory. A commit that already has `bin/<commit>/niri` isn't built again, so `main` is rebuilt only when it has moved. The script runs no niri binary and installs nothing. Before building, it checks for `git`, `cargo`, `clang`, `pkg-config`, `timeout` and the pkg-config modules niri's Building section asks for, and stops with the list of what is missing. A ref that fails to build is recorded with the line naming a missing system library, or else its first `error` line, and its build output is kept in `logs/<commit>.log`; the other refs still build. Fetching has a 10-minute deadline and each build an hour.

`NIRI=<path>` (or `harness run --niri <path>`) runs any nested target with that niri instead of the one on `PATH`; `make nested-compat NIRI=<path>` runs only the probe below. The binary must be named `niri`, since the server finds niri by its process name. Only the nested session uses it: it validates the nested config (`niri validate`, with the nested session's environment), runs as the nested niri, and comes first on the nested session's `PATH`. C1's host snapshot still runs the host's `niri msg`.

For each niri, the matrix first runs `--compat`, a probe in the nested session. With Noctalia and the GTK 4 fixture open, the fixture drawn by GTK's Cairo renderer because niri 25.11 in the headless cage didn't show the window GTK's default GL renderer drew, it:

1. waits up to 20 seconds for `status` to see niri's version, Noctalia running and the screen unlocked, and goes on with `status` as it is when that doesn't come;
2. calls `status`, `desktop_state`, `screenshot`, `elements` on the fixture's window and `wait_for` its `app_id`, each of which must answer;
3. calls every other tool the server lists, without the lease and with arguments it accepts: `release_desktop` must answer, and each of the others must be refused with `lease_required`. A tool the probe has no arguments for counts as a miss;
4. reads `status` again, since a capture can make niri send an event, and calls `acquire_desktop`, which must succeed when `niri.compat` is `ok` or `patch_warning` and be refused with `read_only` when it is `read_only`.

Each call's outcome goes to `compat.json` in the run's artifacts, also when a call goes otherwise. A niri whose ref is a release tag (`v<major>.<minor>` or `v<major>.<minor>.<patch>`) with the pinned `niri-ipc`'s major and minor is expected `Full`: it must report `ok` or `patch_warning`, have its event stream `connected` at step 4's `status`, pass the probe and pass `--control`, `--actions`, `--input`, `--shell` and `--a11y`, which the matrix then runs; one that didn't build fails the matrix too. Any other release is expected `Observe`: it must report `read_only`, have its event stream `connected` at step 4's `status` and pass the probe; one that didn't build is recorded as not built, since an old release can stop building against newer system libraries. A ref that isn't a release tag, such as `main`, is `Record`: it gets the probe and the five suites, and whatever they do is recorded without failing the matrix. The results file lists each miss and each failed suite with its error, and the run names under `target/e2e/`.

### Supporting a new niri release

1. Read niri's release notes and `git diff <old tag> <new tag> -- niri-ipc` for new or changed requests, replies, events, actions and enum variants. The pinned `niri-ipc` can't parse an enum variant it doesn't know. A reply it can't parse fails that tool with `upstream_error`; an event it can't parse twice stops the server's event stream until the server restarts (`event_stream: schema_incompatible`), and the server is read-only meanwhile. Screencast events are the exception: the server drops them unread (`events::IGNORED` in `src/niri/events.rs`). A new kind of event isn't on that list, so it still stops the stream until it is either read or shown safe to drop.
2. Before bumping, run `make niri-matrix NIRI_VERSIONS="<old tag> <new tag>"`. The new release should be read-only with the observation tools working; anything else is what breaks for users who update niri before the server.
3. Bump the `niri-ipc` pin in both `Cargo.toml` and `harness/Cargo.toml`, the `IPC_CRATE` constants in `src/niri/version.rs` and `harness/src/matrix.rs` (tests check each against the pins), and the version `docs/decisions.md` records. Fix what the new types break, such as a new field to fill in or a new variant to match.
4. Run `make check` and the matrix with the new tag, the previous one and `main`. The new tag must pass everything and the previous one must now be read-only.
5. Update the niri version the docs name: the README's badge and its niri-only paragraph, `docs/architecture.md`'s version rule, the site's install and looking pages, and this file's requirements.

## Releases

The crate holds only `src/`, `README.md`, `LICENSE` and `CHANGELOG.md` (`include` in `Cargo.toml`). To see what it would hold and check that it builds on its own:

```sh
cargo package --list -p niri-computer-use
cargo publish --dry-run -p niri-computer-use --locked
```

Pushing a `v<version>` tag runs `.github/workflows/release.yml`. Only repository admins can create `v*` tags, and no one can move or delete one (two tag rulesets). The workflow fails unless the tagged commit is on `main`, then runs the whole CI workflow on that commit itself and builds only if it passed. A tag runs the release workflow of the commit it names, so tag a commit that has this one: older commits' release workflows have none of these gates. It fails unless the tag matches the crate's version and `CHANGELOG.md` has a `## <version> ` section. It builds `x86_64-unknown-linux-gnu` on `ubuntu-24.04` and `aarch64-unknown-linux-gnu` on `ubuntu-24.04-arm`, each with `--locked --release` on its own architecture, checks that the stripped binary prints its version, and packs it with the README, license, changelog, `skills/niri-computer-use/` and a `THIRD-PARTY-LICENSES.txt` that cargo-about writes from `.github/release/` for that target, as `niri-computer-use-<version>-<arch>-linux.tar.gz` with a `.sha256` beside it. It hands both tarballs, their checksums and `install.sh` to a second job, which creates a draft GitHub release with that section as its notes. That job runs in the `release` environment, so it waits until the repository owner approves it in the run's page. Only that second job can write to the repository, and it runs no build code. Nothing is published until someone publishes the draft.

`install.sh` installs a release the way the Install page's [steps by hand](https://ayagmar.github.io/niri-computer-use/start/install/#by-hand) do; [The install script](https://ayagmar.github.io/niri-computer-use/start/install-script/) describes it for users. `tests/install.rs`, part of `make test`, runs it against fake releases that a thread of the test serves over HTTP, with a fake `uname` and the test's own directory as `HOME`:

```sh
cargo test --locked --test install
```

### Cutting a release

A `v*` tag can't be moved or deleted, and a crates.io version can't be replaced, so each step checks what the next one freezes.

1. On the release branch, run `make nested-upgrade` and check that it passes. It upgrades from every `v*` tag, so run `git fetch --tags` first.
2. Merge the release branch into `main`. It dates the version's `CHANGELOG.md` section as `## <version> - <date>`, keeping the space after the version that the workflow looks for, and names the version in the Install page's download links.
3. Wait for `main`'s CI to pass on the merge commit:

   ```sh
   gh run list --repo ayagmar/niri-computer-use --workflow ci.yml --branch main --limit 1
   ```

4. Tag that commit with a lightweight tag and push the tag:

   ```sh
   version=0.1.1
   git switch main
   git pull --ff-only
   git log -1 --oneline
   git tag "v$version"
   git push origin "v$version"
   ```

5. In the Release run's page, approve the `release` environment once `build` has passed. The build's files are kept for 30 days.
6. Check the draft's files the way the Install page's steps by hand do. The draft isn't public, so download it as the owner:

   ```sh
   gh release download "v$version" --repo ayagmar/niri-computer-use --dir "/tmp/ncu-v$version"
   cd "/tmp/ncu-v$version"
   sha256sum -c ./*.sha256
   tar -xzf "niri-computer-use-$version-x86_64-linux.tar.gz"
   "niri-computer-use-$version-x86_64-linux/niri-computer-use" --version
   ```

   The draft should hold five files: both tarballs, their `.sha256` and `install.sh`. Read the draft's notes against the changelog section too.
7. Publish the draft:

   ```sh
   gh release edit "v$version" --repo ayagmar/niri-computer-use --draft=false
   ```

8. Publish the crate from the tagged commit:

   ```sh
   cargo publish --locked -p niri-computer-use
   ```
