# Accessibility gaps results

Three open questions from M9b, on branch `a11y-gaps` from `main` at `c3a7e0a`: GTK 4 entries that hide their text, lists that reuse a row's objects for another record outside GTK 4, and the last gate's 250 ms bound.

Environment: Rust 1.99.0; niri 26.04; cage 0.3.1; at-spi2-core 2.62.0.1; GTK 4.24.1 and GTK 3.24.52 with PyGObject 3.58.1; Qt 6.12.0 (`qml6`, qt6-declarative 6.12.0). No dependency was added.

Every nested run below ran in the private headless cage, with its own session bus and accessibility bus under `TEST_DIR`, pinned to four cores and holding `~/.cache/ncu-nested.lock`. No run read the host's accessibility bus, sent input to the host, or read the host's clipboard.

## Acceptance

The nested runs used `target/debug/harness run` directly, with the flags the `make` targets pass, after one `cargo build`. They ran after the row check was reworked (below), first on `main` at `c3a7e0a` and then, for `make check` and the a11y suite in both modes, once more after rebasing onto `main` at `dbfe457`. The rework was then replaced by the named-row rule (below), and `make check` and the a11y suite in both modes ran again on that.

| Command | Run | Result |
|---|---|---|
| `harness run --scale 1 --a11y` | label binding, 1791803457-1247990 | pass, `Labelled` outcomes as below, longest pass 4.7 ms |
| `harness run --shared --scale 1 --a11y` | label binding, 1791803593-1293178 | pass, `Labelled` outcomes as below, longest pass 12.4 ms |
| `make check` | named-row rule on `dbfe457` | pass: 356 server, 132 harness and 250 protocol tests (one ignored), in both protocol modes |
| `harness run --scale 1 --a11y` | named-row rule, 1791782877-3551010, 1791783323-3650663 | pass, list outcomes as below, longest pass 4.6 and 7.4 ms |
| `harness run --shared --scale 1 --a11y` | named-row rule, 1791783527-3701020 | pass, list outcomes as below, longest pass 12.4 ms |
| `make check` | final tree on `c3a7e0a` | pass: 343 server, 130 harness and 231 protocol tests (one ignored), in both protocol modes; formatting, Clippy, rustdoc, cargo-deny, cargo-machete, typos and ShellCheck pass |
| `make check` | rebased onto `main` at `dbfe457` | pass: 359 server, 132 harness and 249 protocol tests (one ignored), in both protocol modes |
| `harness run --scale 1 --a11y` | rebased, 1791779385-2743645 | pass, list outcomes as below, longest pass 4.2 ms |
| `harness run --shared --scale 1 --a11y` | rebased, 1791779657-2823436 | pass, list outcomes as below, longest pass 3.7 ms |
| `harness run --scale 1 --a11y` | 1791776887-1844269, 1791777660-2253830 | pass |
| `harness run --shared --scale 1 --a11y` | 1791777049-1958478 | pass |
| `harness run --scale 1 --ssd --a11y` | 1791777196-2026514 | pass |
| `harness run --shared --scale 1 --ssd --a11y` | 1791777362-2107619 | pass |
| `harness run --scale 1.5 --a11y` | 1791777527-2187449 | pass |
| `harness run --scale 1 --a11y`, eight busy loops on its four cores | 1791778592-2490785 | pass |
| `harness run --scale 1 --input` | 1791777916-2344326 | fail: `keeper-ending-1-ready`, the keeper's ready report not seen within 5 s, with the host's load average at 8.5; this branch doesn't touch the input suite |
| `harness run --scale 1 --input` | 1791778761-2543687 | pass |
| `harness run --actions` | 1791778364-2458974 | pass |
| `harness run --shell` | 1791778547-2484096 | pass |
| `harness run --control` | 1791778563-2486370 | pass |
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
- It puts no container around a list's rows. All three lists' rows sit in the window's content `filler`, beside `Step` and `Jump`: the first ten of each list wrapped in a `filler` (`Record`) or a `list_item` (`Item`, `Named`), the rest as bare label and button pairs. It creates and drops the delegates' accessible objects as they move, so a row is wrapped one moment and bare the next.
- Until it reuses a deleted row, it keeps the row's objects on the bus, in place, named as before, showing and visible.

