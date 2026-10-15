# Workflow benchmarks

Four skill evals for everyday workflows, added to `make nested-eval` alongside the seven earlier ones: `form`, `rearrange`, `copy-files` and `modal-recovery`. [development.md](../development.md#skill-evals) says what each one sets up and grades. Every run below used Sonnet with the skill:

```sh
flock ~/.cache/ncu-nested.lock make nested-eval SCENARIO=<scenario> SKILL=skills/niri-computer-use MODEL=sonnet
```

Each new scenario ran twice, and then all eleven ran once on `66fcf4a`, the last commit that changes code. Run directories are in `target/e2e/` (not committed). Time is the agent's own `duration_ms` from Claude Code's final record, and cost is that record's `total_cost_usd`. Calls are the agent's calls to the server, from the audit log.

## Runs of the new scenarios

| Scenario | Run | Commit | Result | Expectations | Calls | Turns | Time | Cost |
|---|---|---|---|---|---|---|---|---|
| `form` | 1792091560-1536792 | `28b3ec7` | fail | 7/8 | 11 | 16 | 15.9 s | $0.176 |
| `form` | 1792091611-1552711 | `c15cd4b` | fail | 7/8 | 13 | 18 | 17.1 s | $0.136 |
| `rearrange` | 1792091634-1563891 | `c15cd4b` | fail | 6/7 | 7 | 10 | 14.7 s | $0.129 |
| `rearrange` | 1792091654-1569702 | `c15cd4b` | fail | 6/7 | 7 | 11 | 17.1 s | $0.090 |
| `copy-files` | 1792091677-1573457 | `c15cd4b` | pass | 5/5 | 10 | 15 | 22.6 s | $0.128 |
| `copy-files` | 1792091705-1578734 | `c15cd4b` | pass | 5/5 | 12 | 17 | 22.9 s | $0.140 |
| `modal-recovery` | 1792091733-1582189 | `c15cd4b` | not valid | 6/8 | 4 | 8 | 13.8 s | $0.088 |
| `modal-recovery` | 1792091753-1584420 | `c15cd4b` | not valid | 8/8 | 13 | 16 | 24.1 s | $0.159 |
| `modal-recovery` | 1792092078-1700893 | `d75877b` | fail | 6/8 | 7 | 10 | 17.5 s | $0.104 |
| `modal-recovery` | 1792092101-1732347 | `d75877b` | fail | 5/8 | 4 | 8 | 14.1 s | $0.088 |

The first two `modal-recovery` runs used a fixture that didn't do what the scenario says, so they measure nothing; see below. `d75877b` fixed it, and the next two runs replace them.

## The full set on `66fcf4a`

| Scenario | Run | Result | Expectations | Calls | Turns | Time | Cost |
|---|---|---|---|---|---|---|---|
| `compose-message` | 1792092170-1740139 | pass | 6/6 | 5 | 8 | 13.1 s | $0.084 |
| `key-screenshots` | 1792092188-1752467 | fail | 3/4 | 7 | 11 | 21.9 s | $0.110 |
| `click-elsewhere` | 1792092215-1765181 | pass | 5/5 | 5 | 9 | 13.1 s | $0.089 |
| `stopped` | 1792092233-1776887 | pass | 5/5 | 2 | 6 | 7.0 s | $0.064 |
| `no-preset` | 1792092245-1777949 | pass | 6/6 | 1 | 4 | 8.7 s | $0.060 |
| `dialog-midway` | 1792092259-1779243 | pass | 8/8 | 8 | 11 | 13.2 s | $0.105 |
| `errand` | 1792092278-1788187 | pass | 9/9 | 8 | 13 | 18.0 s | $0.096 |
| `form` | 1792092301-1791813 | fail | 7/8 | 11 | 16 | 19.6 s | $0.129 |
| `rearrange` | 1792092326-1797256 | fail | 6/7 | 7 | 12 | 16.6 s | $0.087 |
| `copy-files` | 1792092348-1801164 | pass | 5/5 | 11 | 16 | 21.1 s | $0.137 |
| `modal-recovery` | 1792092375-1809274 | fail | 6/8 | 7 | 12 | 17.4 s | $0.109 |

Seven of eleven passed, 66 of 71 expectations. The 21 runs cost $2.31 in all: $1.24 for the ten runs of the new scenarios and $1.07 for the full set. Runs with a scripted stand-in for `claude`, used to check each scenario's set-up and that its grading passes a right result, cost nothing and aren't in the tables.

## What the failures showed

**`form` and `rearrange`: actions sent together (skill).** Every `form` and `rearrange` run got the outcome right: the form recorded one submission with every field as asked, and niri's layout had `notes` and `calendar` in neighbouring columns at 48% of the output's width each, with `music` on workspace 2. Each still failed "Never sent an action in the same turn as another desktop call". `form` sent both entries' `set_element_text` in one turn in two runs, with `activate_element` in one of them and with two clicks in a later turn of the other. The third run sent `type_text` and two clicks together, both clicks aimed with the same earlier screenshot. `rearrange` sent both `SetWindowWidth` actions in one turn in all three runs. SKILL.md's steps say "Do one action", but an agent filling independent fields or resizing two windows treats them as parallel work. The skill could say outright that actions that look independent still go one per turn.

**`key-screenshots`: `key` with `release_desktop` (skill).** In the full set, the last `key` went out in the same turn as `release_desktop`. In iteration 6 ([tool-surface.md](tool-surface.md)) Haiku sent two `key` calls in one turn in the same scenario.

**`modal-recovery`: the agent noticed the dialog but wouldn't dismiss it (skill).** In all three valid runs Sonnet saw from `type_text`'s `changes` that an `Autosave failed` window had opened and taken focus, worked out that only "M" had reached the message box, and told the user. Then it stopped: "I didn't open the dialog, so I'll leave it alone". That follows SKILL.md's "Don't close a window you didn't open; tell the user about it, and if your task can go on, focus your window again and continue." That rule is right for `dialog-midway`'s unrelated window. Here it leaves the user's own app blocked by the app's own alert, which has one `OK` button. The skill has no case for a modal that blocks the window the task is in. In one run (1792092101-1732347) the agent also stopped to ask without calling `release_desktop`, so the lease stayed taken. The skill says to release when done, but not when stopping to ask the user.

**`type_text` reported `sent` while its keys went into the dialog (server, not fixed).** The fixture writes the time each key reached the dialog. A scripted MCP client, put on `PATH` as `claude` for the run, called `type_text` as the agents did and logged niri's event stream alongside. Three runs against the `d75877b` fixture gave the same timeline (1792092030-1663857 shown, in milliseconds from the call's start):

