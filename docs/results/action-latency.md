# Action latency results

How long each action tool takes from request to response, and where that time goes, measured on branch `action-latency` against `main` at `8aac368` before it.

Environment: Rust 1.99.0; niri 26.04 (8ed0da4); AMD Ryzen 9 9950X3D, 32 threads. Release builds. Every nested run below ran in the private headless cage, with one 1280x720 `winit` output at scale 1 and the nested Noctalia as the lock source; the nested niri runs without `--session`, so it never asks `loginctl`. No run sent input to the host or read its clipboard. Every run reported C1's host snapshot unchanged, no entries from its processes in the host's journal, and no leftover processes.

## Commands

```sh
make nested-latency SERVER=<before>
make nested-latency SERVER=<after>
```

Each ran as `flock ~/.cache/ncu-nested.lock make …`, with `<before>` a release build of `8aac368` and `<after>` one of this branch. `make nested-latency` makes 30 rounds of nine calls, none with a screenshot; `docs/development.md` describes them. Each call came back with the same `observed` in all 30 rounds, before and after: `focused` for both `focus` calls, `sent` for `click`, `type_text` and `key`, `present` for `activate_element`, `changed` for `niri_action`, and `opened` and `closed` for `shell_panel`.

| Run | Build | Run id |
|---|---|---|
| before 1 | `8aac368` | 1791832459-253343 |
| before 2 | `8aac368` | 1791833582-517540 |
| after 1 | this branch | 1791833321-463707 |
| after 2 | this branch | 1791833848-587361 |

## Before and after

Median round trip in milliseconds, with the 95th percentile in parentheses, over 30 calls each.

| Call | before 1 | before 2 | after 1 | after 2 |
|---|---|---|---|---|
| `focus` on the empty workspace | 291 (300) | 284 (315) | 281 (300) | 280 (284) |
| `focus` on the fixture | 253 (257) | 253 (256) | 253 (257) | 253 (257) |
| `click` on the entry, by element | 253 (253) | 252 (253) | 252 (253) | 252 (253) |
| `type_text` `"ab"` | 498 (499) | 498 (500) | 260 (262) | 260 (262) |
| `key` `BackSpace` twice | 258 (260) | 258 (275) | 252 (253) | 252 (253) |
| `activate_element` | 304 (306) | 304 (305) | 304 (306) | 304 (305) |
| `niri_action` `SetWindowHeight` | 262 (264) | 262 (264) | 261 (263) | 261 (263) |
| `shell_panel` open | 382 (383) | 382 (383) | 382 (398) | 382 (383) |
| `shell_panel` close | 512 (514) | 512 (514) | 512 (514) | 512 (528) |

`type_text` lost 238 ms and `key` 6 ms. The rest moved by at most 10 ms; `focus` on the empty workspace, which moved most, also differed by 7 ms between the two runs of `8aac368`.

## Where the time went, before

A copy of `8aac368` with timing marks added, not committed, ran the same rounds (run 1791832945-349811). Each mark wrote the time since the action reached the desk: the action mutex taken, the gate's refusal done, the call that acts going out, the work done, the evidence done, and the change report's quiet wait begun and ended. It also logged every event from niri's stream and how long each part of the readiness report and the lock read took. Medians in milliseconds:

| Call | Gate | Work | Quiet wait | Total |
|---|---|---|---|---|
| `focus` on the empty workspace | 12.0 | 15.9 | 251.3 | 281.4 |
| `focus` on the fixture | 0.3 | 0.6 | 251.3 | 252.2 |
| `click` | 0.3 | 1.7 | 249.4 | 251.3 |
| `type_text` | 8.1 | 114.2 | 373.6 | 496.0 |
| `key` | 6.4 | 69.0 | 182.3 | 257.6 |
| `activate_element` | 0.3 | 302.9 | 0.0 | 303.2 |
| `niri_action` | 0.3 | 211.6 | 50.1 | 262.0 |
| `shell_panel` open | 0.3 | 17.7 | 363.1 | 381.0 |
| `shell_panel` close | 4.4 | 301.2 | 206.3 | 511.8 |

The work is the tool's own: wtype typing (one process per stroke), `activate_element`'s 300 ms look at the element afterwards, `niri_action`'s 200 ms for a resize that comes in steps, and `shell_panel` polling Noctalia every 100 ms until its panel id changes, which on close comes after Noctalia's close animation. Evidence (no screenshot) took under 0.1 ms everywhere.

### The gate

