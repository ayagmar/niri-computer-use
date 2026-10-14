# Chromium fixture results

What text typed and pasted through the server arrives as in a Chromium text area, measured with `make nested-chromium` on branch `chromium-fixture`. `docs/development.md` describes the suite.

Environment: Rust 1.99.0; niri 26.04 (8ed0da4); AMD Ryzen 9 9950X3D, 32 threads; debug build of the server. Every run below ran in the private headless cage, with one `winit` output at scale 1, the nested niri's default US layout, and the nested Noctalia as the lock source. The browser's profile, `HOME` and XDG directories were under `TEST_DIR`. No run sent input to the host. Every run reported C1's host snapshot unchanged, no entries from its processes in the host's journal, and no leftover processes, and nothing named after Chromium or Helium changed under `~/.config`, `~/.local/share`, `~/.cache` or `~/.pki`. Runs 1 to 4 did leave the browser's singleton directory, `/tmp/org.chromium.Chromium.*`, in the host's `/tmp`: the browser had no `TMPDIR` then. Run 5 sets it to a directory under `TEST_DIR`, and no new `/tmp/org.chromium.Chromium.*` appeared during it. The four directories runs 1 to 4 left were removed after checking that nothing listened on their sockets.

## Commands

```sh
make nested-chromium
make nested-chromium SHARED=1
NCU_CHROMIUM=/path/to/chrome make nested-chromium
```

The first two ran as `flock ~/.cache/ncu-nested.lock make …`.

Runs 1 to 4 ran at `9ab28ed`. Run 5 ran after the review fixes. By then the browser's path was kept as found rather than resolved, it had its own `TMPDIR`, a result had to stay the same for a second even when it was exact, and the emoji case passed only on its known cut text. Each call waiting out that second took the run from about 30 s to 53 s.

| Run | Browser | Mode | Run id | Result |
|---|---|---|---|---|
| 1 | Helium 0.19.2.1 (Chromium 155.0.8059.39), `helium-browser` on `PATH` | standalone | 1792003737-4052612 | pass |
| 2 | Helium 0.19.2.1 (Chromium 155.0.8059.39) | standalone | 1792003767-4056416 | pass |
| 3 | Helium 0.19.2.1 (Chromium 155.0.8059.39) | shared | 1792003797-4059663 | pass |
| 4 | Google Chrome for Testing 153.0.8010.12, Playwright's `chromium-1243` through `NCU_CHROMIUM` | standalone | 1792003652-4046473 | pass |
| 5 | Helium 0.19.2.1 (Chromium 155.0.8059.39), run as `/usr/bin/helium-browser` | standalone | 1792005043-4178932 | pass |

All five logged the same line for every case.

## What arrived

Typed with `type_text`, which always uses the server's own keyboard, and pasted with `via: "paste"` and `ctrl+v`.

| Case | Text | Typed | Pasted |
|---|---|---|---|
| ASCII | ``Hello, world! 0123456789 ~`{}\|<>?`` | exact | exact |
| the C10 corpus | `harness/corpus.txt`, 100 characters with `é`, `ß` and `→` | exact | exact |
| a period first | `. a. b, c` | exact | exact |
| a space first | ` leading space, then words` | exact | exact |
| a period fourteenth | `abcdefghijklm.n` | exact | exact |
| a space fourteenth | `abcdefghijklm n` | exact | exact |
| three symbols the layout lacks | `ab é ab ß ab → ab` | exact | exact |
| fourteen symbols the layout lacks | `a αβγδεζηθικλμνξ ok` | exact | exact |
| thirty symbols the layout lacks, in parts | `αβγδεζηθικλμνξοπρστυφχψω←↑→↓↔↕` | exact | exact |
| accented Latin | `café naïve façade Ærø` | exact | exact |
| Cyrillic | `Привет, мир` | exact | exact |
| emoji | `ok 🤖 🌙 done` | `ok 烙 \u{f319} done` (U+F916, U+F319), the known loss the case expects | exact |

The emoji arrived as the lower 16 bits of their code points, U+1F916 as U+F916 and U+1F319 as U+F319, as the desk probe in Vesktop found. Their keydowns carried the whole emoji as `key`, so the cut happens after the key event.

## Which keycodes the symbols used

The page logs each keydown's DOM `code`. Symbols the layout lacks went out on `IntlRo` (`<AB11>`, evdev 89), `IntlYen` (`<AE13>`, evdev 124), then `F13` to `F24` (evdev 183 to 194), in that order, and a text needing more than 14 started a new keymap at `IntlRo` again:

```text
Chromium type_text, three symbols the layout lacks: exact; keydowns "é" IntlRo, "ß" IntlYen, "→" F13
Chromium type_text, fourteen symbols the layout lacks: exact; keydowns "α" IntlRo, "β" IntlYen, "γ" F13, "δ" F14, "ε" F15, "ζ" F16, "η" F17, "θ" F18, "ι" F19, "κ" F20, "λ" F21, "μ" F22, "ν" F23, "ξ" F24
Chromium type_text, thirty symbols the layout lacks, in parts: exact; keydowns "α" IntlRo, "β" IntlYen, "γ" F13, "δ" F14, "ε" F15, "ζ" F16, "η" F17, "θ" F18, "ι" F19, "κ" F20, "λ" F21, "μ" F22, "ν" F23, "ξ" F24, "ο" IntlRo, "π" IntlYen, "ρ" F13, "σ" F14, "τ" F15, "υ" F16, "φ" F17, "χ" F18, "ψ" F19, "ω" F20, "←" F21, "↑" F22, "→" F23, "↓" F24, "↔" IntlRo, "↕" IntlYen
Chromium type_text, emoji: known loss (Chromium cuts characters outside the Basic Multilingual Plane to 16 bits), received "ok 烙 \u{f319} done" (U+F916 U+F319); keydowns "🤖" IntlRo, "🌙" IntlYen
```

## Checks of the suite itself

- With `PATH` holding no browser and `NCU_CHROMIUM` unset, `harness run --chromium` logged `Chromium fixture: skipped, no Chromium-based browser; set NCU_CHROMIUM to one or put chromium on PATH` and `result: skipped`, and exited 0.
- With `NCU_CHROMIUM=/nonexistent` it failed with `NCU_CHROMIUM=/nonexistent is not an executable file`.
- With `NCU_CHROMIUM` set to a script that answers `--version` and otherwise exits 3, the run failed after 6 s rather than at the 30 s window wait, with `the browser ended before its window opened: env failed with exit status: 3; output in …/target/e2e/1792005020-4176850/chromium.log`, and no leftover processes.
- With the emoji case marked as promised to arrive exactly, a Helium run failed with `Chromium fixture: promised text arrived otherwise: emoji typed as "ok 烙 \u{f319} done" (U+F916 U+F319)`, after logging every case.
