# Decisions

Dependency and design decisions, newest last. Each entry says what was chosen, why, and what was considered instead.

Version rule: use the newest stable release that is at least 7 days old, and record its publish date here.

## 2026-10-06: Rust 1.99.0

- Pinned in `rust-toolchain.toml`, with `rust-version = "1.99"` in `Cargo.toml`.
- Released 2026-09-28. That's the newest stable release at least 7 days old.
- This is an application, so there's no reason to support older compilers. Both values move together, by hand, in one `build:` commit.

## 2026-10-06: `const fn main` in the skeleton

- Clippy 1.99 reports `missing_const_for_fn` for an empty `main`. Marking it `const fn` satisfies the lint without an `#[expect]`.
- It goes away once `main` does real work. Don't copy it as a pattern.

## 2026-10-06: CI actions

| Action | Pin | Latest release when chosen |
|---|---|---|
| `actions/checkout` | `v7` | v7.0.1, 2026-07-20 |
| `taiki-e/install-action` | `v2` | v2.87.26, 2026-10-06 |

- Actions are pinned to a major tag, so patch releases arrive without the 7-day delay. `install-action` publishes almost daily.
- Dependabot only proposes major-version bumps for these, with a 7-day cooldown.
- Pinning exact versions or commit SHAs would enforce the delay, at the cost of frequent update PRs. Revisit if that trade-off changes.

## 2026-10-06: CI tool versions

These match the versions installed locally.

| Tool | Version | Published |
|---|---|---|
| cargo-deny | 0.20.2 | 2026-07-09 |
| cargo-machete | 0.9.2 | 2026-04-15 |
| typos-cli | 1.50.3 | 2026-09-25 |
| shellcheck | 0.11.0 | 2025-08-04 |

- typos-cli 1.51.0 came out on 2026-10-06, so it's too new.
- Dependabot doesn't update these pins. Bump them by hand in `.github/workflows/ci.yml`, following the version rule.

## 2026-10-07: harness dependencies

| Crate | Version | Published | Why |
|---|---|---|---|
| `niri-ipc` | `=26.4.0` | 2026-04-25 | niri's own request and response types. Pinned to the installed niri 26.04, as the version rule's exception requires. |
| `serde_json` | 1.0.151 | 2026-07-20 | Encodes requests and decodes replies. `niri-ipc` already depends on it. |
| `rustix` | 1.1.5 | 2026-09-16 | Waits for a child without reaping it (`waitid` with `WNOWAIT`) and kills its process group, without `unsafe`. Feature `process` only. |
| `signal-hook` | 0.4.4 | 2026-04-04 | Sets a flag on Ctrl+C, SIGTERM and SIGHUP, so `harness run` can kill its children's process groups. They don't get the terminal's signals because each has its own group. 0.4.5 (2026-10-04) is too new. |

- `niri_ipc::socket::Socket` has no timeouts, so the harness writes the same JSON line to a `UnixStream` with read and write timeouts.
- The harness is synchronous. It waits for a child on a separate thread and uses `recv_timeout` for the deadline, so it needs neither `tokio` nor `std::thread::sleep`.
- The runner kills the child's process group before reaping the child. Until then the child's ID can't be reused, so the kill can't reach an unrelated group.
- Captured output is read on its own threads. Once the child is gone, the runner waits at most 2 seconds for both streams together, because a descendant that left the group can hold a pipe open.
- After killing a child that timed out or was interrupted, the runner waits at most 2 seconds for it to exit. If it hasn't, the run fails without reaping it.
- `Cargo.lock` was checked against the version rule, transitive crates included. `libc` 0.2.190 (2026-10-02) was too new, so it is held at 0.2.189 with `cargo update -p libc --precise 0.2.189`.

## 2026-10-07: private bus config for the nested harness

- `dbus-run-session` normally uses `/usr/share/dbus-1/session.conf`, which listens with `unix:tmpdir=/tmp`. That put the bus socket in the host's `/tmp` (seen as `unix:path=/tmp/dbus-…`).
- The harness passes `--config-file` with `<listen>unix:dir=$TEST_DIR/run</listen>` and no service directories, so the socket lives in `TEST_DIR` and nothing on the host can be activated through the bus.
- Unix socket paths are limited to 108 bytes. A test with a long directory failed with `Socket name too long`. The harness refuses a `TEST_DIR/run` longer than 78 bytes, which leaves room for niri's socket name.

## 2026-10-07: the supervisor quits the nested niri

- niri starts its `--` command with a double fork and with stdin, stdout and stderr set to null (`src/utils/spawning.rs` at `v26.04`). It doesn't wait for that command or exit when it does.
- So the supervisor writes its log and status to files, and sends `Action::Quit` to the nested niri when it is done. It sends that only after `NIRI_SOCKET` resolves to a path under `TEST_DIR/run` and niri has reported `winit` as its only output. The host niri drives real outputs, so it can't pass the second check. If either check fails it sends nothing more, and the harness deadline kills the process group.
- The supervisor keeps one niri connection from the version request through `Quit`. niri answers any number of requests on one connection (`src/ipc/server.rs`, `handle_client`), and `Quit` then reaches the niri that was identified even if the socket path changed in between. The plan's one-connection-per-request rule is for the MCP server's actions; the harness doesn't need it.
- `harness supervise` takes the artifacts directory as an extra argument, so the supervisor can write there.

## 2026-10-07: `vpointer` probe dependencies

| Crate | Version | Published | Why |
|---|---|---|---|
| `wayland-client` | 0.31.15 | 2026-07-22 | Connects to the compositor, binds the seat and outputs, and reads `wl_output.name`. |
| `wayland-protocols-wlr` | 0.3.12 | 2026-03-31 | The `zwlr_virtual_pointer_v1` bindings. Feature `client` only. |

- With default features, `wayland-client` uses its pure-Rust backend. The system libwayland isn't needed: `ldd` on the probe lists only libc and libgcc_s. The `system` feature would switch to libwayland.
- Every crate in `probes/vpointer/Cargo.lock` was checked against the version rule, transitive crates included. `libc` 0.2.190 (2026-10-02) and `cc` 1.6.0 (2026-10-03) were too new, so they are held at `libc` 0.2.189 (2026-07-21) and `cc` 1.5.1 (2026-09-25) with `cargo update --precise`. `cc` is a build dependency of `wayland-backend` and only compiles C when that crate's `log` feature is on, which it isn't.
- The rest of the lock, with publish dates from crates.io. Each is the newest release at least 7 days old that its dependents allow. `downcast-rs`, `quick-xml` and `windows-link` have newer releases outside the ranges `wayland-backend` (`^1.2`), `wayland-scanner` (`^0.41`) and `windows-sys` (`^0.2.1`) accept.

  | Crate | Version | Published |
  |---|---|---|
  | `bitflags` | 2.13.2 | 2026-09-10 |
  | `downcast-rs` | 1.2.1 | 2024-04-07 |
  | `errno` | 0.3.14 | 2025-09-09 |
  | `find-msvc-tools` | 0.1.14 | 2026-09-25 |
  | `linux-raw-sys` | 0.12.1 | 2025-12-23 |
  | `memchr` | 2.8.3 | 2026-07-08 |
  | `pkg-config` | 0.3.34 | 2026-08-14 |
  | `proc-macro2` | 1.0.107 | 2026-07-19 |
  | `quick-xml` | 0.41.0 | 2026-06-29 |
  | `quote` | 1.0.47 | 2026-07-19 |
  | `rustix` | 1.1.5 | 2026-09-16 |
  | `shlex` | 2.0.1 | 2026-05-17 |
  | `smallvec` | 1.16.2 | 2026-09-25 |
  | `unicode-ident` | 1.0.26 | 2026-09-17 |
  | `wayland-backend` | 0.3.17 | 2026-08-14 |
  | `wayland-protocols` | 0.32.13 | 2026-06-19 |
  | `wayland-scanner` | 0.31.11 | 2026-07-22 |
  | `wayland-sys` | 0.31.11 | 2026-03-31 |
  | `windows-link` | 0.2.1 | 2025-10-06 |
  | `windows-sys` | 0.61.2 | 2025-10-06 |
