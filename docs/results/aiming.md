# Aiming

On 2026-10-16 an agent misclicked often enough in a real session that it gave up on the desktop once. Claude Opus, driven by pi, browsed a router's web UI in Helium, a Chromium browser, on a 2560x1440 output at scale 1. Screenshots came back 1280 pixels wide, at `scale` 0.5. The server was 0.1.0 in shared mode. This page sorts every click that missed by cause. The evidence is the session's log, which has every call, result and image, and the server's audit log. It then lists what changed.

## The clicks that missed

The session made 55 clicks and one scroll. 19 clicks missed what the agent meant to hit. Age runs from the ref's `captured_at_unix_ms` to the call's start in the audit log. Positions are in image pixels, and the places of rows and tabs are as the ref's image shows them.

| # | Time (UTC) | Click at | Ref, age | Meant | Landed on | Cause |
|---|---|---|---|---|---|---|
| 1 | 00:54:55 | (600, 108) | `-1`, 4.8 s | Internet tab | blank margin of the browser page | stale ref: the browser was resized |
| 2 | 01:08:12 | (100, 208) | `-546`, 3.2 s | WLAN row, y 152 | the Universal Plug and Play row, y 206–221 | estimate |
| 3 | 01:08:16 | (100, 183) | `-560`, 2.8 s | WLAN row, y 152 | Routing row | estimate |
| 4 | 01:08:24 | (297, 177) | `-588`, 2.5 s | a "5GHz" tab | the page's description text | misread: there is no such tab |
| 5 | 01:08:33 | (230, 291) | `-602`, 7.4 s | WLAN Global Configuration, y 299 | the box above it | estimate |
| 6 | 01:08:57 | (199, 136) | `-658`, 6.7 s | WLAN Basic tab, x 236 | the tab strip's ◀ arrow | estimate |
| 7 | 01:09:02 | (185, 136) | `-672`, 3.8 s | WLAN Basic tab | the same arrow | estimate |
| 8 | 01:09:29 | (100, 205) | `-729`, 5.2 s | LAN row, y 167 | FTP row | estimate |
| 9 | 01:09:35 | (100, 180) | `-743`, 4.6 s | LAN row | Routing row | estimate |
| 10 | 01:09:44 | (100, 197) | `-757`, 7.3 s | LAN row | FTP row | estimate |
| 11 | 01:14:39 | (100, 301) | `-799`, 2.9 s | DNS row, y 276 | below the menu | estimate |
| 12 | 01:14:44 | (100, 297) | `-813`, 3.7 s | DNS row | USB row, y 283–299 | estimate |
| 13 | 01:14:51 | (100, 284) | `-827`, 5.5 s | DNS row | USB row | estimate |
| 14 | 01:38:13 | (355, 36) | `-892`, 12.5 s | the router's tab in the browser | the user's terminal | stale ref: the user focused their terminal |
| 15 | 01:38:33 | (282, 108) | `-907`, 6.0 s | Internet tab, x 350 | Topology tab | an x from an earlier screenshot |
| 16 | 01:38:51 | (100, 184) | `-936`, 3.4 s | Security row; the menu starts at x 145 | the page margin left of the menu | an x from an earlier screenshot |
| 17 | 01:39:16 | (289, 328) | `-979`, 7.3 s | Action drop-down, x 390 and right | the "Action" label | estimate |
| 18 | 01:49:51 | (620, 282) | `-1011`, 4.5 s | Type drop-down, y 283–294 | 1 pixel above it | estimate |
| 19 | 01:50:17 | (470, 184) | `-1041`, 9.6 s | Security row, y 168 | Parental Controls row, while a drop-down's list was open | estimate; the open list took the click to close itself |

Refs are `shot-f5ce5272-<n>`. Clicks 1 and 14 are the server's fault, and the other 17 are the agent's.

## Causes

**Coordinate mapping is right.** Every miss in the left-hand menus landed on the row that the image pixel times two gives. (100, 208) hit Universal Plug and Play, whose row spans 206–221. (100, 205) and (100, 197) hit FTP at 190–206, and (100, 180) hit Routing at 175–190. (100, 297) and (100, 284) hit USB at 283–299, and (100, 301) hit below the menu's box. Clicks the agent aimed right landed right at the same scale, among them the Internet tab at (282, 108) at 00:55:03 and the menu rows after it zoomed in at 01:39:03. The session used no region ref or zoom ref to click, and its one output had its origin at (0, 0). The nested M4 accuracy check covers fractional scales and a lowered capture scale within ±0.05 px.

**Stale refs (clicks 1 and 14), a server bug.** A ref only expired after 60 seconds or when its output changed, so a click went out after the windows on the output had moved.

- Click 1: `desktop_state` at 00:54:46 gave the browser, window 7, a tile 2540 wide, and the screenshot at 00:54:50.46 shows it filling the output. The click's own screenshot, 1.4 s after the click, shows it 1265 wide beside pi's terminal, window 9. The click result has no `changes`, so the resize happened before the click's own look at niri, in the 4.8 s since the capture. Nothing the agent sent resized it. (600, 108) had shown the Internet tab and was now blank page right of the menu bar. The agent said "The layout moved while I clicked."
- Click 14: the screenshot at 01:38:00.77 shows the browser focused. During the agent's 12.5 s turn the user focused pi's terminal, which niri showed across the output, and started typing a message. The click result says `focused_window: 9`, again with no `changes`, so focus had moved before the click. The press landed in the terminal. The agent said "Layout shifted under me and I clicked the wrong window" and released the desktop.