With `main`'s server, a ref to `Record 112`'s `Delete`, pressed again after a `Step` scrolled its row to another record, was accepted and reached a row that no longer showed `Record 112`, which failed the run.

The first fix kept the objects between the element and its nearest look-alike elements, up to 16, and checked their names. It passed the nested runs, but a blind review reproduced three ways an old ref still acted on a row showing another record: a second button of the same name in the row, a row whose record label was past the 16th object, and a listing stopped by its limit, object cap or budget before the label. It also showed that a bare row's ref went stale when the next row's label changed, and that working out the rows took 11.8 s on a wide tree. Each is now a protocol test in `tests/protocol/elements.rs`, and the check was reworked to refuse what it can't pin down (`docs/decisions.md`).

The rework refused every Qt `Delete` ref, fresh or old, since no structural rule told where one of Qt's bare rows ends. A second blind review found it still unsound: a row holding two wrapped `Delete` buttons passed its inner wrapper off as the row, a list showing a single reused row looked like a control of its own, and named rows of different shapes, or with a changing elapsed time, were refused. Inferring a row from the tree's shape was dropped for a rule on what the bus states (`docs/decisions.md`): a ref is bound to its record by the element's nearest `list_item`, `table_row` or `tree_item` with a name. A control in an unnamed row, or outside any row while other elements of its role and name were listed, is marked `unbound` and refused with `element_unbound`. The GTK fixture gained a second list whose rows are labelled with their record through `GtkListItem:accessible-label`.

With the named-row rule, each run's outcomes, the same in the a11y runs listed above:

| List | Rows on the bus | Fresh refs accepted, own record deleted | Refused: `element_stale` | Refused: `element_unbound` | Deleted another record |
|---|---|---|---|---|---|
| GTK 4 `Record` | unnamed `list_item` | 0 | 0 | 8, `unnamed_row` | 0 |
| GTK 4 `Labelled` | named `list_item` | 4 | 10 | 0 | 0 |
| Qt `Record` | a `filler` for some rows, none for the rest | 0 | 0 | 8, `repeated` | 0 |
| Qt `Item` | an unnamed `list_item` for some rows, none for the rest | 0 | 0 | 8: 5 `unnamed_row`, 3 `repeated` | 0 |
| Qt `Named` | a named `list_item` for some rows, none for the rest | 1 | 4 | 3, `repeated` | 0 |

In every Qt list, the refs to rows Qt put on the bus without a row role are `repeated`. In the `Named` list, the third record's label was renamed in place from `Named 003` to `Named 023` once its row lost the named `list_item`, and its old ref was stale because its parent changed. A lone reused Qt row without a row role, or one whose look-alike elements `elements` didn't reach, still isn't caught, and the docs say so.

## Records named by a label

A later spike, on branch `correctness`, asked whether Qt 6 or GTK 4 states on the bus which record a `Delete` belongs to where its row has no row role and name. In a private session with its own accessibility bus, a probe read `GetRelationSet` for every object of a copy of the Qt fixture whose `Delete` buttons set `Accessible.labelledBy` to their row's label, and of a small GTK 4 window.

- Qt Quick 6.12 states it when the app asks. Each `Delete` had a `labelled-by` relation to its row's label, and the label a `label-for` relation back to that button alone: 117 buttons at the start, 132 after `Step` and 210 after `Jump`. When Qt reused a row in place, the label at the kept path was renamed with the record, 39 labels after `Jump` and 15 after the next `Step`, one from `Record 002` to `Record 128`, and after a delete the same button's label went from `Record 003` to `Record 040`.
- GTK 4.24 labels each button that has text by its own child label, one to one, whose name is the button's text. That names the control: a counter's label went from `Count: 0` to `Count: 1`.

