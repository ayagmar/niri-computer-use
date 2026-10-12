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

## Shorter descriptions

A wording-only pass on branch `desc-trim`, rebased onto `main` at `8df9acc`: descriptions no longer list error names or repeat the lease and "never repeat" rules that the server instructions state once, and repeated field text is shorter. Tool names, parameters, error names and annotations are unchanged. [decisions.md](../decisions.md) has the reasons.

`tools/list` measured as above, by `the_tool_list_stays_within_its_budget`. `main` was measured at `8f798ae`; `8df9acc` changed no description and lists the same. The branch column is the branch's last code commit, with `type_text`'s `focus_mismatch` clause back (see iteration 8).

| Profile | `main`: bytes, tokens | Branch: bytes, tokens | Change | New ceiling |
|---|---|---|---|---|
| `observe` | 6,946, ~1,736 | 6,240, ~1,560 | −10% | 6,350 (was 7,000) |
| `act` | 26,855, ~6,713 | 23,172, ~5,793 | −14% | 23,250 (was 27,000) |
| `full` (default) | 30,838, ~7,709 | 26,403, ~6,600 | −14% | 26,500 (was 31,000) |

The server instructions are 1,745 characters, up from 1,700 and under the 1,900 their test allows, because they now say once that actions and `clipboard_read` need the lease.

Each tool in bytes, largest first on `main`:

| Tool | `main` | Branch |
|---|---|---|
| `type_text` | 2,889 | 2,391 |
| `activate_element` | 2,129 | 1,643 |
| `screenshot` | 1,981 | 1,780 |
| `elements` | 1,942 | 1,588 |
| `set_element_text` | 1,904 | 1,766 |
| `wait_for` | 1,885 | 1,809 |
| `key` | 1,782 | 1,580 |
| `niri_action` | 1,686 | 1,351 |
| `click` | 1,535 | 1,541 |
| `drag` | 1,495 | 1,379 |
| `launch` | 1,375 | 935 |
| `scroll` | 1,227 | 1,155 |
| `noctalia` | 1,223 | 1,010 |
| `pointer_move` | 1,157 | 860 |
| `release_desktop` | 1,104 | 1,104, unchanged: see below |
| `shell_panel` | 1,069 | 865 |
| `acquire_desktop` | 997 | 607 |
| `focus` | 964 | 853 |
| `close_window` | 752 | 631 |
| `desktop_state` | 664 | 637 |
| `clipboard_read` | 585 | 473 |
| `status` | 456 | 408 |

### Nested suites

At the rebased head, with the server of the branch's `refactor(tools)` commit `5ccbda8`, every suite passed in both modes, 20 of 20, run as `flock ~/.cache/ncu-nested.lock nice -n 19 make …` with `CARGO_BUILD_JOBS=8`:

| Suite | Standalone run | Shared run |
|---|---|---|
| `nested` | 1791842854-2359339 | 1791842863-2360211 |
| `nested NOCTALIA=1` | 1791842873-2360847 | 1791842885-2361675 |
| `nested-control` | 1791842897-2362601 | 1791842916-2363165 |
| `nested-actions` | 1791842935-2363801 | 1791842952-2364461 |
| `nested-input` | 1791842970-2364969 | 1791843121-2371929 |
| `nested-shell` | 1791843273-2378798 | 1791843281-2379266 |
| `nested-a11y` | 1791843292-2379713 | 1791843420-2418307 |
| `nested-a11y SSD=1` | 1791843547-2456990 | 1791843732-2498168 |
| `nested-engine` | 1791843917-2540530 | 1791843929-2541044 |
| `nested-measure` | 1791843993-2543072 | 1791844052-2545046 |

### Skill evals

Results are in `target/skill-evals/` (not committed).