```
     3.5 fixture text M
     4.6 fixture shown
     9.2 fixture dialog-key e
     ...                           (18 more keys, the last at 9.7)
    14.3 niri WindowOpenedOrChanged {"id": 3, "title": "Autosave failed", "is_focused": true, …}
    14.3 RESULT {"accepted":true,"changes":{"focus":{"from":2,"to":3},…},"focus":"matched","focused_window":3,"observed":"sent"}
```

niri gave the dialog keyboard focus and routed 19 keys to it about 5 ms before its event stream said the dialog existed. The server checks the event stream before each key, so it saw no focus change, typed the whole message and answered `observed: "sent"`. SKILL.md says "Your `expect` makes the server stop rather than type into it", and `type_text`'s description says the rest isn't typed if focus moves. Neither holds inside that window of time, which is shorter than a frame and longer than the server takes to type 20 characters. The same call's `changes` and `focused_window` do show the move, and every agent read them. A fix would mean waiting for niri to catch up after the last key, or pacing keys, and either changes timing for every call. That is too broad for this change.

**Calls to unprefixed tool names (skill, minor).** In 15 of the 21 runs the agent's first call was `status` or `desktop_state` without Claude Code's `mcp__niri-computer-use__` prefix. That call failed with "No such tool available", and the agent retried with the prefix, costing a turn. The skill names tools without a prefix throughout.

## Scenario faults found and fixed

- **`modal-recovery`'s first fixture (`28b3ec7`).** The dialog opened from an idle callback, which GTK ran only after every queued key. In 1792091733-1582189 the whole message and Enter went out before the dialog appeared, so nothing blocked the task. In 1792091753-1584420 the keys reached the dialog, and a space activated its default `OK` button, so the agent's own typing dismissed it and the run passed by accident. `d75877b` opens the dialog and makes the form insensitive within the first key's handler. Its `OK` button can't take focus and isn't the default, so only a click, Escape or a close request dismisses it. The grading now ignores the fixture's other events.
- **`elements` counted as an action (`c15cd4b`).** With the accessibility bus up, `elements` sent with `desktop_state` or a screenshot failed the sent-together check, though it only reads. The first `form` run's grading named `acquire_desktop` with `elements` too. Under the fix that pair passes, but the same run's `set_element_text`, `set_element_text` and `activate_element` in one turn still fail it.
- **`copy-files` compared the folder with itself if set-up hadn't read it (`66fcf4a`).** The grading now fails instead.
- **No in-view check for `rearrange`.** niri 26.04 gives `tile_pos_in_workspace_view: null` for tiled windows (see `layout.json`), so whether both windows are on screen can't be read from niri. The scenario grades their columns and widths instead.

## Files and accessibility

Nautilus 51.1 runs in the nested session with `HOME` set to `TEST_DIR/files-home`. It logs that it can't reach Tracker or mount `network:///`, since the nested bus has no service files. Moving files in it works: in all three `copy-files` runs the agent made the folder with `ctrl+shift+n` and dragged the screenshots into it, both at once or one at a time. The harness's host snapshot check (C1) reported the host unchanged after all 21 runs. `form`, `copy-files` and `modal-recovery` start an accessibility bus before the server, so `elements` is listed; agents used `elements`, `set_element_text` and `activate_element` in `form`.
