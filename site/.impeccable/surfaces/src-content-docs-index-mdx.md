---
version: 1
slug: "src-content-docs-index-mdx"
primary_target: "src/content/docs/index.mdx"
related_targets: []
---

# Landing page (site home)

Scope: the Starlight splash home page, `src/content/docs/index.mdx`, and the components it uses in `src/components/`. Visitor mode: Persuade.

Audience and job: a niri user with an MCP coding agent (Claude Code, Codex, Pi or any stdio MCP client) deciding whether to let it act on their desktop. Action: install from source; secondary, read the safety model. Proof: the demo recording (scripted calls, real server results, nested niri) and measured numbers from `docs/results/`. Constraints: Starlight header, search, theme toggle and llms.txt stay; ≤150 KB first load without video, no extra JS beyond Starlight's own except a tiny inline script if a motion need can't be met in CSS; dark and light; reduced motion means no autoplay.

## Direction contract

THESIS: Every claim is a request with the event that answered it, laid on niri's strip of columns. Refuses the centred-headline-over-terminal-mock-plus-feature-cards page.

OWN-WORLD: Graphite panels (#121416 dark / #F3F4F5 light) split by 1px rules into columns with 16px gutters; one amber focus ring (#F2A33A, deepened for light) marks the column in view and the primary action; red only for stopped/refused. Archivo for headings and body, a mono for request/event lines with typed arguments (→ request, ← event). Media registered like plates: corner ticks carrying real layout geometry. The lease tag: dashed and unlit when free, amber while held.

STORY: The visitor sees an agent run a real errand on niri, reads the exact calls and their accepted/observed answers, sees the stop key and limits before installing, then copies four commands.

FIRST VIEWPORT: Two columns. Left (≈40%): wordmark eyebrow, the promise as a large headline, a subhead naming the stop key, "Install from source" (amber, primary) and "How the safety works" (quiet). Right (≈60%): the demo loop in a ringed column with the lease tag and plate ticks, the current request/event line set beneath it. Mobile: stacked, demo under the headline.

FORM: Brief-pinned column strip and focus ring (my list #1), with the dealt grammar from list #5: Wayland protocol request/event notation. Seed ab8f66a5. Raises: plate registration (design annual), ghost unlit lease state (seven-segment), caption traceability to audit timestamps (jacquard).

FINISH: unreviewed and undocumented is unfinished; this build ends with the finish review, the verdict, DESIGN.md, and every shipping raster carrying its provenance

Signature interaction: the amber ring moves to the column (section) in view, 200 ms transform-only; off under reduced motion.

Unresolved: none.