**The agent's estimates on a downscaled image (15 clicks: 2 to 13 and 17 to 19).** The menu rows are 15.5 image pixels apart, with text about 6 pixels high. The agent's notes assumed 28 or 29: "Status 123, WLAN 152, LAN 181, Routing 210". Then it blamed the screen: "the rows shifted again", "the menu coordinates have shifted". Clicks 4, 5, 6, 7, 17 and 18 missed a tab, header, arrow or drop-down by 1 to 100 pixels. When the agent zoomed into the menu at 01:38:58 it read the rows right away ("Rows are 14 px apart; Security sits at y≈168") and hit Security on the next click.

**Coordinates from an earlier screenshot (clicks 15 and 16).** By 01:38 the browser was 1520 wide, so the page's centred content sat about 63 image pixels further right than at 00:55. The agent clicked the x it had used before, 282 for Internet and 100 for the menu, though the fresh screenshot showed Internet at 350 and the menu starting at 145. In the image for click 15, a tab's hover card also covered the Internet tab.

**Ruled out: a view scroll or animation under the click.** No click changed focus to another window, so niri didn't scroll the view during any of them, and `changes` was absent from all 55 results. Both stale cases moved before the call began.

## Also found

**The screen never settled.** 56 of the 57 screenshots that came with an action say `settled: false`, each after the full 1.5 s, and a `wait_for` `screen_stable` with a 25 s timeout timed out. The settle wait compares the whole output, and pi's terminal on the same output showed a spinner the whole time. The pages had loaded in every after-image checked, so this caused no miss, but `settled` was useless on this desktop. It isn't fixed: comparing only the target window's part of the image would need its place, which niri doesn't give for tiled windows.

**Settle captures pushed the agent's refs out.** Ref numbers jumped by 14 per action with `screenshot: true`, and by 247 during the 25 s `wait_for`. Every capture of a settle wait kept a ref, and a lease keeps 64. At 00:58:50 the agent zoomed into `shot-f5ce5272-213`, the screenshot it had looked at last, 37 s old, and got `unknown_ref`.

## Changes

- `351d723` fix(refs): every screenshot ref keeps what niri showed on its output when the capture began: the overview, the output's workspace and that workspace's active window, and each window there with its layout. A pointer tool aimed at pixels fails with `ref_invalid` `layout_changed` when that differs as the call starts, and the detail says what moved. A title change, a window on another output, or focus moving to another output doesn't count. An aim at an `element` is placed again just before the gesture and skips this check.
- `8aa7917` fix(engine): a settle wait drops the ref of each capture once a later one replaces it, so only the capture it returns keeps a ref.
- `0bcc6f7` docs(skill): read each pixel off the latest image, not from an earlier screenshot, and zoom in for small targets such as menu rows. SKILL.md stays at 126 lines and is 12 bytes shorter.

niri reports no place for a tiled window, so the check can't see the view scrolled with nothing else changed, as after `CenterWindow` or a touchpad swipe. It also can't see changes inside a window, such as a page that reflows. A stale-looking ref can be refused when nothing under the pixel moved, such as a floating window clicked after focus moved to a tiled window under it. That costs the agent one screenshot.

## Tests

- Protocol `act::a_pixel_aim_is_refused_once_the_windows_its_screenshot_shows_moved`: a click after the active window changed and a scroll after a resize fail with `layout_changed`, and a click after only a title change passes the check. On the code before `351d723`, the first click reached the Wayland connect, failing with `upstream_error` instead of `ref_invalid`.
- Unit `refs::scene` (4 tests): what moves an output's scene, including the session's resize and focus change, and what doesn't.
- Nested M4 (`make nested-input`), new step after the clicks: the supervisor moves the floating `wev` 250 px with niri's `MoveFloatingWindow`, then clicks at the pixel that showed its centre. Before the fix the call answered `{"accepted":true,"focused_window":2,"observed":"sent"}`, with no `changes`, the same shape as clicks 1 and 14. After it, the call fails with `layout_changed: niri moved or resized window 2 since the screenshot` and nothing reaches `wev`. The whole M4 run passed with the fix.
- Protocol `wait::a_long_screen_wait_keeps_the_agents_refs`: a fake grim returns a different image each time, and after an 8 s `wait_for` both the screenshot taken before it and the one it returned can still be zoomed. Before `8aa7917` the earlier one failed with `unknown_ref`.
- Two protocol tests in `elements` reused a ref for a pixel click after they had opened the overview or moved the window. They now take a new screenshot first, since that click is refused.

`make check` passed after each commit, run with `ulimit -n 524288`.

## Evals

One run each with Sonnet and the skill on `0bcc6f7`, $0.33 in all:

```sh
flock ~/.cache/ncu-nested.lock make nested-eval SCENARIO=<scenario> SKILL=skills/niri-computer-use MODEL=sonnet
```

| Scenario | Run | Result | Expectations | Calls | Turns | Time | Cost |
|---|---|---|---|---|---|---|---|
| `form` | 1792118198-574950 | pass | 8/8 | 13 | 18 | 21.0 s | $0.148 |
| `click-elsewhere` | 1792118225-576954 | pass | 5/5 | 6 | 9 | 12.2 s | $0.091 |
| `errand` | 1792118242-578036 | pass | 9/9 | 8 | 11 | 16.2 s | $0.092 |

No call was refused with `layout_changed`. `form` still aimed its first two clicks with one screenshot, at a check box and a drop-down in a window that didn't move, which the check lets through. Its ref numbers went up by 2 for each click with `screenshot: true`, where the session's went up by 14, because the settle wait no longer keeps a ref for every capture.
