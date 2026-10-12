# Tool surface results

What the smaller tool surface costs a client and how agents do with it, measured on branch `tool-surface` at `96c76d3`, against `main` at `943f32e` before it.

Environment: Rust 1.99.0; niri 26.04 (8ed0da4); Noctalia 5.2.1; Claude Code's `claude -p` with the models `sonnet` and `haiku`. Debug builds, as `make nested-*` builds them. Every nested run ran in the private headless cage, with one 1280x720 `winit` output at scale 1. No run sent input to the host or viewed host pixels, clipboard or window titles. Every run reported C1's host snapshot unchanged, no entries from its processes in the host's journal, and no leftover processes.

## Tool list size

`tools/list` as a client gets it, with every tool the profile can list present: Noctalia installed, `unrestricted` on and an accessibility bus. Bytes of compact JSON; tokens estimated as bytes / 4. `main` has no profiles, so its row for `observe` and `act` is the same selection of its tools: `observe` takes `status`, `desktop_state`, `screenshot`, `elements` and `wait_for`, and `act` leaves out `niri_action`, `noctalia` and the three `shell_*` tools.

| Profile | `main`: tools, bytes, tokens | Branch: tools, bytes, tokens | Change |
|---|---|---|---|
| `observe` | 5, 7,556, ~1,889 | 5, 6,553, ~1,638 | −13% |
| `act` | 22, 35,907, ~8,976 | 19, 25,940, ~6,485 | −28% |
| `full` (default) | 27, 41,643, ~10,410 | 22, 29,884, ~7,471 | −28% |

The branch's sizes include what it added: a `title` on every tool, `openWorldHint` on the reading tools, `_meta` on three, and the `elements` arguments `interactive_only` and `format`. The unit test `the_tool_list_stays_within_its_budget` prints these sizes with `--nocapture` and fails above 7,000, 27,000 and 31,000 bytes.

The server instructions are 1,700 characters, down from 1,736, under the 1,900 their test allows.

Each tool in bytes, largest first:

| Tool | `main` | Branch |
|---|---|---|
| `type_text` | 2,403 | 2,686, with `paste` folded in |
| `paste` | 3,134 | in `type_text` |
| `activate_element` | 3,491 | 2,071 |
| `set_element_text` | 2,871 | 1,904 |
| `wait_for` | 2,405 | 1,885 |
| `elements` | 1,857 | 1,794 |
| `key` | 2,691 | 1,782 |
| `screenshot` | 2,413 | 1,738 |
| `niri_action` | 2,296 | 1,686 |
| `click` | 1,983 | 1,535 |
| `drag` | 1,966 | 1,495 |
| `launch` | 1,723 | 1,375 |
| `scroll` | 1,524 | 1,227 |
| `noctalia` | 1,310 | 1,186 |
| `pointer_move` | 1,496 | 1,157 |
| `shell_panel` | `shell_open` 1,081, `shell_close` 763 | 1,069 |
| `release_desktop` | 936 | 995 |
| `focus` | `focus_window` 903, `focus_workspace` 786 | 964 |
| `acquire_desktop` | 768 | 845 |
| `close_window` | 884 | 752 |
| `desktop_state` | 496 | 664, with `outputs` and `shell_status` folded in |
| `outputs` | 241 | in `desktop_state` |
| `shell_status` | 281 | in `desktop_state` |
| `clipboard_read` | 534 | 585 |
| `status` | 369 | 456 |

## Commands

```sh
make check
make nested
make nested SHARED=1
make nested NOCTALIA=1
make nested NOCTALIA=1 SHARED=1
make nested-control
make nested-control SHARED=1
make nested-actions
make nested-actions SHARED=1
make nested-input
make nested-input SHARED=1
make nested-shell
make nested-shell SHARED=1
make nested-a11y
make nested-a11y SHARED=1
make nested-a11y SSD=1
make nested-a11y SSD=1 SHARED=1
make nested-engine
make nested-engine SHARED=1
make nested-measure
make nested-measure SHARED=1
make nested-eval SCENARIO=<scenario> SKILL=skills/niri-computer-use MODEL=<model>
make nested-eval SCENARIO=<scenario> SKILL=none MODEL=<model>
```

Each nested run ran as `flock ~/.cache/ncu-nested.lock nice -n 19 make …` with `CARGO_BUILD_JOBS=8`, at `a04525a`, which has the same server code as `96c76d3` apart from two descriptions.

## Nested suites

19 of 20 passed:

