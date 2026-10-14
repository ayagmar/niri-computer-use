---
version: 1
slug: "src-pages-index-astro"
primary_target: "src/pages/index.astro"
related_targets: ["src/components/home","src/styles/theme.css"]
---

# Landing page (site home)

Scope: the standalone home page `src/pages/index.astro` and its components in `src/components/home/` (Timing, Result, Gates, Code), with its words in `src/lib/home.ts`. The Starlight docs pages keep the concrete palette of `src/styles/tokens.css` and `src/styles/theme.css`; they share the fonts, the stored theme and the page crossfade. Visitor mode: Persuade.

Audience and job: a niri user with an MCP coding agent deciding whether to let it act on their desktop. Action: install (the commands are on the page). Proof: five real-desktop recordings, each with a timing diagram of its calls, a real action's answer explained field by field, and the table of checks that refuse an action. Constraints: dark and light; reduced motion is calm and complete; every part reads without JavaScript.

## Direction contract

THESIS: A datasheet for the server. The page shows what the server does the way a component datasheet would: a numbered figure of timing diagrams drawn from real sessions, the answer format, a numbered table of when an action is refused, limits, requirements, install. Refuses a marketing hero over a product shot.

OWN-WORLD: Paper and ink (#f4f5f2 and #121413 light, #0f1110 and #e9ebe6 dark), rules instead of panels (2px ink to open a section, 1px ink to head a table, 1px rule between rows), a highlighter yellow (mark) for what is held or pointed at, one red for the stop key and refused answers. Archivo Variable for text at 84% and 88% width for heads; JetBrains Mono for calls, commands, answers and their numbers. Small 3px corners. Figures and tables numbered in bold ink.

STORY: The visitor reads what it is and that one key takes it back, then watches Figure 1: a recording beside the prompt and the red Esc keycap, its timing diagram (Calls, Lease, niri saw, You) drawing in with it. Pressing the keycap or Esc stops the session at that moment and the diagram shows what the server answers from then on. Then what an action answers, the checks before anything reaches niri with Table 1, what it enforces and misses, what it needs, and three install steps.

FIRST VIEWPORT: The header with the wordmark, the version tag linking to the release, the running head, Docs, GitHub, the theme toggle and Install over an ink rule. Then the two-line headline with the stop line in red, the lede, Install and the link to how the stop key and lease work. Figure 1 starts below.

FORM: datasheet. Tabs are CSS radios; the diagrams are drawn at build time; each recording is a still with a `<noscript>` video until it starts.

Signature interaction: the timing diagram draws in step with the recording, which plays muted once 60% of it is in view (not under reduced motion or Save-Data); the stop key cuts the running call, refuses later actions, leaves later looks answered, drops the lease in red and writes the result in the readout, while the highlighter stroke draws under the headline's stop line.

FINISH: unreviewed and undocumented is unfinished; this build ends with the finish review, the verdict, DESIGN.md, and every shipping raster carrying its provenance
