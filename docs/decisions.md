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
- The harness generates a Noctalia config instead of using the defaults. Without a setup marker in its fresh state directory, Noctalia opens its setup wizard as a panel on startup (`application_ui.cpp`), so `activePanelId` wouldn't start as null. Weather is off so a test run makes no weather requests. `noctalia config validate` exits 0 even when it warns about an unknown key, so the harness requires its plain success line.
- Noctalia starts after the `wev` checks. Its bar reserves 34 logical pixels at the top of the output, and `wev`'s position is relative to the top-left of the working area.
- C13's pass rule only reads `status`. The supervisor also compares captures of the output, because a screenshot taken right after `status` reports the panel open showed no panel: the panel was not drawn yet. Noctalia also answers `status` before it has drawn its wallpaper, and a capture taken then is the magenta background: in one review run the wallpaper alone made the panel look drawn and never gone. So the supervisor first waits until less than half the output is magenta and two consecutive captures differ in fewer than 1% of their pixels, and keeps that capture. It waits until at least 1% of the pixels differ from it after `panel-open` and saves the screenshot then. These capture waits allow five seconds rather than C13's two, because Noctalia can report the panel open long before it draws it: in one run (1791393079-462290) it logged `queued surface rendering took 1935.1ms`, and the panel wasn't drawn within two seconds. A change of 90% or more fails at once: the control center covered about 23% of the output at scale 1 and 53% at scale 1.5, so a change that large means a bad baseline. After `panel-close`, it waits until fewer than 1% of the pixels differ from that capture. Exact equality would fail whenever the bar's clock ticks between the two captures.
- Noctalia binds its IPC socket before it listens (`IpcService::start`), so a `status` sent as soon as the socket exists can be refused. The supervisor retries the first `status` until its startup deadline and reports the last failure if none succeeds.