- The probe has its own `Cargo.lock`, committed, so its results can be reproduced.

## 2026-10-07: pointer and capture steps in the supervisor

- wev 1.1.0 never flushes stdout (no `fflush` or `setvbuf` in `wev.c`), and with stdout in a file, glibc buffers it in blocks. The supervisor runs `stdbuf -oL wev` (coreutils), so each event reaches `wev.log` as a line.
- `wait_until` polls every 50 ms with `std::thread::sleep`, behind one `#[expect(clippy::disallowed_methods)]`. The ban exists so nothing blocks an async runtime, and the harness has none. Polling niri state and the `wev` log is how plan §13 describes `wait_until`. `thread::park_timeout` would avoid the lint without saying why.
- Each probe call sends its own event time. niri passes it through to the client's `wl_pointer` events (a probe motion with time 4001 shows up in `wev.log` as `motion: time: 4001`), so each check reads only its own events.
- C3 asks grim for a PPM, which is a short header followed by raw RGB, so the harness needs no image decoder. C15 reads the size from the PNG `IHDR` chunk or the JPEG start-of-frame segment, as plan §8 says the server will. Both take a few dozen lines, so neither needs a crate.
- The runner can now start a process and stop it later, which `wev` needs. A watchdog thread kills the process at its deadline. It shares a lock with the code that reaps the child, and only kills while the child is unreaped, so it can't signal a reused process ID. Any ending seen at or after the deadline, including a clean exit, counts as a timeout. Dropping the handle kills the process.
- `wait_until` ignores a value its condition returns after the deadline. A condition that asks niri can take that request's own 5-second deadline, so a wait can end up to 5 seconds late, but it then fails instead of passing.
- C15 on a host output is a separate command, `harness host-capture`, run from your shell. The supervisor can only reach the nested niri.

## 2026-10-07: keyboard checks and the stdin gate

- The existing runner holds and feeds stdin, so C5(b) needs no new probe or dependency. A writer runs on a thread and waits only until the child's original deadline; the existing watchdog bounds a child that doesn't read. All wtype children start through the checked nested `Session`.
- Keyboard checks use byte offsets in the append-only `wev` log, because wtype 0.4 sends key time 0 (`main.c`, `type_keycode` and `run_key`). The parser includes the key and modifier continuation lines from wev 1.1.0 (`wev.c`, `wl_keyboard_key` and `wl_keyboard_modifiers`). A partial record waits for its final newline; a malformed complete record fails.
- `still_absent` checks for the full interval and once at its end. C5(b) watches keys and modifiers for two seconds while stdin is open, and C9 watches `bind-fired` for one second after the chord. Both passed at scale 1 and 1.5.
- Keep the stdin gate, and keep the 100-character cap and three-second wtype deadline. The slowest of the ten C10 calls was 624.112 ms, below the 1500 ms decision threshold. Keeping wtype remains conditional on C6's human keymap-restoration check.
- Sources: [wtype 0.4](https://raw.githubusercontent.com/atx/wtype/v0.4/main.c) and [wev 1.1.0](https://git.sr.ht/~sircmpwn/wev/blob/1.1.0/wev.c).

## 2026-10-07: no system bus in the nested session

- PARENT sets `DBUS_SYSTEM_BUS_ADDRESS` to `TEST_DIR/run/no-system-bus`, where nothing listens. A nested Noctalia on the real system bus registered itself as BlueZ's default pairing agent (`RequestDefaultAgent` in `bluetooth_agent.cpp`) and tried to register a NetworkManager secret agent. With its default lock screen settings it also takes a logind sleep-delay inhibitor, and sets the locked hint of the logind session it finds by its PID when it locks (`logind_service.cpp`, `application_ui.cpp`). That session is the host's wherever the harness runs inside the session scope. The config can turn off the lock screen parts but not the two agents, and all of them act on the host.
- With no system bus, Noctalia logs `system dbus disabled` and runs without those services. The nested niri loses its read-only `login1` (lid switch) and `locale1` (keyboard layout) watchers and logs a warning for each. `X11 Layout` is unset on this machine, so the nested keymap doesn't change.
- Containment and the supervisor's endpoint checks require exactly that address, and that nothing, not even a symlink, exists at its path. A path that merely lies under `TEST_DIR` could be a symlink to the host bus or a socket someone listens on.

## 2026-10-07: `noctalia-socket` probe

- The probe uses only the standard library: a `UnixStream` with read and write timeouts. It has no dependencies, and its `Cargo.lock` lists only itself.
- It sends `/`, `\x1e` and one of three fixed commands. Noctalia 5.2.1 erases everything up to the first `\x1e` before it parses a command (`src/ipc/ipc_service.cpp`), so text from elsewhere must never reach the payload. The probe picks a constant by exact match and sends that constant, never its argument.

## 2026-10-07: Noctalia in the nested harness

- No new dependencies. The harness reads `status` with `serde_json`, which it already depends on, and drives Noctalia through the `noctalia-socket` probe.
- The harness generates a Noctalia config instead of using the defaults. Without a setup marker in its fresh state directory, Noctalia opens its setup wizard as a panel on startup (`application_ui.cpp`), so `activePanelId` wouldn't start as null. Weather is off so a test run makes no weather requests. The config also lists no plugin sources (`[plugins] source = []`). By default Noctalia has two git sources, the official and community plugin repositories on GitHub, and at every start it clones each one that isn't cached yet (`ensureEnabledMaterialized` in `plugin_manager.cpp`), whatever `auto_update` says. It runs the clones in a process group of its own (`setpgid` in `process.cpp`), so they escape the harness's group kill: a reviewer's run left both clones writing under the removed `TEST_DIR` for about 20 seconds after the harness exited. The defaults apply only when the `source` array is absent (`config_service.cpp`), so an explicit empty array removes them. `auto_update = "none"` alone was tried first; the community clone still outlived the run. `noctalia config validate` exits 0 even when it warns about an unknown key, so the harness requires its plain success line.
- Noctalia starts after the `wev` checks. Its bar reserves 34 logical pixels at the top of the output, and `wev`'s position is relative to the top-left of the working area.
- C13's pass rule only reads `status`. The supervisor also compares captures of the output, because a screenshot taken right after `status` reports the panel open showed no panel: the panel was not drawn yet. Noctalia also answers `status` before it has drawn its wallpaper, and a capture taken then is the magenta background: in one review run the wallpaper alone made the panel look drawn and never gone. So the supervisor first waits until less than half the output is magenta and two consecutive captures differ in fewer than 1% of their pixels, and keeps that capture. It waits until at least 1% of the pixels differ from it after `panel-open` and saves the screenshot then. These capture waits allow five seconds rather than C13's two, because Noctalia can report the panel open long before it draws it: in one run (1791393079-462290) it logged `queued surface rendering took 1935.1ms`, and the panel wasn't drawn within two seconds. Both plugin clones had just finished in that run, so they may have caused the stall. A change of 90% or more fails at once: the control center changed 22–24% of the output at scale 1 and 50–67% at scale 1.5 in the runs so far, so a change that large means a bad baseline. After `panel-close`, it waits until fewer than 1% of the pixels differ from that capture. Exact equality would fail whenever the bar's clock ticks between the two captures.
- After each run, `harness run` lists processes whose command line names `TEST_DIR` and fails if any are left. It reports them and doesn't kill them, because they are outside the run's process groups and matching by command line is not proof of ownership.
- Noctalia binds its IPC socket before it listens (`IpcService::start`), so a `status` sent as soon as the socket exists can be refused. The supervisor retries the first `status` until its startup deadline and reports the last failure if none succeeds.

## 2026-10-08: supervised input and recovery evidence

- No dependencies or lockfiles changed. The sitting reuses `Session`, the bounded runner, keyboard log offsets and pointer output binding. Human confirmation files are separate from observed physical events. Only an explicit sitting mode extends deadlines; automatic checks retain their original path.
- Keep wtype 0.4: C6 observed broadcasts to an unfocused keyboard and restoration of its original keymap format and size after physical `x`. The stdin gate, 100-character cap and three-second child deadline stay. wtype's 100-character batching is in `main.c:367–418`; its key and modifier requests are in `main.c:318–342`. Keymap identity here means format and size, as C6 specifies, not a content hash.
- C7 matched recovery by the original logged keycode. Fresh `wtype -p a` cleared the one-symbol synthetic code 9; physical `a` used code 38 and did not release code 9. Physical Shift cleared the modifier. wev's key logger (`wev.c:383–401`) does not track held keys or client repeat, so these event results do not prove application behavior. wtype chooses its own keycodes; a fresh one-symbol release is not evidence of generic recovery for arbitrary multi-symbol calls.
- C8 independently verified fresh-device release and physical click after observed press then SIGKILL. The later human-only `recover` should send a fresh virtual-pointer release for the interrupted button. It should retain manual modifier/click checks and interactive confirmation, warn about ordinary-key limitations, and keep the dirty marker when the human cannot confirm the application's state. Only Shift and the left button were physically verified here.
- C9's physical chord fired the nested bind while wtype's chord did not. Smithay sends virtual keys directly to focused keyboards (`src/wayland/virtual_keyboard/virtual_keyboard_handle.rs:104–120` at `ff5fa7df`) and performs no release on destruction (`:160–173`). niri's virtual-pointer destruction hook is the trait's empty default (`src/protocols/virtual_pointer.rs:278–280` at `v26.04`), which niri's `State` doesn't override (`src/handlers/mod.rs:666–690`); `destroyed` only drops the resource from the manager's set (`virtual_pointer.rs:530–544`). C8 supplies the runtime release result.
- C14(a) returned `no` for session 3. The optional physical lock/unlock check was skipped, so no runtime claim is made about the locked transition.
- The first sitting stopped because its pointer check expected motion where wev logged enter. The sitting now reuses the automatic enter-or-motion helper. An explicit C8 restart mode preserves the completed keyboard evidence and prevents unnecessary repeated human checks.

## 2026-10-08: M0 outcome

- Input: wtype 0.4 for the keyboard, behind the stdin gate, with the 100-character cap and three-second deadline. The pointer stays a native `zwlr_virtual_pointer_v1` bound to an output (plan §4), which `vpointer` exercises.
- Screenshots: grim honours `-s` below 1 on niri, so `max_width` becomes a lower capture scale and the server needs no resize step. The default format is JPEG. The default `max_width` of 1280 stays provisional until M1's image delivery check.
- Recovery: send the interrupted button's release from a fresh virtual pointer, keep the manual checks and interactive confirmation, and keep the input-dirty marker when recovery is uncertain. The evidence and its limits are in `docs/results/m0.md`.

## 2026-10-08: niri-ipc's license

- `niri-ipc` 26.4.0 is GPL-3.0-or-later, like niri. The license gate allowed only permissive licenses, and it hadn't checked the harness, because cargo-deny skips workspace crates with `publish = false`. The gate first failed when the server started to depend on `niri-ipc`.
- The repository stays MIT. `deny.toml` allows GPL-3.0-or-later for `niri-ipc` alone; any other GPL crate still fails. MIT code can be combined into a GPL program, but a built binary includes `niri-ipc`, so a distributed binary follows GPL-3.0 terms. The README says so.
- Considered: relicensing the project to GPL-3.0-or-later, which changes little for binaries but gives up MIT for code that doesn't need GPL, and dropping `niri-ipc` for hand-written copies of niri's types, which would have to track niri by hand and are derived from GPL source anyway.

## 2026-10-08: MCP server dependencies

| Crate | Version | Published | Why |
|---|---|---|---|
| `rmcp` | 3.5.0 | 2026-09-28 | The official Rust MCP SDK. Default features (`server`, `macros`, `base64`) plus `transport-io` for stdio. 3.5.1 (2026-10-05) is too new. |
| `tokio` | 1.53.1 | 2026-07-20 | rmcp's runtime. Features `rt`, `macros`, `net` (niri's Unix socket), `time` (deadlines) and `io-util`. 1.53.2 (2026-10-03) is too new. |
| `serde` | 1.0.229 | 2026-07-18 | `derive` for the status report and error bodies. |
| `serde_json` | 1.0.151 | 2026-07-20 | Same version as the harness. |
| `niri-ipc` | `=26.4.0` | 2026-04-25 | Pinned to the installed niri, as the version rule's exception requires. |

