# Multi-output pointer results

Evidence for the pointer tools on more than one monitor (`docs/decisions.md`, 2026-10-30). Nested runs keep their files in `target/e2e/<run>/`, which isn't committed.

Machine: Rust 1.99.0; niri v26.04 (8ed0da4) built from source with `harness/niri-test/headless-outputs.patch` (it reports `26.04 (v26.04-modified)`); Noctalia 5.2.1. No input was sent to the real session, and no run had a parent display.

| Check | Command | Run | Observation | Result |
|---|---|---|---|---|
| Two outputs, mixed scales, a negative origin | `make nested-multi` | 1793368698-1374838 | niri reported `headless-1`, 1280x720 at scale 1 at (0, 0), and `headless-2`, 1280x720 logical at scale 1.5 at (-1280, 100); `status` listed both in `outputs.pointer_outputs`. | pass |
| Accuracy on each output | the same | the same | `wev` moved to each output with `MoveWindowToMonitor`; C4's five surface points through `pointer_move` from a screenshot of that output. On `headless-1` at capture scale 1, every motion landed on the pixel's centre (off by 0.0000), and at 0.546875 within 0.0031 px. On `headless-2` at capture scale 1.5, within 0.0013 px, and at 0.546875 within 0.0031 px. The rule is ±0.05 px. | pass |
| Clicks on each output | the same | the same | The GTK 4 fixture's button, moved to each output, clicked 20 times from a screenshot of that output: 20 of 20 counted once each on `headless-1` and on `headless-2`. | pass |
| Containment | the same | the same | C1's host snapshot unchanged, no host journal entries from the run, no leftover processes. | pass |
| An unpatched niri doesn't start | `cargo run --locked -q -p harness -- run --niri /usr/bin/niri --multi` | 1793368734-1433596 | Without the patch, niri starts its `winit` backend, finds no compositor at the dead parent socket (`NoCompositor`) and exits; no window opened. The run fails, as it should. | pass |
| Output changes mid-gesture | `make check` | — | The property test `an_output_change_never_leaves_input_reported_sent_or_held` draws 40 cases from a gesture (move, click, drag, scroll, with or without a held `ctrl`), a moment (before the call, while the pointer connects, at one of the requests the gesture makes) and an event (the bound output unplugged, turned off, replugged under the same name, rescaled, moved or rotated; another output unplugged, turned off, replugged, rescaled or added). Every motion went through a pointer bound to the screenshot's output, none went out to an output no longer as captured once a round trip could have shown the change, nothing stayed held, the marker was gone, a refusal sent nothing, `sent` came only without a change to the bound output, and a change to another output never stopped the gesture. `a_drag_stops_once_niri_replugs_its_output` replugs the bound output at the drag's eighth request: the drag stops at its next pause and ends `uncertain`. With the output checks before each send and after each pause removed, the eight motions after the replug go out, mapped through niri's fallback, and that test fails. | pass |

## Not covered

- No two real monitors: the owner has one. `scripts/real-pointer-check.py --output NAME` runs the supervised accuracy check on each named output when a second screen is attached.
- The patched niri has no hotplug, so the nested run doesn't unplug an output mid-gesture; only the fake niri does.
- Rotated and flipped monitors stay refused and aren't run here.