**Iteration 6, the full matrix, before the rebase.** The seven M6 scenarios, twice each with and without the skill, for Sonnet and Haiku. It ran on a commit since rewritten: the trim with `release_desktop`'s text restored, based on `main` at `8aac368`, without the review's fixes (`click`'s deny-list note, `set_element_text`'s two clauses, `elements`' `tiled` reason). `main`'s changes since `8aac368`, among them the skipped quiet wait after `focus` and `niri_action` and the pointer's checks on moving windows, weren't under test in it.

| Model | Configuration | Iteration 5 | Iteration 6 |
|---|---|---|---|
| Sonnet | with the skill | 85/86, 99% | 85/86, 99% |
| Sonnet | without | 85/86, 99% | 84/86, 98% |
| Haiku | with the skill | 86/86, 100% | 83/86, 97% |
| Haiku | without | 83/86, 97% | 86/86, 100% |

Its misses: twice in `dialog-midway`, Sonnet's answer didn't use the word "dialog" the grader looks for, once with the skill and once without, as in iteration 5. Sonnet without the skill released with `restore_focus: false` in one of two `click-elsewhere` runs, as in iterations 3 and 5. Haiku with the skill sent two `key` calls in one turn in `key-screenshots`. And in `dialog-midway` it saw the dialog had focus, sent nothing and asked the user which window to type into, which cost two expectations.

Three results needed a closer look, and one led to a fix:

- The first version of the trim cut `release_desktop`'s description to two sentences and merged the lease rule into the instructions' "one action at a time" sentence. In 9 Sonnet runs of `click-elsewhere` and `key-screenshots` (`iteration-6a-sonnet` and `6b-sonnet`), it released with `restore_focus: false` once with the skill and once without, and twice sent its last `key` and `release_desktop` in one turn, which iterations 4 and 5 never did in eight runs of `key-screenshots`. `6b` had the two sentences on restoring focus back but not the one saying the release passes an action's checks first. With `release_desktop`'s full text back, 8 targeted runs (`iteration-6c-sonnet`) had one miss, the `restore_focus: false` without the skill that earlier iterations also had. The instructions sentence went back to "To act, call `acquire_desktop`, then one action at a time", with the lease rule in a sentence of its own.
- Haiku sends two `key` calls in one turn in `key-screenshots` on `main` too. Same-day runs at `8aac368` did so in 2 of 16 (in the session's scratch directory, not kept); the branch's final wording did so in 3 of 12 (`iteration-6e-haiku` and iteration 6). Putting `key`'s original description back made no difference (3 of 8, `iteration-6f-haiku`), so it stays trimmed. With so few runs, the gap between 2 of 16 and 3 of 12 can't be told from chance.
- Haiku's `dialog-midway` stop: in a same-day rerun, `main` at `8aac368` and the branch each passed 4 of 4 (`iteration-6g-haiku`).

Also, in `iteration-6a-sonnet`'s `compose-message` without the skill, Sonnet once called `type_text` by its short name instead of `mcp__niri-computer-use__type_text`, and Claude Code refused the call before it reached the server.

**Iteration 7, at the rebased head** (`5ccbda8`'s server): `dialog-midway` and `key-screenshots`, twice each with the skill, for Sonnet and Haiku (`iteration-7-*`). `key-screenshots` passed all 4 runs, with no calls sent together. `dialog-midway` passed 2 of 4: one Sonnet run got `focus_mismatch` once the dialog had focus, and one Haiku run saw it in `desktop_state`; both sent nothing and asked the user rather than focus wev again and type.

`dialog-midway` with the skill, every run of the day at the head and at `main` `8df9acc` (`iteration-7-*`, `iteration-7b-*`, and control runs in the session's scratch directory, not kept):

| Model | Branch: passed, stopped to ask | `main` `8df9acc`: passed, stopped to ask |
|---|---|---|
| Sonnet | 5 of 6, 1 | 2 of 4, 0 |
| Haiku | 4 of 6, 1 | 4 of 4, 0 |

Every other miss in these runs was the grader's word check: both of `main`'s Sonnet misses, and one Haiku run on the branch. With iteration 6's stop and its rerun, the branch stopped to ask in 3 of 20 `dialog-midway` runs with the skill, and `main` in none of 12. A stop sends nothing and tells the user, so it is a missed task rather than a wrong call. The runs are too few to say whether the shorter `type_text`, `key` or `focus` descriptions cause it.

**Iteration 8, with `type_text`'s clause back.** The branch's next commit, `fix(tools): restore type_text's focus_mismatch clause`, puts back "(`focus_mismatch` otherwise)" after `type_text`'s `expect` rule. That adds 29 bytes: `act` 23,172 and `full` 26,403, with the ceilings unchanged. At that commit, `dialog-midway` with the skill passed all 8 runs, 4 for Sonnet and 4 for Haiku (`iteration-8-*`). None stopped to ask, and none missed the word check. Neither those runs nor `main`'s 12 stopped to ask.