So a ref is now bound through a label that labels its element alone and sits outside it (`docs/decisions.md`). The Qt fixture gained a fourth list, `Labelled`: plain rows, like `Record`, whose `Delete` sets `Accessible.labelledBy` to the row's label. The harness now lists each row's ref just before its press in the last step, since a delete moves the rows below it in Qt too. A first run with the label inside the element counted bound made the GTK `Primary` counter's ref stale after its first click, which is why a label inside the element binds nothing.

| List | Fresh refs accepted, own record deleted | Refused: `element_stale` | Refused: `element_unbound` | Accepted, reached a row with no record | Deleted another record |
|---|---|---|---|---|---|
| Qt `Labelled`, runs 1791803457-1247990 and 1791803593-1293178 (shared) | 4 | 8 | 0 | 2 | 0 |

Every other list's outcomes were as in the table above. Each stale refusal came from the lineage, since the row's objects moved among their parent's children; no old ref reached the label check in these runs. A lone reused row at a kept index, which the lineage passes, is caught by the label check in the protocol tests, with the label renamed or relabelling another control. Twice, `Labelled 112`'s ref, pressed again at once and once more after a `Step`, was accepted and reached the row Qt holds for reuse, which shows no record: Qt keeps a removed record's row on the bus, named as before, showing and visible, until it reuses it, so nothing on the bus tells it from a showing row. The fixture deleted nothing, and the harness now accepts that outcome only for a ref whose record is already gone. The result said `accepted` and `observed: present`, and an agent learns that nothing happened only from the screen. A named Qt row behaves the same way; in the earlier runs its refs had been refused as `repeated` before reaching it.

## The last gate's bound

Each element action's audit entry now has `gate`: each pass's time and the answers' age when the call went out. On the reworked server, 86 element actions per a11y run ran the gate; 81 per run went out, and the other 5 were refused inside the pass with `secret_field`. With the named-row rule, 87 ran it and 82 went out, the Qt `Named` list's accepted ref among them. Refusals before the gate, such as `element_stale` and `element_unbound`, don't run it.

| Run | Second passes | `element_busy` | Pass ms, median | p95 | max | Share of 250 ms |
|---|---|---|---|---|---|---|
| default, 1791776887-1844269 | 0 | 0 | 2.0 | 2.8 | 3.5 | 1.4% |
| default, 1791777660-2253830 | 0 | 0 | 2.7 | 8.0 | 30.7 | 12.3% |
| shared, 1791777049-1958478 | 0 | 0 | 2.1 | 3.3 | 4.0 | 1.6% |
| SSD, 1791777196-2026514 | 0 | 0 | 2.9 | 12.8 | 30.1 | 12.0% |
| SSD and shared, 1791777362-2107619 | 0 | 0 | 2.0 | 4.0 | 7.3 | 2.9% |
| scale 1.5, 1791777527-2187449 | 0 | 0 | 2.0 | 4.2 | 27.7 | 11.1% |
| eight busy loops, 1791778592-2490785 | 0 | 0 | 8.9 | 14.4 | 19.8 | 7.9% |
| named-row rule, default, 1791782877-3551010 | 0 | 0 | 2.0 | 3.0 | 4.6 | 1.8% |
| named-row rule, default, 1791783323-3650663 | 0 | 0 | 2.4 | 5.6 | 7.4 | 3.0% |
| named-row rule, shared, 1791783527-3701020 | 0 | 0 | 2.3 | 3.3 | 12.4 | 5.0% |

The first fix's eight runs, 736 gated actions, gave the same picture: no second pass, no `element_busy`, the longest pass 28.9 ms. The bound stays at 250 ms. An action's answers are as old as its pass took, at most 30.7 ms here. A tighter bound would refuse slow passes without making fast ones fresher, and large apps on a real desktop weren't measured (`docs/decisions.md`).

## Not covered here

- Large apps on the user's desktop, such as Firefox or LibreOffice: how long their passes take, and whether their lists reuse rows in place. These need the real session.
- Lists in other toolkits and apps: whether their rows have a row role and a name on the bus, which decides whether their refs bind or are `unbound`.
