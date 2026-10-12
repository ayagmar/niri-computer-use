# Shared engine by default: results

What serving every client through the shared engine costs, measured on branch `daemon` once shared mode became the default and the engine got a systemd scope of its own. [engine.md](engine.md) has the first measurements of the engine, against the build before it.

Environment: Rust 1.99.0; niri 26.04 (8ed0da4); AMD Ryzen 9 9950X3D, 32 threads. Release builds. The nested runs ran in the private headless cage, with one 1280x720 `winit` output at scale 1, under `nice -n 19` while other builds shared the machine, so treat differences under a millisecond as noise. No run sent input to the host or viewed host pixels, clipboard or window titles.

## Commands

```sh
make nested-measure
make nested-measure SHARED=1
make nested-engine
```

Each writes `measure.json` to `target/e2e/<run>/`; [development.md](../development.md#nested-shared-engine-checks) says what it measures. Round trips are timed by reading the client's log every millisecond, so a `status` near 1.1 ms is mostly that polling.

## Latency

50 round trips of each call from the first client, with that client alone and with 9 more connected and idle.

| Call, clients | Standalone median | Standalone p95 | Shared median | Shared p95 |
| --- | --- | --- | --- | --- |
| `status`, 1 | 1.13 ms | 1.16 ms | 1.19 ms | 1.23 ms |
| `status`, 10 | 1.13 ms | 1.17 ms | 1.13 ms | 1.14 ms |
| `screenshot`, 1 | 5.51 ms | 6.56 ms | 5.73 ms | 7.64 ms |
| `screenshot`, 10 | 5.49 ms | 6.60 ms | 5.48 ms | 5.60 ms |

The relay adds at most a tenth of a millisecond to a median. The screenshot p95 moves by about a millisecond in either direction between the two modes, the same spread [engine.md](engine.md#latency) found between runs of one build.

A `status` sent while another client's screenshot runs took 1.20 ms at the median and 3.28 ms at most standalone, and 1.19 ms and 2.30 ms in shared mode.

## Cold start

From starting the first client's server to its reply to `initialize`, with no engine running:

| Mode | 1 client | 3 clients | 10 clients |
| --- | --- | --- | --- |
| Standalone | 2.4 ms | 2.4 ms | 2.5 ms |
| Shared | 65.5 ms | 65.6 ms | 65.6 ms |

The shared figure includes starting the engine and its guardian and waiting for its socket. The nested session can't reach the user's systemd manager, so there each bridge's `systemd-run` fails and the bridge starts the engine itself. On the host, outside the nested session, 20 runs of each gave:

| Command | Median | p95 |
| --- | --- | --- |
| `true` | 0.2 ms | 0.3 ms |
| `systemd-run --user --scope --collect --quiet -- true` | 3.3 ms | 4.6 ms |
| the same, with no manager socket | 0.9 ms | 1.2 ms |

So the scope adds about 3 ms to a cold start when systemd is reachable, and a failed `systemd-run` about 1 ms. Only the client that starts the engine pays the cold start; every later client reaches the running engine.

## Memory and processes

Totals over every process the clients caused, five seconds after their last call:

| Clients | Mode | Processes | PSS | RSS | Open files |
| --- | --- | --- | --- | --- | --- |
| 1 | standalone | 2 | 7.9 MiB | 14.7 MiB | 26 |
| 1 | shared | 3 | 8.6 MiB | 22.2 MiB | 41 |
| 3 | standalone | 6 | 11.2 MiB | 44.6 MiB | 78 |
| 3 | shared | 5 | 10.3 MiB | 36.7 MiB | 67 |
| 10 | standalone | 20 | 22.5 MiB | 149.0 MiB | 260 |
| 10 | shared | 12 | 16.5 MiB | 87.7 MiB | 158 |

One client costs a third process and 0.7 MiB more PSS in shared mode; from three clients on, shared mode uses less of everything. Over 200 clients coming and going, with another client connected throughout, the engine kept 18 open files and its PSS grew from 5080 KiB to 5206 KiB, and no guardian was left behind.

## The engine's scope

`make nested-engine` passed E1 to E6. In E6, against the user's real systemd manager, client X ran in `ncu-e6-x-<pid>.scope` and started the engine, which ran with its guardian in `run-p<engine pid>-i<n>.scope`. Stopping X's scope ended X's server and left client Y served by the same engine; once Y left, the engine exited and its scope was collected.
