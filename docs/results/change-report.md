# Change report results

What the change report (`changes` in every action result, and `element_changes`) tells, how large it is and what it costs, measured on branch `change-report` at `7708d46`, against `main` at `366d4cf` before it. [After the review fixes](#after-the-review-fixes) has the runs with the code at `3e22c95`.

Environment: Rust 1.99.0; niri 26.04 (8ed0da4); AMD Ryzen 9 9950X3D, 32 threads. Debug builds, as `make nested-*` builds them. Every run below ran in the private headless cage, with one 1280x720 `winit` output at scale 1. No run sent input to the host or viewed host pixels, clipboard or window titles. Every run reported C1's host snapshot unchanged, no entries from its processes in the host's journal, and no leftover processes.

## Commands

```sh
make check
make nested-input
make nested-control
make nested-engine
make nested-a11y
make nested-actions
make nested-input SHARED=1
make nested-control SHARED=1
make nested-engine SHARED=1
make nested-a11y SHARED=1
make nested-actions SHARED=1
```

Each ran as `flock ~/.cache/ncu-nested.lock nice -n 19 make …` with `CARGO_BUILD_JOBS=8`. All ten passed:

| Suite | Standalone run | Shared run |
|---|---|---|
| `nested-input` | 1791732794-1565583 | 1791733093-1622784 |
| `nested-control` | 1791732944-1592407 | 1791733242-1645293 |
| `nested-engine` | 1791732963-1600621 | 1791733261-1646787 |
| `nested-a11y` | 1791732970-1601051 | 1791733268-1647343 |
| `nested-actions` | 1791733071-1619117 | 1791733369-1658940 |

`nested-a11y` and `nested-actions` log every report they check as `changes: <what>: checked <bool>, <bytes> bytes: <json>` in `target/e2e/<run>/harness.log`; [development.md](../development.md) lists what each must say. Both modes gave the same reports, apart from element refs.

## What the report told

26 reports per mode, 21 of them checked to tell what the action did on their own:

| Action | Report | Bytes |
|---|---|---|
| `launch` with a late `app_id` | the window opened, focus moved to it | 108 |
| `launch` of two windows | both windows opened, focus moved | 170 |
| `launch` with `reuse`, one window | focus moved to it | 27 |
| `focus_workspace` to an empty workspace, and back | workspace and focus, each way | 60, 60 |
| `close_window` | the window closed | 64, 38 |
| `launch` interrupted by a focus change | where focus went | 27 |
| `niri_action` `FullscreenWindow`, twice | the window's new size, each time | 40, 39 |
| `niri_action` `ToggleWindowFloating` | `floating: true`, and the slow fixture's window, which mapped meanwhile, opened and focused | 144 |
| `niri_action` `SetWindowWidth` | the new size | 39 |
| `click` on `Vanish`, `element_changes` | the button and its label removed | 107 |
| `activate_element`, relabelling button, `element_changes` | the button's new name with `was`; GTK 4 also its label, Qt also `focused` set | 219 (GTK 4), 125 (GTK 3), 146 (Qt) |
| `activate_element` on `Open dialog`, `element_changes` | the dialog opened and focused; `Primary` lost `focused` | 248 |
| `close_window` on the dialog | the dialog closed, focus back | 66 |
| `activate_element` on `Dismiss`, `element_changes` | the window closed, focus to none, elements `unknown: window_gone` | 153, 157, 164 |

Not checked:

- `set_element_text` with `element_changes` on each toolkit's plain entry: no report. The entry's name, role and states don't change with its text, and the report never reads text; `observed: matched` tells instead.
- `close_window` on a window that ignores the request: no report, as nothing changed; `observed: pending` and its screenshot tell.
- `niri_action` `Spawn`: the report listed the spawned window as opened and focused (111 bytes) in all four runs, but only because the fixture mapped its window within the report's quiet wait; a slower app would leave it to `wait_for`.

## Size

The reports above are 27 to 248 bytes of JSON, median 108. At about 4 bytes a token that is 7 to 62 tokens, median 27; a 1280x720 screenshot is about 1229 tokens (width × height / 750). The largest report is 5% of one screenshot.

The tool list grows from 30,895 to 31,846 bytes of JSON (+951, 3.1%): one `element_changes` property on six tools and longer descriptions on four. The server instructions grow from 1,928 to 2,172 bytes. Both measured from `tools/list` and `initialize` in the protocol tests' fake environment, on this branch and on `main`.

## Cost

Without `screenshot: true`, every action now waits until niri has been quiet for 250 ms, at most a second. `activate_element` took a median of 584, 594 and 603 ms (GTK 4, GTK 3, Qt) per call in the standalone run, against 316 to 346 ms on `main` ([m9b.md](m9b.md)); its own wait is unchanged, and the difference is that quiet wait. `shell_open` gave `opened` in 302 ms (`nested-shell`, run 1791732546-1525643), against 50 ms before.

`element_changes` adds two walks of the window. The first activation of each toolkit, the one with `element_changes`, took 630, 588 and 605 ms, against medians of 584, 594 and 603 ms for the other nineteen: the walks of these small fixture windows cost at most 46 ms.

## After the review fixes

Measured with the code at `3e22c95`; the one commit after it, which the runs included, changed only the skill's reference. The quiet wait now counts from niri's last event, or the start of the action, so time the action's own wait spends quiet counts toward it, and it adds at most 500 ms after that wait. `focused_window` comes from the view the report ends at. The server instructions are 1,736 characters, down from 2,172.

```sh
make nested-actions
make nested-actions SHARED=1
make nested-a11y
make nested-a11y SHARED=1
make nested-shell
make nested-shell SHARED=1
```

Each ran as `flock ~/.cache/ncu-nested.lock nice -n 19 make …` with `CARGO_BUILD_JOBS=8`:

| Suite | Standalone run | Shared run |
|---|---|---|
| `nested-actions` | 1791743023-135913, pass | 1791743043-138014, pass |
| `nested-a11y` | 1791743063-140091, pass | 1791743146-157391, pass |
| `nested-shell` | 1791743228-174215, pass | 1791743236-175599, fail |

The shared `nested-shell` run failed one check: "no shell tools without noctalia on PATH". It starts a server with an empty `PATH` and expects no `shell_` tools, but with `--shared` that server bridges to the shared engine, which found `noctalia` on its own `PATH` and lists them. Nothing in the change report decides that list. Every other check in the run passed.

Medians per call, standalone runs, from the harness log (`activate_element`, the 19 calls after the first per toolkit) and the audit log's `duration_ms` (`focus_window`, `click`):

| Action | `main` | First version | Now |
|---|---|---|---|
| `activate_element` (GTK 4, GTK 3, Qt) | 318, 323, 329 ms | 584, 594, 603 ms | 319, 324, 332 ms |
| `focus_window` | 13 ms | 264 ms | 263 ms |
| `click` at a pixel | 3 ms | 255 ms | 251 ms |
| `shell_open` | 51 ms | 302 ms | 302 ms |
| `shell_close` | 352 ms | 605 ms | 352 ms |

`main` is `2dfde99`, run 1791734946-1917389; for the shell tools, the last `nested-shell` run on `main`, 1791606558-342961 from 2026-10-10, since when `src/act/shell.rs` and `src/noctalia.rs` gained only a struct field. The first version is `7708d46`, runs 1791732970-1601051 and 1791732546-1525643. The shared runs gave the same within 3 ms, except the tray drawer's `shell_open` below.

`activate_element` and `shell_close` are back to what they cost on `main`: their own waits, 300 ms on niri's stream and Noctalia's polling, are quiet, so the report adds nothing. `focus_window` and `click` still pay about 250 ms: `focus_window` ends at niri's focus event, and the quiet wait counts from it, while `click` returns at once with `sent`. `shell_open` gets `opened` from Noctalia in about 50 ms, and in these runs niri then sent an event, so the report waited 250 ms after it; one shared `shell_open` of the tray drawer took 252 ms.

`element_changes` still costs little: the first activation of each toolkit, with its two walks, took 370, 380 and 328 ms against the medians above.

What the reports said is the same as before, but for two:

- `niri_action` `ToggleWindowFloating` reported only `floating: true` (38 bytes, was 144). The slow fixture's window now maps after this report ends, so its opening isn't in it; it shows in the next `close_window`'s report instead (64 bytes, was 38).
- No result now names a `focused_window` other than its `changes.focus.to`: 0 of 11 results with a focus change in each `nested-actions` run. The first version's run had 2 of 12 (the floating report and `Spawn`, which gave `focused_window` 9 and 10 beside `focus.to` 10 and 11).
