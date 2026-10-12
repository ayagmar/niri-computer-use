# Input robustness results

Measured on branch `input-robustness`, off `main` at `c3a7e0a`, on 2026-10-12. Designs and the reasons behind them are in [decisions](../decisions.md), 2026-10-12.

Environment: Rust 1.99.0; niri 26.04 (8ed0da4); Noctalia 5.2.1; Qt 6.12.0; kitty 0.49.2; AMD Ryzen 9 9950X3D, 32 threads; NVIDIA GeForce RTX 5090 with nvidia-utils 615.78.08, and Mesa 26.2.4 for the A/B below. Debug builds. Every nested run below was headless, in the private cage, with one 1280x720 `winit` output at scale 1, while three other workers' nested runs and builds took turns on the same machine. Every run reported C1's host snapshot unchanged, no entries from its processes in the host's journal, and no leftover processes. No run sent input to the host or read its clipboard.

## Gates

| Command | Run | Result |
|---|---|---|
| `make check` | | pass |
| `make nested-input` | `1791772717-712117` | pass, 152 s |
| `make nested-input SHARED=1` | `1791773008-800077` | pass, 152 s |
| `make nested-actions` | `1791771972-520692` | pass |
| `make nested-actions SHARED=1` | `1791772166-540393` | pass |
| `make nested-a11y` | `1791772340-577958` | pass |
| `make nested-a11y SHARED=1` | `1791772524-636899` | pass |

The input runs took 152 s of what was a 180-second deadline, so it is now 240 seconds, with the main server's and Noctalia's own deadlines raised to match.

## A guardian killed with its server

Drag started, then server A's guardian and A killed with SIGKILL, guardian first. Server B's guardian released the button as it started. Times from the kill, by the marker's `released` (ms resolution), and until `wev` showed the release (50 ms polling):

| Mode | niri acknowledged | `wev` saw it |
|---|---|---|
| standalone | 3 ms | within 102 ms |
| shared, the engine and its guardian killed | 4 ms | within 102 ms |

B still got `recovery_required` until `recover`. With the start-up release removed from the guardian, the run failed at this step (`m7-guardian: the guardian's releases noted in the marker not seen within 5s`), after the existing mid-drag crash check had passed.

## Native stalls

From a temporary loop, not committed: the native GTK entry case typing the C10 corpus (100 keys, extended keymap) and an ASCII line, ten times each, then ten more with the pacer, a window that redraws every frame, open. Key loop and round trips are server-side timings; the last column is from the tool call to the entry's report, polled every 50 ms.

| niri's EGL | Pacer | 100-key key loop | Longest round trip | Tool to entry, corpus |
|---|---|---|---|---|
| NVIDIA, default | off | 4.9 to 9.0 ms | 2.6 ms | 253 to 304 ms |
| NVIDIA, default | on | 3.15 to 3.20 s | 32.2 ms, every key | 3356 to 3462 ms |
| Mesa, default | on | 3.18 to 3.20 s | 32.7 ms, every key | 3356 to 3460 ms |
| Mesa, `vblank_mode=0` | on | 270 to 339 ms | 9.8 ms | 306 to 358 ms |

With the pacer open, every round trip waited two frames. Making the EGL swap interval 0 removed the wait, which places it in niri's winit backend: each swap blocks niri's main thread until the parent compositor's next frame. An earlier 85-call loop without the pacer had no slow call (the slowest 469 ms to the entry).

## Other toolkits

Through a native server, in `make nested-input`, both modes:

| App | ASCII and C10 corpus | 4000-character paste | `paste` |
|---|---|---|---|
| Qt Quick `TextField` | exact | exact, `ctrl+v` | `{"read": false, "clipboard": "restored"}` |
| kitty, `cat` on a raw terminal | exact | exact, `ctrl+shift+v` | `{"read": true, "clipboard": "restored"}` |

After each paste, the clipboard offered the owner's three types with the same bytes. Qt reads the clipboard as soon as it changes and keeps what it read, so it reads nothing after the key.

## Held modifiers

Keys from a virtual keyboard on cage, which the nested niri handles as physical:

| Backend | `abc` typed with Shift held | with Caps Lock on | `a` pressed by hand afterwards, still held or on |
|---|---|---|---|
| wtype | `abc` | `abc` | `A` |
| native | `abc` | `abc` | `a` |

## GTK entry timings

Tool call to the entry's report, min/median/max of 3, ms, standalone run:

| Backend | ASCII | C10 corpus | Submitted line |
|---|---|---|---|
| wtype | 272.6 / 273.0 / 594.8 | 433.1 / 433.3 / 433.4 | 272.5 / 272.8 / 273.1 |
| native | 301.0 / 301.3 / 301.7 | 301.0 / 301.3 / 301.5 | 301.2 / 301.4 / 301.6 |

4000 characters pasted: 275 ms with wtype, 302 ms with native.

## Needs the user

- Native typing speed on a niri running on a TTY, where the stall above shouldn't apply: it needs input to your session.
- The held-modifier result with a real keyboard, and with Num Lock: the nested check holds Shift and Caps Lock through cage.
- An Xwayland app: the nested config runs no `xwayland-satellite`, and the host check watches `/tmp/.X11-unix`.
