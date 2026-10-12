# Accessibility gaps results

Three open questions from M9b, on branch `a11y-gaps` from `main` at `c3a7e0a`: GTK 4 entries that hide their text, lists that reuse a row's objects for another record outside GTK 4, and the last gate's 250 ms bound.

Environment: Rust 1.99.0; niri 26.04; cage 0.3.1; at-spi2-core 2.62.0.1; GTK 4.24.1 and GTK 3.24.52 with PyGObject 3.58.1; Qt 6.12.0 (`qml6`, qt6-declarative 6.12.0). No dependency was added.

Every nested run below ran in the private headless cage, with its own session bus and accessibility bus under `TEST_DIR`, pinned to four cores and holding `~/.cache/ncu-nested.lock`. No run read the host's accessibility bus, sent input to the host, or read the host's clipboard.

## Acceptance

The nested runs used `target/debug/harness run` directly, with the flags the `make` targets pass, after one `cargo build`.

| Command | Run | Result |
|---|---|---|
| `make check` | final tree | pass: 337 server, 130 harness and 226 protocol tests (one ignored), in both protocol modes; formatting, Clippy, rustdoc, cargo-deny, cargo-machete, typos and ShellCheck pass |
| `harness run --scale 1 --a11y` | 1791769218-3946882, 1791771316-266585 | pass |
| `harness run --shared --scale 1 --a11y` | 1791769856-4076742 | fail: C1, the host's `XDG_RUNTIME_DIR` lost `jb.station.sock`, a JetBrains IDE's socket, during the run; every server check passed |
| `harness run --shared --scale 1 --a11y` | 1791772607-661715 | pass |
| `harness run --scale 1 --ssd --a11y` | 1791770200-4157902 | pass |
| `harness run --shared --scale 1 --ssd --a11y` | 1791770597-64203 | pass |
| `harness run --scale 1.5 --a11y` | 1791771020-167433 | pass |
| `harness run --scale 1 --a11y`, eight busy loops on its four cores | 1791772869-750086 | pass |
| `harness run --scale 1 --input` | 1791771692-467850 | pass |
| `harness run --actions` | 1791772134-537352 | pass |
| `harness run --shell` | 1791772331-574087 | pass |
| `harness run --control` | 1791772505-633908 | pass |
| `harness run --scale 1 --a11y` with `main`'s server | 1791769572-4007289 | fail, as expected: an old ref to a reused Qt row was accepted (below) |

The input, actions, shell and control suites run no element action, so their audit logs have no last gate to measure.

## Entries that hide their text

GTK 4.24.1's source gives an entry the role `password_text` only for the input purposes `PASSWORD` and `PIN` (`gtk_accessible_is_password_text`), and `gtk_text_set_visibility` changes no accessible property. The nested runs read the fixture's `Hidden entry` (`Gtk.Entry(visibility=False)`) beside its `Plain entry` with `busctl`:

| | GTK 4 hidden entry | GTK 4 plain entry |
|---|---|---|
| role | `text` (61) | `text` (61) |
| states | `[1124075648, 1024]` | `[1124075648, 1024]` |
| interfaces | Accessible, Component, Text, EditableText, Action | the same |
| attributes | `toolkit: GTK` | the same |
| default text attributes | 22, `invisible: false` among them | the same |
| actions | `Activate` | `Activate` |
| text | ten `●` for ten synthetic characters | empty |

Only the dots tell it apart, and an empty field shows none, so `secret_field` doesn't refuse it and the docs say so. The harness now requires every row above but the text to match, so the run fails if GTK starts marking such entries.

Qt Quick 6.12 gives a `TextField` in `NoEcho` mode the role `password_text`, as in `Password` mode, and its text reads empty. The Qt fixture's `Hidden entry` holds synthetic text in that mode, and every run refused `set_element_text` on it with `secret_field`, its change counter at 0. Qt drops the accessible name of a field in `Password` mode but keeps it in `NoEcho` mode.

## Lists that reuse their rows