| Suite | Standalone run | Shared run |
|---|---|---|
| `nested` | 1791750276-1429725, pass | 1791750286-1432060, pass |
| `nested NOCTALIA=1` | 1791750296-1433424, pass | 1791750308-1435380, pass |
| `nested-control` | 1791750319-1436924, pass | 1791750338-1438558, pass |
| `nested-actions` | 1791750357-1440151, pass | 1791750377-1441198, pass |
| `nested-input` | 1791750397-1443089, pass | 1791750542-1458224, pass |
| `nested-shell` | 1791750686-1466483, pass | 1791750694-1466883, fail |
| `nested-a11y` | 1791750702-1467384, pass | 1791750785-1474722, pass |
| `nested-a11y SSD=1` | 1791750868-1482119, pass | 1791751005-1489728, pass |
| `nested-engine` | 1791751141-1497141, pass | 1791751148-1497656, pass |
| `nested-measure` | 1791751219-1502100, pass | 1791751277-1504002, pass |

`nested-shell SHARED=1` fails its last check, "no shell tools without noctalia on PATH": the server it starts with an empty `PATH` lists `shell_panel`. It failed twice more on the branch (1791759708-1676408, 1791759716-1677381) and also fails on `main` at `943f32e` (1791759745-1681516), where that server lists `shell_open`, `shell_close` and `shell_status`. The check starts its server right after the suite's first server stops, while the shared engine that server started is still running; the new server joins it, and the engine decided once, at its own start, that Noctalia is installed. `PATH` isn't part of what a client brings to the shared engine, so the check can only pass in shared mode once that engine has exited.

## Skill evals

The seven M6 scenarios, each run twice with the skill (`with_skill`) and twice without (`without_skill`), for Sonnet and for Haiku, as iteration 3 in [m6.md](m6.md) did. Results are in `target/skill-evals/` (not committed): `iteration-4-*` at `a04525a`, then `iteration-5-*` at `96c76d3`, after the fix described below. Iteration 3 ran in a visible nested window, and keys pressed on the host reached it; these runs are headless and replace its scores.

Expectations passed:

| Model | Configuration | Iteration 3 | Iteration 4 (`a04525a`) | Iteration 5 (`96c76d3`) |
|---|---|---|---|---|
| Sonnet | with the skill | 84/86, 98% | 86/86, 100% | 85/86, 99% |
| Sonnet | without | 83/86, 97% | 84/86, 98% | 85/86, 99% |
| Haiku | with the skill | 81/86, 94% | 83/86, 97% | 86/86, 100% |
| Haiku | without | 83/86, 97% | 84/86, 98% | 83/86, 97% |

Iteration 4 had two scenarios that did worse than iteration 3, both in `click-elsewhere`:

- Sonnet without the skill released the desktop with `restore_focus: false` in both runs (iteration 3: one run). Both answers said the click had moved focus to wev, which the change report now shows, and the field said "false to leave focus where your task put it". The field now says false is only for a window the user asked to stay in front.
- Haiku, with and without the skill, sent `focus` in the same turn as `acquire_desktop` in one run each (iteration 3: none). `acquire_desktop`'s description now says to wait for its result before the next call.

With those two descriptions changed, `click-elsewhere` passed all 8 runs (`iteration-4b-*`), and iteration 5 reran every scenario. In iteration 5, three scenarios did worse than iteration 3 for one model and configuration, by one expectation each:

| Model and configuration | Scenario | Iteration 3 | Iteration 5 | What happened |
|---|---|---|---|---|
| Sonnet with the skill | `dialog-midway` | 14/16 | 15/16 | Better overall; one answer didn't use the word "dialog" for the window that took focus |
| Haiku without | `dialog-midway` | 15/16 | 14/16 | Both answers told the user that a window titled "harness window" took focus, but the grader looks for "dialog" |
| Haiku without | `errand` | 18/18 | 17/18 | One run sent `focus` and `release_desktop` in the same turn |

Sonnet without the skill still passed `restore_focus: false` in one of two `click-elsewhere` runs, as in iteration 3. None of these follows from a description this branch changed, so they were left as they are.

Calls per task, averaged over the seven scenarios with the skill: Sonnet 4.8 in iteration 3 and 5.1 in iteration 5, Haiku 5.4 and 5.7. Most of the increase is `dialog-midway`, 4.0 calls for Sonnet before and 8.0 now. In iteration 3's audit logs, every Sonnet run typed the message in one `type_text` before the dialog took focus. In iteration 5, three of its four runs got `focus_mismatch` first, then focused wev and typed again: the dialog now takes focus before the agent types, which is what the scenario tests.