- Every crate rmcp and tokio added to `Cargo.lock` was checked against the version rule. `cc` 1.6.0 (2026-10-03), `mio` 1.2.4 (2026-10-03) and `uuid` 1.27.0 (2026-10-02) were too new, so they are held at `cc` 1.5.1, `mio` 1.2.3 and `uuid` 1.26.1 with `cargo update --precise`. The other new crates were published on or before 2026-10-01.
- The runtime is Tokio's current-thread flavour. The server handles one client over stdio, and one thread keeps the order of its work easy to follow.
- rmcp's `#[tool_handler]` generates an `async fn list_tools` with no `.await`, which Clippy's `unused_async_trait_impl` rejects. The handler impl carries one `#[expect]` for it. It is the first lint exception the rmcp macros have needed.
- Each niri request uses a new connection, so a reply that arrives after its deadline can't be mistaken for the next reply. The harness keeps one connection instead, because it needs to reach the niri it identified.
- With tokio in the build but its `process` feature off, Clippy warns that the `tokio::process::Command::new` entry in `clippy.toml` doesn't resolve. The warning doesn't fail the gate and goes away when the server starts its first subprocess.

## 2026-10-08: niri event stream

- No new crate. tokio's `sync` feature adds the `watch` channel between the reader task and the tools; the version is unchanged (1.53.1).
- The server replays the stream with niri-ipc's `EventStreamState` instead of keeping its own copy of niri's state, so niri's own reducer decides what each event changes.
- The state counts as initialized when the workspaces, windows and overview events have arrived. niri sends its whole state as one burst on connect (`EventStreamState::replicate` in niri-ipc 26.4.0), but it marks no end of the burst. The overview event comes after the workspaces, windows and keyboard layouts, which are what `desktop_state` returns.
- One unparsable event reconnects; a second stops the stream until restart, as plan §7 says. The counter doesn't reset, so two bad events far apart also stop it. Ordinary disconnects reconnect after one second and don't count.
- Matching niri's `Event` in the server uses `matches!` for the three initialization events. Everything else goes to niri-ipc's reducer, so the server has no `match` on `Event` that a niri-ipc bump would need to extend.

## 2026-10-08: deterministic timing tests

- tokio's `test-util` feature is a dev-dependency feature only (same version, 1.53.1). Tests that depend on deadlines and reconnect delays run on Tokio's paused clock, which moves on only when every task is waiting, so their outcome doesn't depend on how fast the machine is.

## 2026-10-08: cancellation

- rmcp 3.5.0 runs each request in its own task and only cancels the request's token when the client sends a cancellation (`service.rs`, the request branch of the serve loop); it doesn't stop the task. Each tool takes rmcp's `RequestContext` and races its work against that token, so a cancelled call drops its niri connection or wait at once instead of at its deadline.

## 2026-10-08: screenshots, clipboard and the runner

| Crate | Version | Published | Why |
|---|---|---|---|
| `base64` | 0.23.1 | 2026-08-04 | Encodes screenshot images for MCP image content. rmcp already depends on the same version. |
| `rustix` | 1.1.5 | 2026-09-16 | Kills a child's process group on timeout or cancellation, without `unsafe`. Feature `process`. Same version as the harness. |