The new Qt fixture, `harness/fixtures/qt-list.qml`, has three `ListView`s with `reuseItems: true`, of 200 records each: rows as plain layouts (`Record`), as unnamed list items (`Item`) and as list items named after their record (`Named`). The checks are the GTK 4 list's (`docs/development.md`).

What Qt does, from the bus snapshots:

- It reuses a row's objects in place. Once the third records were deleted, the label at `Record 003`'s path read `Record 023`, and likewise in the `Item` and `Named` lists; after `Jump`, the label at `Record 002`'s path read `Record 112`, and the next ones moved on alike; after a later delete, one went from `Record 020` to `Record 129`. The `Delete` buttons kept their paths and names.
- It creates and drops the accessible objects of rows' delegates as they move, so a row sits under a `filler` or `list_item` one moment and straight in the window's panel the next. Most old refs fail by index for that reason.
- Until it reuses a deleted row, it keeps the row's objects on the bus, in place, named as before, showing and visible. A press through a fresh ref to a row whose record was just deleted is accepted and reaches the delegate with no record, which the fixture records as `a row with no record`.

Each run's outcomes, the same in all eight runs of this branch's server:

| List | Fresh refs accepted, own record deleted | Old refs refused: moved | Old refs refused: beside changed | Old refs refused: not showing | Press reaching a held row |
|---|---|---|---|---|---|
| GTK 4 | 4 | 10 | 0 | 0 | 0 |
| Qt, three lists | 4 | 19 | 1 | 1 | 1 |

The refusal by the new check is the reused row: `Record 112`'s `Delete`, pressed again after a `Step` scrolled its row to another record. With `main`'s server, that press was accepted and reached a row that no longer showed `Record 112`, which failed the run. Nothing deleted another listed record in any run.

The press reaching a held row is the documented limit: a ref listed fresh, pressed once to delete its record and pressed again at once. Nothing on the bus tells the held row apart. The docs and the skill say to list again after a control that deletes something and to check the removal on a screenshot.

## The last gate's bound

Each element action's audit entry now has `gate`: each pass's time and the answers' age when the call went out. Over the eight runs of this branch's server, 92 element actions per run ran the gate, 736 in all; 86 per run went out, and the other 6 were refused inside the pass (5 `secret_field`, 1 `element_unmappable`). Refusals before the gate, such as `element_stale`, don't run it.

| Run | Second passes | `element_busy` | Pass ms, median | p95 | max | Share of 250 ms |
|---|---|---|---|---|---|---|
| default, 1791769218-3946882 | 0 | 0 | 2.2 | 3.4 | 4.5 | 1.8% |
| default, 1791771316-266585 | 0 | 0 | 3.8 | 10.8 | 19.7 | 7.9% |
| shared, 1791769856-4076742 | 0 | 0 | 2.2 | 3.2 | 4.2 | 1.7% |
| shared, 1791772607-661715 | 0 | 0 | 2.0 | 3.4 | 6.8 | 2.7% |
| SSD, 1791770200-4157902 | 0 | 0 | 2.5 | 4.1 | 6.0 | 2.4% |
| SSD and shared, 1791770597-64203 | 0 | 0 | 3.2 | 9.4 | 11.8 | 4.7% |
| scale 1.5, 1791771020-167433 | 0 | 0 | 2.3 | 6.2 | 18.0 | 7.2% |
| eight busy loops, 1791772869-750086 | 0 | 0 | 8.7 | 14.8 | 28.9 | 11.6% |

The bound stays at 250 ms. An action's answers are as old as its pass took, at most 28.9 ms here. A tighter bound would refuse slow passes without making fast ones fresher, and large apps on a real desktop weren't measured (`docs/decisions.md`).

## Not covered here

- Large apps on the user's desktop, such as Firefox or LibreOffice: how long their passes take, and whether their lists reuse rows in place. These need the real session.
- Qt Quick's held rows stay a gap: an old ref to a row deleted moments ago can reach it.
