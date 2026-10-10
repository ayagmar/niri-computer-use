# Shared engine results

What the shared engine costs and saves, measured on branch `shared-engine`, against `main` at `8db53b1` before it.

Environment: Rust 1.99.0; niri 26.04 (8ed0da4); Noctalia 5.2.1; AMD Ryzen 9 9950X3D, 32 threads. Release builds. Every run below ran in the private headless cage, with one 1280x720 `winit` output at scale 1. No run sent input to the host or viewed host pixels, clipboard or window titles. Every run reported C1's host snapshot unchanged, no entries from its processes in the host's journal, and no leftover processes. The runs were started with `nice -n 19`, so the timings are those of a low-priority process on a desktop in use.

## Commands

`before` is `main` at `8db53b1`, built with `cargo build --release --locked` in a detached worktree and measured with the branch's harness through `SERVER=`:

```sh
make nested-measure SHARED=1
make nested-measure
make nested-measure SERVER=<worktree>/target/release/niri-computer-use
```

Each writes `measure.json` to `target/e2e/<run>/`; [development.md](../development.md#nested-shared-engine-checks) says what it measures. Memory is the median of three samples a second apart, five seconds after the clients' last call. PSS counts a page shared by several processes once in all, so its totals are what the processes cost together; RSS counts it in each, so RSS totals overstate.

## Memory and processes

Each client called `status`, `desktop_state` and `screenshot`, then idled. Totals over every process the clients caused: servers or bridges, the engine and the guardians.

| Clients | Build and mode | Processes | PSS total | RSS total | Open files |
|---|---|---|---|---|---|
| 1 | before | 2 | 6.8 MiB | 13.3 MiB | 26 |
| 1 | after, standalone | 2 | 7.1 MiB | 13.7 MiB | 26 |
| 1 | after, shared | 3 | 7.7 MiB | 20.2 MiB | 41 |
| 3 | before | 6 | 10.0 MiB | 39.8 MiB | 78 |
| 3 | after, standalone | 6 | 10.2 MiB | 41.5 MiB | 78 |
| 3 | after, shared | 5 | 9.1 MiB | 33.3 MiB | 67 |
| 10 | before | 20 | 20.1 MiB | 132.9 MiB | 260 |
| 10 | after, standalone | 20 | 20.5 MiB | 137.7 MiB | 260 |
| 10 | after, shared | 12 | 13.8 MiB | 78.3 MiB | 158 |

Per process with 10 clients:

| Process | PSS | RSS | Open files |
|---|---|---|---|
| standalone server, after | 1.6 MiB | 9.1 MiB | 15 |
| its guardian | 0.5 MiB | 4.6 MiB | 11 |
| bridge | 0.9 MiB | 6.3 MiB | 12 |
| engine | 4.4 MiB | 10.4 MiB | 27 |
| the engine's guardian | 0.6 MiB | 4.6 MiB | 11 |

- With 10 clients, shared mode costs a third less memory than standalone (13.8 MiB against 20.5 MiB PSS), 8 fewer processes and 102 fewer open files. With one client it costs 0.6 MiB more, for the extra bridge; it breaks even at about two.
- The engine's own cost barely grows with its sessions: 4.9, 4.2 and 4.4 MiB PSS for 1, 3 and 10 clients, with 18, 20 and 27 open files, one more per connection.
- The branch's standalone server costs 0.2 to 0.4 MiB PSS more in all than `before` for the same clients.
- No process used CPU while idle: 0 clock ticks over ten seconds (at 100 per second) for every process, except one tick for the engine with 10 clients.

## Latency

Median round trip over 50 calls from one client, with that client alone and with 9 more connected and idle. The screenshot is the default one, a 1280x720 JPEG.

| Build and mode | `status`, 1 client | `status`, 10 clients | `screenshot`, 1 client | `screenshot`, 10 clients |
|---|---|---|---|---|
| before | 1.13 ms | 1.12 ms | 5.49 ms | 5.48 ms |
| after, standalone | 1.15 ms | 1.12 ms | 6.58 ms | 5.49 ms |
| after, shared | 1.15 ms | 1.14 ms | 6.59 ms | 6.59 ms |

The bridge adds nothing measurable to `status`. Screenshot medians fall at either about 5.5 ms or about 6.6 ms from one run to the next, in every build and mode: an earlier run of the same three builds gave `before` 6.54 ms with one client and the shared engine 6.60 ms. The difference between the rows is that variation, not the bridge.

The first client's start, from starting its server to the reply to `initialize`: 1.3 to 2.7 ms standalone, and 44.6 to 44.8 ms in shared mode when no engine runs, since the bridge starts the engine and waits for its socket.

### Head-of-line blocking

The engine serves every client on one thread, so one client's work can delay another's. The measure sends a `status` from one client during another client's screenshot, 0 to 9.5 ms after the screenshot request in 20 steps, so the tries cover the capture and the encoding:

| Build and mode | median | longest |
|---|---|---|
| before | 1.15 ms | 3.25 ms |
| after, standalone | 1.15 ms | 3.27 ms |
| after, shared | 1.18 ms | 3.29 ms |

Two standalone servers are separate processes, yet their longest `status` is as long as the shared engine's: what delays it is niri, busy with the capture, not the engine's thread. The 10-client latency didn't regress either, so moving the encoding off the engine's thread (plan amendment L5) isn't needed.

## Churn

200 clients, one after another, each starting, calling `status` and going, with one other client connected throughout and without. The engine's PSS and open files:

| After cycle | 1 | 50 | 100 | 150 | 200 |
|---|---|---|---|---|---|
| anchored, PSS | 4668 KiB | 4732 KiB | 4742 KiB | 4758 KiB | 4758 KiB |
| anchored, open files | 18 | 18 | 18 | 18 | 18 |
| unanchored, PSS | 5896 KiB | 5968 KiB | 5970 KiB | 5970 KiB | 5970 KiB |
| unanchored, open files | 17 | 17 | 17 | 17 | 17 |

PSS grows by 64 to 72 KiB over the first 50 cycles and then stays within 26 KiB: allocator warm-up, not a per-session leak. Open files don't change. The unanchored clients came faster than the two-second idle grace, so one engine served all 200. Once the last one was gone, the engine exited within five seconds, and no engine, guardian or `engine.sock` was left. `tests/protocol/shared.rs` checks the same for 50 clients, in `make check`, against the engine's session count and open files.