- tokio gains the `process` feature (same version). No new package entered `Cargo.lock`.
- grim truncates its image size (`render.c:145–146` at grim v1.5.0), where plan §8 says `round(logical × s)`. M0's C15 only used scales whose products are whole numbers, so it couldn't tell the two apart. The server expects truncation and nudges a lowered scale up to the next representable double until the width is exactly `max_width`; a unit test checks every width from 1281 to 3999 at four output scales.
- `max_width` defaults to 1280 image pixels (plan §4, provisional until M1's image delivery check), so an omitted `max_width` on a 2560-pixel output gives a 1280-pixel image. A capture keeps its full detail only when its logical width times the output's scale fits; a 1000-pixel region at scale 2 is still lowered to 1280. On DP-1 the default made JPEG capture take about 100 ms instead of 14 ms.
- Screenshot refs (plan §8) are not stored yet. Only the pointer tools use them, and they arrive with the input milestones; until then a stored ref would be dead code. The metadata returns everything a ref will hold.
- Arguments that don't fit the desktop (an unknown output, a region that crosses outputs, a malformed target) come back as a tool result with `isError` and plain text, not a new name in the stable error list. rmcp 3.5.0 already answers arguments that don't fit the schema that way (`handler/server/router/tool.rs`, `into_tool_argument_error`), so both kinds of argument mistake look the same to the model, which can then correct the call.
- `clipboard_read` runs `wl-paste --no-newline --type text`. wl-paste 2.3.0 exits 1 both for an empty clipboard and when nothing copied is text (`src/wl-paste.c:222–245`, `:265–268`); the tool returns those as `text: null` with `reason` `nothing_copied` or `no_text`. Text is capped at 1 MiB and must be UTF-8.
- The harness includes `src/image_header.rs` with `#[path]` instead of keeping its own copy. The server is a binary crate, so there is no library to depend on, and one header parser keeps the server and C15's checks in agreement.

## 2026-10-08: Noctalia detection and the lock state

- No new crate. The server's Noctalia client is async and sends one fixed payload, `/\x1estatus`, framed the way M0's `noctalia-socket` probe and Noctalia's own client frame it. Panel commands wait for the milestone that needs them.
- `shell_status` is removed from the tool router at startup when `noctalia` isn't on `PATH` (plan §6.1), using rmcp's `ToolRouter::remove_route`. An installed Noctalia that isn't answering gives `noctalia_unavailable`, a new name in the stable list, as plan §6.2 defines it.
- `status` reports Noctalia's failure detail as `noctalia_error` and logind's as `lock.logind_error`. Plan §6's shape allows added fields. niri's version and Noctalia's status are read concurrently, and the lock read follows because it may need Noctalia's reply, so `status` waits at most two deadlines, not three.
- `XDG_SESSION_ID` is passed to `loginctl` as an argument, so a value that isn't plain letters and digits is refused instead of risking it being read as an option.
- Locked wins between the two sources (review finding). niri sets logind's locked hint only on its own session (`src/niri.rs` at v26.04), so a server started from an SSH login, a TTY or an environment without the graphical session's ID reads a hint that never changes. Plan §9 ordered the sources but didn't say what to do when they disagree. Checking that the session is the graphical one is left for the action tools' lock gate.
- Whether Noctalia is installed is decided once at startup and used for both the tool list and `status`.

## 2026-10-08: the audit log

| Crate | Version | Published | Why |
|---|---|---|---|
| `chrono` | 0.4.45 | 2026-06-04 | RFC 3339 timestamps with milliseconds in UTC. rmcp already depends on this version with the `now` feature, which is the only one the server enables, so no package was added to `Cargo.lock`. |

- One line per call, opened in append mode and written with one `write_all`, so servers sharing the file don't interleave inside a line. The directory is created with mode `0700` and the file with `0600` (plan §12).
- The session label is the MCP client's `clientInfo.name` and the server's PID, as plan §10 defines it; `unknown` when the client sent no initialization.
- The outcome comes only from the result's error name, never from its content, so a result holding clipboard text or an image can't leak into the log. Outcomes beyond plan §6.2's names are `invalid_arguments`, `cancelled`, and `internal` when the tool couldn't build its result.
- A failed write doesn't fail the tool call, because the read-only tools have nothing to protect; `status` shows the last failure. The action tools may need a stricter rule.
- rmcp rejects arguments that don't fit a tool's schema before the tool runs, so those calls aren't logged.
- `screenshot`'s arguments are logged as given, `target` included, even when it names no output. They are the model's own identifiers, never desktop content, and serde_json escapes them.

## 2026-10-08: protocol tests

- The protocol tests write JSON-RPC by hand instead of using rmcp's client, so they see every byte the server writes to stdout and send each cancellation at the moment they choose. No crate was added.

## 2026-10-08: the MCP Inspector

| Tool | Version | Published | Why |
|---|---|---|---|
| `@modelcontextprotocol/inspector` | 2.9.0 | 2026-09-30 | The pinned Inspector for `make inspect` and `make inspect-check` (plan §13). 2.10.0 was published on 2026-10-07, too recently for the version rule. It runs through `npx`, so nothing is added to the repository. |

- The Inspector starts a stdio server with a minimal environment. `NIRI_SOCKET`, `XDG_RUNTIME_DIR`, `WAYLAND_DISPLAY` and `XDG_SESSION_ID` are not passed on, and `status` then reports `NIRI_SOCKET is not set`. `scripts/inspector.sh` passes those four on explicitly.
- The Inspector's `--strict` schema check flagged `screenshot`'s `max_width`, typed `["integer", "null"]`, as less portable: clients that map tool schemas onto a single-type dialect may reject it. The optional `screenshot` arguments are now described as their own types and left out of `required`. The server still accepts `null` for them.

## 2026-10-08: the name

- The project is `niri-computer-use`: the crate, the binary, the MCP server's name, the skill, the audit log's directory and the GitHub repository. "Computer use" is the term agents and their users look for, and the milestones after M1 add input. The server name is the same everywhere, so clients register it as `niri-computer-use`. `docs/results/m0.md` keeps the commands as they were run, under the old name `niri-desktop-mcp`.

## 2026-10-08: the docs site

| Package or action | Version | Published | Why |
|---|---|---|---|
| `astro` | 7.3.5 | 2026-09-24 | The site generator Starlight runs on (plan §21.3). 7.3.6 and 7.3.7 are too new for the version rule. |
| `@astrojs/starlight` | 0.42.5 | 2026-10-01 | Documentation theme with navigation and search. |
| `actions/setup-node` | v7.0.0 | 2026-07-14 | Node.js for the Pages build. v7.1.0 was published on 2026-10-08, so the major tag isn't used. |
| `actions/upload-pages-artifact` | v5 (5.0.0) | 2026-04-10 | Uploads the built site. |
| `actions/deploy-pages` | v5 (5.0.1) | 2026-09-01 | Deploys it to GitHub Pages. |

- `site/package-lock.json` was created with `npm install --before=2026-10-01T09:30:00Z`, so every transitive package also follows the version rule. Dependabot watches `site/` with the same seven-day cooldown.
- `npm audit` reports two advisories in the build tooling. `http-cache-semantics` 4.2.0, through Astro, can leak cached responses between users of a server's HTTP cache (GHSA-ch52-4w7c-c8xp); the static build serves nobody, and the fix, 4.3.0, came out on 2026-10-04. `postcss-selector-parser` 6.1.4, through Starlight's code-block styling, has quadratic parsing on crafted selectors (GHSA-rj75-hqrm-r3gf), and only our own CSS goes through it; no 6.x release fixes it. Neither is overridden.
- The site is plain Markdown pages in `site/src/content/docs/`, built to `site/dist/` and deployed by `.github/workflows/pages.yml` on pushes to `main` that touch `site/`.
- npm instead of the pnpm with `minimumReleaseAge` that plan §21.5 names: npm ships with Node.js, and the site has two direct dependencies. npm keeps no release-age setting, so a manual `npm install` or `npm update` in `site/` must pass `--before` with a date seven days back, as Cargo updates use `--precise`; Dependabot's cooldown covers routine bumps.
- `actions/setup-node` and `npm ci` instead of Astro's `withastro/action`, so CI and the deploy build the site with the same steps.
- CI builds the site on every push and pull request (`ci.yml`, job `site`), so a broken page fails before the deploy does. There is no link checker yet; the internal links were checked by hand against the build.
- The landing page has no demo. Plan §21.3 asks for a screenshot or recording of a real run, never a mock-up, and none has been taken for publication.

## 2026-10-08: the lease and the stop watcher

- No new crate. The lease lock is `std::fs::File::try_lock`, an `flock` on Linux, in the standard library since Rust 1.89. The stop watcher uses inotify through rustix's `fs` feature (same version, 1.1.5) and Tokio's `AsyncFd`, which the `net` feature already brings.
- The watcher doesn't interpret event names: any change in the runtime directory makes it check whether `stop` exists. That avoids reasoning about event order, renames and coalesced events.
- The flags fail closed (review finding): a runtime directory that can't be read counts as stopped for the watcher and refuses `acquire_desktop`, instead of reading as "no flag".
- `lease.json` is display only. The holder empties it before unlocking, and readers ignore a record whose PID has no `/proc` entry, so a crashed holder doesn't show up as holding the lease.
- Removing the runtime directory or the `lease` file under a holder (review finding) is caught by inode checks: the watcher compares the directory's path with the inode it watches, on every event and once a second, and the holder does the same for `lease`. A plain inotify watch isn't enough, because the kernel delays a directory's deletion event while a file in it, the held lease, is open. After the directory is lost the server refuses the lease until it restarts.
- The release task reads the latest flag on every change (level-triggered), so a resume followed quickly by a stop can't be missed, and `acquire_desktop` checks both the watcher's view and the file.
- `release_desktop` when not holding the lease succeeds with `released: false` rather than failing, so an agent whose lease the stop flag already took back isn't told it made a mistake.
- Whether `acquire_desktop` also refuses while the screen is locked is decided with the gate in a later step; for now only the stop flag and the input-dirty marker refuse it.

## 2026-10-08: the input-dirty marker and `recover`

- No new crate. `/proc` is read directly: the start time (field 22 of `stat`, counted from the last `)` because the command name may contain one) pins a PID to one process, and `comm` plus the real UID find the user's `wtype` processes.
- Only the reader of the marker exists in M2. The input tools that write it arrive in M3 and M4; until then the tests write it as a fixture. A marker that can't be parsed blocks like any other, so a malformed file can never unblock input.
- Plan §11 step 3 has `recover` send the release of a pressed pointer button from a fresh virtual pointer. The pointer module arrives in M4, so this version asks the human to press and release the buttons the marker names instead, and the manual check already asks for a click. The automatic release is added with the pointer.
- The plan's M2 exit lists `recover` against a live owner and with a marker naming a delayed-exit child as nested tests. Neither touches niri, so they run as protocol tests against the binary with a fixture runtime directory: the child is a `sh` that ignores SIGTERM in a process group of its own, and a marker whose start time doesn't match proves a reused PID is never killed. The `pending` path, which scans the user's real `/proc` for `wtype`, is unit-tested against a fake `/proc` whose entries link to processes the test started, so no test can end a process it doesn't own.
- `recover` pins every process it may kill by PID and start time when it lists it, and kills it only if it is still that process; a process that is already gone counts as ended (review finding). A child that doesn't lead its own process group is ended alone rather than by group.

## 2026-10-08: the lock state follows niri's session

- Plan §9 asked for a check that `XDG_SESSION_ID` names the graphical session before the lock gate trusts logind. On this machine the session in `XDG_SESSION_ID` is of type `tty` and niri runs as a systemd user service outside any session scope, so checks on the session's type or on niri's cgroup would reject the normal setup. niri v26.04 sets `LockedHint` on the session in its own `XDG_SESSION_ID` (`update_locked_hint`, `src/niri.rs`). The server now asks logind about exactly that session: niri's PID comes from the socket's peer credentials, and its `XDG_SESSION_ID` from `/proc/<pid>/environ`, readable because niri runs as the same user. The server's own `XDG_SESSION_ID` is no longer read.
- The protocol tests run the fake niri for these cases as a process of its own, the test binary started with `--ignored --exact`, so its environment is exactly what the test sets; the in-process fake would show the test runner's environment, which differs between machines and CI.

## 2026-10-08: the policy file and the lease decision

| Crate | Version | Published | Why |
|---|---|---|---|
| `toml` | 1.1.6 | 2026-09-10 | Parses the policy file (plan §9). Features `std`, `serde` and `parse` only. 1.1.7 was published on 2026-10-08, too new for the version rule. |

- `toml` 1.1.6 pulled in `serde_spanned` 1.1.2, `toml_datetime` 1.1.2, `toml_parser` 1.1.4 and `toml_writer` 1.1.3, all published on 2026-10-08. They are held at `serde_spanned` 1.1.1 (2026-03-31), `toml_datetime` 1.1.1 (2026-03-31), `toml_parser` 1.1.3 (2026-07-27) and `toml_writer` 1.1.2 (2026-07-14) with `cargo update --precise`. `winnow` 1.0.4 (2026-07-13) is old enough.
- Plan §9 refuses presets whose `argv[0]` is "a known shell or terminal with `-e`/`-c`". The rule here is wider: any program that runs a command it is given (shells, interpreters, `env`, `sudo`, `doas`, `pkexec`, `su`, `run0`, `setsid`, `nohup`, `systemd-run`, `timeout`, `xargs`, `nice`) is refused, and a terminal is allowed only without arguments, because terminals such as `kitty` run trailing arguments as a command without any flag. The lists match the program's file name, so a path doesn't get around them, and a trailing version is removed before matching, so `python3.13` is `python3`. `busybox`, `toybox`, `uwsm`, `distrobox` and `toolbox` are on the list, and `flatpak` is refused only with `--command`, because `flatpak run <app>` is the usual way to start a Flatpak app (review finding). The rules are a guardrail, not a boundary: a wrapper script or a renamed link gets past any list.
- An unresolvable config directory (no `XDG_CONFIG_HOME` and no `HOME`) makes the policy `invalid` rather than `missing`: a file might exist that the server can't find.
- `acquire_desktop` now refuses while the screen is locked and while the server is read-only, so an agent learns at once that it can't act, instead of at its first action. An unknown lock state is allowed, as plan §9 says. The app deny list is loaded and counted, and enforced once the input tools exist.

## 2026-10-08: an unknown lock state refuses the lease

- Plan §9 reported `unknown` and allowed it. Since the lock state follows niri's own session, `unknown` has more causes: niri's environment can't be read, something else serves the socket, or niri wasn't started from a session, each without Noctalia running. The user chose to refuse: `acquire_desktop` returns `screen_locked` with a detail that says the state is unknown, so input only ever goes to a screen that a source niri actually updates says is unlocked. One case still reads a confident wrong answer: a plain `niri`, not `niri --session`, started inside a logind session inherits `XDG_SESSION_ID` but never sets the hint, so logind keeps saying `no`; reading `--session` from niri's command line closes it, and is planned before input arrives in M4. A desktop without logind's hint and without Noctalia can't be controlled until one of them answers.

## 2026-10-08: M2's nested acceptance

- The M2 exit criteria run in `make nested-control`: two servers competing for the lease, stop and resume, `recover` against a live owner and with a marker naming a delayed-exit child. The protocol tests check the same behaviour against fakes; the nested run adds a real niri (its version, its peer credentials), a real Noctalia as the lock source, and the stop sent through niri's `spawn` action, the path the stop keybind takes.
- The nested run starts Noctalia because the lease now needs an unlocked answer: a nested niri isn't a session instance and sets no logind hint.
- The harness has no interactive MCP client. Each server reads a `printf` of its requests followed by a `sleep` that keeps stdin open, so every process stays under the harness runner's deadlines and process groups.

## 2026-10-08: logind only for `niri --session`

- niri v26.04 sets logind's `LockedHint` only when it runs as the session instance (`update_locked_hint` returns early unless `is_session_instance`, `src/niri.rs`), which `niri --session` turns on. A plain `niri` started inside a logind session inherits `XDG_SESSION_ID`, so logind kept answering `no` on a locked screen. The server now reads niri's `/proc/<pid>/cmdline` and asks logind only when `--session` is among the arguments; otherwise logind counts as not answering, Noctalia decides, and without Noctalia the state is `unknown`, which refuses the lease.
- The protocol tests' fake niri is the test binary started with `--ignored --exact`. libtest rejects unknown options, so `--session` goes after `--`, where libtest reads it as one more test-name filter that matches no test. No separate fake-niri binary was needed.

## 2026-10-08: the niri action tools

- No new crate. Waiters use Tokio's `broadcast` channel, from the `sync` feature already enabled.
- Every action runs through one gate that holds the action mutex for the whole action (plan §9, §10). Its order is the stop flag, the input-dirty marker, the lease, then the same `refuse_control` as `acquire_desktop` (niri unreachable, `read_only`, `screen_locked`). Stop and recovery come before `lease_required`: a stop also takes the lease away, and an agent told `lease_required` would call `acquire_desktop` only to be told `stopped`. The readiness report behind `refuse_control` costs a niri request and a `loginctl` run, so it is gathered only after the cheap checks pass, and it is gathered again for every action, because the lock state can change while the lease is held.
- A stop during an action cancels it (plan §11: cancel, clean up, release). The gate races the action against the stop watcher; the watcher's task waits for the action mutex before it takes the lease back, so the lease is released only after the action has been dropped. The cancelled call returns `stopped`, whose detail says that what niri already accepted may have taken effect, because the server doesn't track whether the request was sent. A watcher that ends during the action, because the runtime directory was removed, cancels it too, but the call says `upstream_error` and to restart the server, as `acquire_desktop` does then (review finding). Cancelling the MCP request drops the action and keeps the lease: only the user's stop or `release_desktop` gives it up. `release_desktop` waits for a running action, at worst about fifteen seconds.
- Waiters apply every event themselves instead of reading the replica when it changes. A Tokio `watch` keeps only the latest value, so focus passing through another window between two reads would be missed, and `interrupted` would depend on timing. The stream task numbers each event and broadcasts it after the replica applies it; a waiter subscribes before copying the replica's windows and workspaces and skips events the copy already holds. niri-ipc's `EventStreamState` isn't `Clone`, but its window and workspace parts have public fields, so the waiter copies those two and applies events with niri-ipc's own reducer. Connections are numbered too, and the end of a connection carries its number, so a waiter whose copy came from a newer connection ignores an older one's end instead of reporting `uncertain` (review finding).
- A lost reply is told apart from a refused request. niri reads one request line and answers after carrying the action out (`src/ipc/server.rs` at v26.04), so a failed connect or write, and niri's error reply, mean nothing happened, and the call fails with that error. A reply lost after the whole request was written is the outcome `uncertain` with `accepted: null`, never an error and never retried (plan §6.2).
- An unknown window or workspace id is an argument mistake, checked against the waiter's state before anything is sent. niri silently ignores actions on ids it doesn't know (`do_action`, `src/input/mod.rs` at v26.04), which would otherwise show up only as a timeout.
- `focus_workspace` takes a workspace id from `desktop_state`, not the plan's `ref`. niri's index references count per output and its names are optional, so an id is the one reference that always names one workspace.
- `interrupted` applies to the focus tools and to `launch`, whose wait is the longest. For `launch`, only focus moving to a window that was already open counts, because a launched app may map and take focus before it sets its `app_id`. A window that was already open and has the preset's `app_id` doesn't count either: a single-instance app hands a second start to its running process, which focuses the window it has, so `launch` reports `focused` with that window rather than an intruder (review finding). `close_window` doesn't use it: focus moves away from a closing window by design. Focus leaving every window, for example to a shell panel, keeps waiting.
- `launch` keeps counting matching windows for half a second after the first appears, so an app that opens two windows reports `ambiguous` instead of `one`. The whole call can take up to five and a half seconds.
- A focus target that already has focus is reported as `focused` with `accepted: false`, and nothing is sent (plan §7 step 3). With `workspace-auto-back-and-forth` set, niri answers `FocusWorkspace` on the focused workspace by switching to the previous one (`switch_workspace_auto_back_and_forth`, `src/layout/monitor.rs` at v26.04), so sending it would move the agent away while the check already said `focused` (review finding). `focus_window` follows the same rule, so the two read alike.
- `close_window` is marked destructive and not idempotent: an app may answer a second close differently, for example while its unsaved-changes dialog is open. The focus tools are idempotent; `launch` isn't.
- `status.policy.preset_names` lists the preset names, because `launch` takes a name and an agent has no other way to learn them. The field is added beside the existing ones, as plan §6 allows.
- The audit log reads `accepted` and `observed` only from action tools' successful results, so a read-only tool whose content happens to have such a field, such as Noctalia's status, never fills them.
- The protocol tests run the actions against the in-process fake niri, which answers actions `Handled` or holds them, and hands each one to the test; the test then sends the events niri would. The lock state comes from a fake Noctalia whose reply the test can switch, since the in-process fake niri doesn't run as `niri --session`.

## 2026-10-08: a screenshot with outcomes in doubt

- Plan §6 attaches a fresh screenshot to an action result whose observation is a timeout, `pending`, `uncertain` or `interrupted`. `launch`'s `none` is a timeout under another name, so it gets one too. `one`, `ambiguous`, `focused` and `closed` don't.
- It is taken after the stop race but before the action mutex is released: a stop during the capture no longer throws away the outcome the server already observed, such as `pending` (review finding), and no other action of this server can change the desktop between the observation and the picture. A stop during the capture waits for it, at most grim's five seconds, before the lease is given back. It is the focused output at the default 1280 pixels wide, as JPEG, through the same code as the `screenshot` tool.
- The image comes after the outcome's text in the result's content, so a client that reads only the first block still gets the outcome. Its metadata is the `screenshot` field. A failed capture adds `screenshot_error` with the error's name and detail and leaves the outcome as it is; an output niri can't name as focused counts as an `upstream_error` there.
- The `screenshot_ref` that plan §6 mentions arrives with the pointer tools in M4, like the ref store.

## 2026-10-08: M3's nested acceptance

| Crate | Version | Published | Why |
|---|---|---|---|
| `wayland-client` | 0.31.15 | 2026-07-22 | The harness's fixture app, `harness window`, connects to the nested niri and maps toplevels. Same version the `vpointer` probe uses. |
| `wayland-protocols` | 0.32.13 | 2026-06-19 | The `xdg_wm_base` bindings for those toplevels. Feature `client` only. |

- `rustix` (same version, 1.1.5) gains the harness features `fs`, for the fixture's `memfd` buffer, and `event`, for polling its connection with a deadline.
- The crates the two pull into `Cargo.lock` are the ones already vetted for the probe, each the newest release at least 7 days old that its dependents allow, rechecked on crates.io today: `wayland-backend` 0.3.17 (2026-08-14), `wayland-scanner` 0.31.11 (2026-07-22), `wayland-sys` 0.31.11 (2026-03-31), `smallvec` 1.16.2 (2026-09-25), `pkg-config` 0.3.34 (2026-08-14), `quick-xml` 0.41.0 (2026-06-29) and `downcast-rs` 1.2.1 (2024-04-07). `quick-xml` 0.42.0 and `downcast-rs` 2.x are outside the ranges `wayland-scanner` and `wayland-backend` accept. They are dev-only: the server binary doesn't depend on them.
- A fixture app of our own instead of installed apps: the checks need an `app_id` set after mapping, two windows from one start, a window that ignores close requests, and a start delayed long enough to move focus during the wait. No app on the machine does all of these, and none would be reproducible elsewhere. The fixture's buffer is never written, so it needs no `mmap`.
- The fixture is the harness binary itself, started by niri from presets whose program is its absolute path. The policy rules allow it: `harness` is neither a command runner nor a terminal. It checks its environment is the nested one before connecting, exits when niri goes away, and has its own 90-second deadline, so the run's leftover check stays clean.
- The harness talks MCP to one long-running server instead of the one-shot `printf` pipelines of M2's checks, because the lease has to stay with one server across many calls and the `interrupted` check moves focus while a call is in flight. The runner's `Process::send` writes to a held stdin without closing it, within the process's deadline, and replies are read back from the server's log file.


## 2026-10-08: the pointer tools

| Crate | Version | Published | Why |
|---|---|---|---|
| `wayland-client` | 0.31.15 | 2026-07-22 | The virtual pointer's Wayland connection to niri. The harness already uses it; newest release, rechecked on crates.io today. |
| `wayland-protocols-wlr` | 0.3.12 | 2026-03-31 | The `zwlr_virtual_pointer_v1` bindings, as in the `vpointer` probe. Feature `client` only. Newest release. |

- The only new crate in `Cargo.lock` is `wayland-protocols-wlr`; everything it pulls in was vetted for the harness. `rustix` gains the `time` feature for the monotonic clock, and Tokio's `net` feature, already on, provides `AsyncFd`.
- The pointer lives under `niri/`: its Wayland connection talks to niri too. It connects to the display itself rather than through `connect_to_env`, which reads the process environment that `main` already read once, and it checks the socket's peer PID against the one serving `NIRI_SOCKET`, so a stale or foreign `WAYLAND_DISPLAY` can't receive input meant for this niri.
- The pointer is async: `AsyncFd` watches a copy of the connection's socket, and each round trip uses `prepare_read`, a readiness wait with a deadline, and `dispatch_pending`. A blocking round trip would stall the single-threaded server, including the stop watcher.
- One pointer per gesture, created after every check passed and destroyed at its end. A long-lived device would need its own cleanup on release, stop and output changes, and buys nothing at one action per observation.
- `observed` for input is `sent`: niri handled the input (a `wl_display.sync` round trip after the last step), and nothing more is claimed. Plan §6 called this `verified: false`; an outcome name keeps the audit log's `observed` field meaningful without a second field. A failure after the first step is `uncertain` with `accepted: null`, as for a lost niri reply.
- Only `click` and `drag` write the input-dirty marker. A motion or a wheel turn leaves nothing held, and the marker would block every server for nothing if the server died between writing and removing it. The pointer's marker stays `pending`: there is no child process for `running` to name, and its `buttons` field says what to release.
- Plan §6 lists refusals; the order here is: arguments, unknown ref, the deny list, untested outputs, then the ref against the outputs just requested (expired, reconnected stream, changed output, out of bounds). A disconnect of the event stream drops every ref (plan §7); the ref keeps the connection number from capture and is refused as `unknown_ref` when it differs, instead of a store cleared from the stream task.
- `click` takes `count` from 1 to 3 with no pause between clicks, so a double click arrives within any app's double-click time. `drag` waits 50 ms before and after the press and moves in ten steps 20 ms apart, so toolkits that start a drag after a motion threshold see one. `scroll` takes at most 10 notches per axis per call, so a mistaken argument can't scroll a page away.
- The live setups (plan §8) are both enabled in code: the nested `winit` output, which the nested acceptance tests, and one monitor at `Normal`, which the supervised real-session run on DP-1 passed on 2026-10-09 (`docs/results/m4.md`).
- `recover` sends the release of the marker's buttons from a fresh virtual pointer (plan §11 step 3), bound to niri's first enabled output: a release needs no position. C8 verified this for the left button (272); the right and middle buttons are released the same way but remain runtime-unverified. A marker with buttons and no child skips the `wtype` scan, which only fits a keyboard marker. If the release can't be sent, the human is asked to press and release the buttons, as before, and the confirmation is still required either way.

## 2026-10-09: the keyboard tools

- No new crate. `runner::gated` starts a child with a held stdin pipe; `wtype` is the only user.
- `expect` is required and takes `{"window_id": …}`, `{"app_id": …}` or the string `"none"`, plan §6's `{window_id}`, `{app_id}` and `{none}` written as JSON an agent can't leave out by accident.
- `combo` is modifiers and one keysym name joined by `+`. The key must be letters, digits and `_`, which every XKB keysym name is, so a symbol like `/` is spelled `slash` and nothing can be read as a wtype option. `super`, `logo` and `win` all mean wtype's `logo`.
- The wtype call runs in a spawned task that owns the child and the marker. Plan §11 lets a running wtype finish within its deadline rather than killing it on a stop; the action gate drops the tool's work on a stop or cancel, so the child can't live in that work. The task removes the marker when wtype exits by itself, whatever its exit code: wtype checks its arguments and connects before it types, and it releases each key it presses. A wtype ended by a signal, or killed at the deadline, leaves the marker.
- If wtype's PID or start time can't be recorded, wtype is killed while it still waits at the gate and the marker is removed: nothing could have been typed.
- `interrupted` for the keyboard tools means focus left the window that had it at any event between the check and wtype's exit, as the plan's before-and-after comparison, but without missing a focus change that came back.
- `observed` is `sent`, as for the pointer tools (2026-10-08), plus `focus`: `matched` or `unchecked`.
- `key` and `type_text` carry `destructiveHint`, like `click` and `drag`: a keystroke can delete or send anything. Plan §6 names only `close_window`; the hint tells clients the truth about input.

## 2026-10-09: M4 pointer review

- A pointer failure before any step reached niri's socket is now an error, not `uncertain`: nothing can have happened (review finding).
- A marker that can't be removed after a gesture niri handled is an `upstream_error` saying so, not `uncertain` (review finding).
- A gesture dropped midway sends its releases, then a spawned task waits for niri's `wl_display.sync` reply before removing the marker. `Drop` can't wait, and a flush only puts the release in the socket buffer (review finding). Until the task finishes, other servers see `recovery_required` for a moment, which errs on the safe side.
- The pointer marker records its output, and `recover` binds its fresh pointer there when that output is still enabled (review finding). niri ignores the bound output for a button, so this only makes the record exact.
- No protocol test drives a successful gesture: the fake niri has no Wayland display, and a fake Wayland compositor would test our own fake. The nested run checks the marker after each click and drag, and the stop checks cover the release on drop.
- `click`, `drag`, `key` and `type_text` carry `destructiveHint`; plan §6 names only `close_window`, but a click or a keystroke can delete or send anything.

## 2026-10-09: the `vpointer` probe is gone

- Plan §14 deletes the probes once real code replaces them, by the end of M4. The server's pointer replaces `vpointer`: its mapping is `coords.rs`, and `make nested-input` checks accuracy, clicks, a drag and the wheel's frames through the server, at scale 1 and 1.5. `make nested` no longer runs C4, the probe's click and C12; their M0 results stay in `docs/results/m0.md`.
- C8, the interrupted pointer in `make sitting`, needed the probe's `hold`, so the sitting runs C6, C7 and C9, and `--sitting-from-c8`, which only resumed C8 and C9, is gone. C8's automatic half, a button left down by a killed pointer and cleared by a fresh pointer's release, now runs against the real server in `make nested-input`'s crash check; its human half was recorded in M0.
- `noctalia-socket` stays until M5: it drives C13's panel commands, and the server's replacement for those, `shell_open` and `shell_close`, arrives in M5. A plan correction (revision 19) moves its deletion there.
- The C10 corpus is a test fixture, not a probe; it moves to `harness/corpus.txt`.

## 2026-10-09: M4 keyboard review

- wtype runs with `LC_ALL=C.UTF-8`. It decodes stdin with `setlocale(LC_CTYPE, "")` and `mbstowcs`, and in a non-UTF-8 locale it stops at the first byte above ASCII, types what it had and exits 0, so a server started with a trimmed environment would report `sent` for half a text (review finding). glibc has `C.UTF-8` built in.
- A broken pipe on wtype's stdin means wtype exited before reading, which with `-` first means before typing anything; the runner ignores that write error and reports wtype's exit status and stderr, and the marker comes off (review finding).
- A gesture that failed before its press, and was dropped, removes its marker at once instead of waiting for a sync on a connection that may be gone (review finding).
- When the event stream is lost after input was sent, the result is `uncertain` with `accepted: true`, as for the action tools, rather than `sent` with a stale focus (review finding).
- The protocol tests drive the keyboard path through a fake `wtype` on the fixture's `PATH`: its arguments, its stdin and locale, the marker in its `running` phase while it runs, and the marker kept after a kill or the deadline (review finding).
- A pixel inside the image that maps off the output stays `out_of_bounds`, with the distance in the detail, rather than a reason of its own: `observe` only captures rectangles inside one output, so it can't happen today, and the agent's remedy is the same.

## 2026-10-09: the shell tools

- No new crate. `shell_open` and `shell_close` use the server's Noctalia client from M1, which now sends three kinds of command: `status`, `panel-open <id>` and `panel-close <id>`.
- The panel allowlist is `policy::Panel`, an enum of the three panels plan §6.1 allows. Plan §6.1 asks the payload builder to reject `\x1e` and newlines; with commands built only from the enum's fixed words there is nothing to reject, so a unit test checks every payload instead.
- `panel` is a plain string in the schema, so a refused panel reaches the server and comes back as `panel_not_allowed`, the stable name plan §6.2 promises, rather than as a schema mistake. The check runs after the action gate, as `launch` checks its preset.
- `Unanswered`, the refused-or-lost split for niri requests, moves to `error.rs` and also describes a panel command: a failed connect or Noctalia's `error:` reply is an error, and a reply lost after the connect is `uncertain` with `accepted: null`, because Noctalia carries the command out before it replies (`PanelManager::registerIpc` in 5.2.1).
- The tools read `status` before sending, which is plan §6.1's "checked before every Noctalia-backed call", and send nothing when the panel is already open, or already not open, reporting `accepted: false` as the focus tools do.
- `opened` is a new `observed` value; `closed` now also means a panel closed. `shell_close` counts another panel being open as closed, since `activePanelId` names only one.
- Results carry `shell.active_panel`, absent when it is unknown: after a lost reply, or a timeout before any `status` read answered, and `focused_window` from niri's event stream, which an open panel leaves null.
- They have no `interrupted`: Noctalia sends no events, and plan §6 defines `interrupted` by window focus.
- They take the lock gate like every action, before Noctalia is asked. Where only Noctalia can say whether the screen is locked, as in the nested session, a stopped Noctalia makes the lock state unknown, so they answer `screen_locked`, and `shell_status` answers `noctalia_unavailable`.

## 2026-10-09: C13 through the server, and M5's nested acceptance

- `make nested NOCTALIA=1` drives C13 through `shell_status`, `shell_open` and `shell_close`, so the `noctalia-socket` probe is deleted, and with it `probes/` (plan revision 19). C13's rule, `activePanelId` seen within two seconds of each command, is the server's observation plus a check that each call returned within two seconds; the server's own wait starts only after Noctalia's reply. The capture checks stay in the harness. Before each panel call the harness still checks that Noctalia's socket resolves under `TEST_DIR/run`, which it used to check before handing the socket to the probe.
- The nested Noctalia's wallpaper directory is an empty `TEST_DIR/data/wallpapers`. With the default, the wallpaper panel lists the pictures directory of the host user, whose `HOME` the nested session keeps, and its screenshot would land in the run's artifacts.
- `make nested-shell` is M5's acceptance: the three panels, the refused ones, the Noctalia lock source, Noctalia stopped and Noctalia absent from `PATH`. The absent case runs a second server under `env PATH=<empty directory>`, since `PATH` decides the tool list.
- The tray drawer gets no pixel check: with no tray items it is one icon wide (`TrayDrawerPanel::preferredWidth` in 5.2.1), under the 1% a drawn panel must change, while the bar's clock alone can change a few hundred pixels.
- The OCR helper looks for `Home`, the control center's title on open (`control-center.tabs.home`, `control_center_panel.cpp`), in English, which is the `LANG` the harness keeps from the host here. The first runs skipped it because tesseract wasn't installed; with tesseract 5.5.3 it found the word (`docs/results/m5.md`).

## 2026-10-09: agent guidance and skill evals

- A session transcript showed an agent breaking rules the skill stated: screenshots sent alongside actions, text split by hand with Enter pressed after a failed part, apps started by typing into a terminal, focus left on the wrong window. The rules an agent needs at the moment of the call now sit in the tool descriptions and the server's `instructions`, which every client shows; the skill keeps the workflow and the reasons, and its tables move to `references/`.
- `type_text` takes up to 1000 characters and types them in parts of 100, one `wtype` call each, checking focus between parts. Splitting is the server's job because the agent got it wrong; parts keep each `wtype` well inside its three-second deadline. 1000 bounds one call to about thirty seconds. A result that stops early carries `typed`, and a failed part says in its detail how much went out, so an agent can tell a half-typed message from a whole one.
- `screenshot` waits for the action mutex before capturing. A screenshot sent in parallel with an action otherwise showed the screen before it, and agents read it as the action's effect. Evidence screenshots are taken inside the action and don't wait.
- `release_desktop` doesn't put focus back by itself. Many tasks end with another window in front on purpose, such as "open the notes for me", and the server can't tell which. The tool description and the skill tell the agent to do it with `focus_window`.
- `make nested-eval` runs `claude -p` inside the nested session against one scenario, with only the nested server (`--strict-mcp-config`), only the `Skill` and `Read` tools, and only project settings, so the agent has no shell and no host MCP servers. The skill under test is copied into the run's `TEST_DIR/agent/.claude/skills/`. Grading reads the nested server's audit log, which has every call's timing, outcome and `text_len`, and the scenario's fixture logs. The one write outside `TEST_DIR` is Claude Code's own transcript under `~/.claude/projects/`, which `claude -p` always writes; the run keeps its own copy of the stream as `transcript.jsonl`.
- The scenarios come from failures seen in a real session's transcript: a long message sent with one Enter, four keys with a screenshot after each, a click in another window with focus returned, a stopped server, and an app without a preset. Results go in `grading.json` and `timing.json` in the skill-creator's format, so its benchmark and viewer can compare skill versions.

## 2026-10-09: fewer turns per task

- About 98% of an eval run's wall time is the agent's own turns; the server's calls take milliseconds. So speed comes from fewer calls per task, not faster calls.
- Every action takes `screenshot: true`, which returns a screenshot of the focused output with the result. It is taken inside the action, under the action mutex, so no other action can start before it. It waits for the screen to stop changing: a first look after 50 ms, then a capture at least 100 ms after the previous one until two are the same bytes, for at most 1.5 seconds. 100 ms is several frames at 60 Hz, so two identical captures mean the app stopped redrawing rather than caught between frames; 1.5 seconds bounds a screen that never stops, such as a video, and `settled: false` says so. Comparing bytes needs no image decoding, and grim encodes the same pixels to the same bytes.
- `type_text` takes `submit: true`, which presses `Return` only once the whole text went out, and reports `submitted`. Pressing Enter after a call that stopped early was the worst mistake seen in transcripts, and with `submit` the server makes that call.
- `key` takes `keys`, a list of up to 16 combinations, pressed one `wtype` call each with the same focus check between them as `type_text`'s parts. Every combination is parsed before the first is pressed, so a typo presses nothing. 16 covers menu walks and form tabbing without letting one call run long.
- `wait_for` replaces polling with screenshots: a window appearing, closing or changing its title, from niri's event stream, or the screen stopping changing. It reads only, so it needs no lease and doesn't take the action mutex, except that `screen_stable` waits for a running action first. Up to 30 seconds, default 10: long enough for an app to start, short enough that the agent stays in control. Titles can hold private text, so the audit log keeps only their length.
- `acquire_desktop` keeps the window the user was on, and `release_desktop` takes a required `restore_focus`. It replaces the earlier rule that the agent puts focus back with `focus_window` itself, which needed the agent to remember the window across the whole task and was the step most often missed. The argument is required, not defaulted, because whether to leave another window in front is the task's decision and the agent must make it each time. Restoring runs through the action gate, so a stop or a lock still refuses it, and the lease is given up whatever the outcome.
- The evals get harder: `dialog-midway` opens a window that takes focus as soon as the agent takes the lease, before its first action, and `errand` chains a launch, a message, a close and the return to the user's window. Every scenario also checks, from the agent's transcript, that no action was sent in the same turn as another desktop call, which the audit log alone can't see when the server ran them one after the other. `timing.json` adds the number of calls and of agent turns.
