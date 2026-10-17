---
version: 1
slug: "src-pages-index-astro"
primary_target: "src/pages/index.astro"
related_targets: ["src/components/home","src/lib/home.ts"]
---

# Landing page (site home)

Scope: the standalone home page `src/pages/index.astro` with `src/components/home/` (Mark, Buttons) and its words in `src/lib/home.ts`, which `pages/index.md.ts` also serves as Markdown. The Starlight docs keep `src/styles/tokens.css` and `theme.css`; the home page shares none of it. Visitor mode: Persuade.

Audience and job: a niri user with an MCP coding agent deciding whether to let it act on their desktop. Action: install (commands on the page). Proof: one real recording, as a framed still that opens a 223 KB clip; the safety ledger; the release table from CHANGELOG.md. Constraints: light only, no JavaScript, quick to scan, nothing invented (no counters, guestbooks, awards, webrings).

## Direction contract

THESIS: A hand-made Linux project page from around 2000, done with care rather than as parody. Facts in tables and short lists instead of sentences. Refuses the marketing hero, gradients, cards and autoplay video.

OWN-WORLD: The desktop grey (#c0c0c0) around a 760-pixel white page with a 2px bevel. A teal title band (#336666) with white Tinos and a yellow stop line. A side column in face grey (#e4e4dc) with bevelled buttons that press in. Light teal heading bars. Sunken tables and code wells. Blue links, purple visited, red active. 88x31 buttons in solid era colours. A sunken-panel status bar.

STORY: The visitor reads what it is and that they hold a stop key, sees "niri 26.04 on Linux only", sees the real session in a framed still, scans four rows of facts, copies three install steps, reads what is enforced and what is not, and sees the last three releases.

FIRST VIEWPORT: The title band with the pixel mark (niri's columns, the focused one ringed, a pointer in it), the name and the tagline. The side buttons. The yellow niri-only notice, the framed still with its caption and clip size, and the facts table.

FORM: seed: none (unavailable). The user picked the late-90s project page from the phase-1 mocks; build-time What's new from CHANGELOG.md and clip facts from the MP4.

Signature interaction: the bevelled buttons and 88x31 buttons press in on :active. A print stylesheet drops the side column and prints outside links' addresses.

FINISH: finish review, fixes, DESIGN.md and design.json from the built page.