The gate, which is the readiness report, then the lock read, took 0.2 to 0.4 ms in the nested session, except right after a call that kept niri busy. There niri's answers to `Version` and `Outputs`, which the report asks side by side, came back equally late, 4 to 12 ms at the median and up to 31 ms. That is niri answering any request late, not the number of requests: dropping `Outputs` would leave `Version` just as late. Noctalia's `status` took 0.05 ms, and the display check 0.02 ms. `activate_element` runs the gate twice, the second time in its last checks, at 0.1 to 0.2 ms.

The nested niri has no logind session, so `loginctl` never ran there. On the host session, `niri-computer-use status` with the same marks, 30 times, which asks niri, Noctalia and `loginctl` and sends no input:

| Part | Median | Longest |
|---|---|---|
| niri `Version` | 0.18 | 0.47 |
| niri `Outputs` | 0.17 | 0.34 |
| Noctalia `status` | 0.15 | 0.21 |
| logind (niri's PID, then `loginctl`) | 0.91 | 1.29 |
| `loginctl` alone | 0.87 | 1.22 |
| display check | 0.01 | 0.02 |

The parts run side by side, so the report takes about 1 ms, and the lock read after it, which asks Noctalia and logind again, about 1 ms more: about 2 ms an action, or under 1% of the shortest call above. The gate wasn't changed (see `docs/decisions.md`, "2026-10-12: action latency").

### The quiet wait

The change report waits until 250 ms have passed, counted from the start of the action, without niri reporting a change, at most 500 ms after the action's own wait. Every event niri sent during the report, in round 6, by time since the action reached the desk:

| Call | niri's events | Work done | Report done |
|---|---|---|---|
| `focus` on the empty workspace | `WorkspaceActivated`, `WindowFocusChanged` at 48 | 48 | 299 |
| `focus` on the fixture | `WorkspaceActivated`, `WindowFocusChanged` at 1 | 1 | 251 |
| `click` | none | 2 | 251 |
| `type_text` | `WindowFocusTimestampChanged` at 246 | 123 | 496 |
| `key` | none | 75 | 257 |
| `activate_element` | none | 303 | 303 |
| `niri_action` | `WindowLayoutsChanged` at 12 | 213 | 264 |
| `shell_panel` open | `WindowFocusChanged` at 129 | 18 | 381 |
| `shell_panel` close | `WindowFocusChanged` at 260 | 305 | 512 |

niri sets a window's focus timestamp once it has kept focus for a while, so that quick switching doesn't mark the windows passed through as recently used. Here that came about 750 ms after `focus` gave the fixture focus, which in all 30 rounds fell inside `type_text`'s report and restarted its 250 ms. The report shows no focus timestamps, so that wait bought nothing. Since this branch only an event that changes something the report shows restarts it (`act::record`), and `type_text` ends at 250 ms like `click` and `key`. The panel's focus changes do show in the report, so `shell_panel` keeps its wait.

## No quiet wait for `focus` and `niri_action`

The user chose to skip the quiet wait for `focus` and `niri_action` once their own wait ended with `focused`, `changed` or `closed`, or after `unchanged`'s full second, and always for `niri_action`'s `Spawn` and `SpawnSh`. The same `make nested-latency` rounds, against a release build of that change, compared with the two runs above it:

| Run | Build | Run id |
|---|---|---|
| skip 1 | this branch, skipping | 1791835605-980156 |
| skip 2 | this branch, skipping | 1791835698-983902 |

| Call | after 1 | after 2 | skip 1 | skip 2 |
|---|---|---|---|---|
| `focus` on the empty workspace | 281 (300) | 280 (284) | 41 (49) | 31 (51) |
| `focus` on the fixture | 253 (257) | 253 (257) | 64 (64) | 64 (64) |
| `click` on the entry, by element | 252 (253) | 252 (253) | 283 (284) | 283 (299) |
| `type_text` `"ab"` | 260 (262) | 260 (262) | 252 (253) | 252 (253) |
| `key` `BackSpace` twice | 252 (253) | 252 (253) | 260 (262) | 260 (276) |
| `activate_element` | 304 (306) | 304 (305) | 304 (305) | 304 (306) |
| `niri_action` `SetWindowHeight` | 261 (263) | 261 (263) | 213 (215) | 212 (215) |
| `shell_panel` open | 382 (398) | 382 (383) | 382 (383) | 382 (383) |
| `shell_panel` close | 512 (514) | 512 (528) | 512 (527) | 512 (514) |

Every call came back with the same `observed` as before in all 30 rounds. A round of the nine calls took about 2,750 ms before and 2,300 ms after: `focus` saves about 190 to 250 ms a call, `niri_action` about 50 ms, the rest of its own 200 ms wait for a resize in steps.

`click` became 31 ms slower. The timing marks, on the skipping build (run 1791836047-1060338), show why: after the workspace switch, niri answered every request about 30 ms late for a while. `focus` on the fixture paid 31 ms in its gate, and `click`, right after it, 31 ms in its gate and 64 ms in its work, which aims and sends the click through niri, against 0.3 and 2 ms before. The quiet wait of `focus` used to take that time. `click`'s quiet wait counts from the start of its work, so it covers most of the slower work, and only the gate's 31 ms is added. `key` and `type_text` moved 8 ms each, in opposite directions. niri's `WindowFocusTimestampChanged`, about 750 ms after the focus, now comes during `key` instead of `type_text`, and still restarts nothing; the 8 ms weren't traced further.

## After the review fixes

Opus's review of the branch led to two changes in the report and two in the harness. A report that skips the quiet wait still reads the rest of niri's batch, until no event came for 10 ms, at most 50 ms, so that the urgency niri clears on a window that took focus is in `focus`'s `changes`, and the quiet wait applies events already received before it checks the time. `make nested-latency` now also counts which `changes` keys each call returned, and picks the empty workspace below the fixture's, on the same output. The same rounds:

| Run | Build | Run id |
|---|---|---|
| before 3 | `8aac368` | 1791837760-1422393 |
| fixes 1 | this branch, after the review | 1791838058-1475861 |
| fixes 2 | this branch, after the review | 1791838333-1522340 |

| Call | before 3 | skip 1 | skip 2 | fixes 1 | fixes 2 |
|---|---|---|---|---|---|
| `focus` on the empty workspace | 280 (298) | 41 (49) | 31 (51) | 42 (59) | 42 (59) |
| `focus` on the fixture | 252 (256) | 64 (64) | 64 (64) | 64 (64) | 64 (64) |
| `click` on the entry, by element | 252 (253) | 283 (284) | 283 (299) | 272 (274) | 272 (273) |
| `type_text` `"ab"` | 497 (500) | 252 (253) | 252 (253) | 253 (253) | 252 (253) |
| `key` `BackSpace` twice | 258 (260) | 260 (262) | 260 (276) | 260 (261) | 260 (262) |
| `activate_element` | 304 (305) | 304 (305) | 304 (306) | 304 (305) | 304 (306) |
| `niri_action` `SetWindowHeight` | 262 (265) | 213 (215) | 212 (215) | 223 (225) | 223 (225) |
| `shell_panel` open | 382 (383) | 382 (383) | 382 (383) | 382 (382) | 382 (383) |
| `shell_panel` close | 512 (513) | 512 (527) | 512 (514) | 513 (519) | 512 (514) |

In all three runs every call came back with the same `observed` and the same `changes` keys in all 30 rounds, and the two builds agreed: `focus+workspace` for both `focus` calls, `windows` for `niri_action`, `focus` for both `shell_panel` calls, and none for `click`, `type_text`, `key` and `activate_element`, whose effects in the fixture niri doesn't report. `niri_action` pays the batch's 10 ms: its own wait ends after 200 ms without an event, and the report then waits 10 ms more for one. `focus` on the fixture didn't move, and `focus` on the empty workspace stayed within the spread of the skip runs. `click` came back 11 ms faster than in the skip runs; that wasn't traced.

## What is left

What remains for most calls is the quiet wait itself, by design: it gives an app 250 ms from the start of the action to answer, so that a dialog a click opens is in `changes`. The time each call spends in it, at the median, from the timing marks of the two builds and the runs above:

| Call | Quiet wait | Of its round trip |
|---|---|---|
| `click` | about 250 | over 90% |
| `key` | about 177 | 70% |
| `type_text` | about 138 | 53% |
| `shell_panel` open | about 363, from focus moving to the panel at 129 | 95% |
| `shell_panel` close | about 206, from focus coming back at 260 | 40% |
| `focus` | none once focus moved; 10 to 50 ms for the rest of niri's batch | not measured |
| `niri_action` | none once its window changed or a second passed; 10 ms for the rest of niri's batch | 4% |
| `activate_element` | none: its own 300 ms look covers it | 0% |

`screenshot: true` replaces the quiet wait with the screenshot's own settle, which looks 50 ms after the action and then every 100 ms until two captures match.
