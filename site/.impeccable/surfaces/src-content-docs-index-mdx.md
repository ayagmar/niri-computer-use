---
version: 1
slug: "src-content-docs-index-mdx"
primary_target: "src/content/docs/index.mdx"
related_targets: ["src/components/Hero.astro","src/components/Header.astro"]
---

# Landing page (site home)

Scope: the Starlight splash home page, `src/content/docs/index.mdx`, and the components it uses in `src/components/` (Head, Header and Hero overrides, Recording, Calls, Demo, Workspace). Visitor mode: Persuade.

Audience and job: a niri user with an MCP coding agent (Claude Code, Codex, Pi or any stdio MCP client) deciding whether to let it act on their desktop. Action: install from source; secondary, read the safety model. Proof: real-desktop recordings (a real Claude Code session, nothing scripted, thinking time between calls shortened, calls in real time) with their calls as text, and measured numbers from `docs/results/`. Constraints: Starlight search, theme switch and llms.txt stay; Lighthouse mobile Performance ≥ 95, Accessibility 100, Best Practices ≥ 95, SEO 100, LCP < 1.8 s, CLS < 0.05, TBT < 100 ms; no client framework, small deferred scripts only; dark and light; reduced motion means no autoplay. Recordings swap by replacing `public/demo/<name>.{webm,mp4,png,json}`.

## Direction contract

THESIS: The page is a niri screen: a bar, then a strip of windows with one focused, and every claim is a call with the answer that came back. Refuses the split hero (copy left, product right, cards below) and the centred headline over a terminal mock.

OWN-WORLD: Graphite (#121416 dark / #F3F4F5 light) windows with 1px rules and niri's 16px gaps; one amber focus ring (#F2A33A, deepened in light) on the focused window and, as the page scrolls, on the focused column of the section in view; red only for refused or stopped answers. Archivo for prose and headings, JetBrains Mono for calls, keys and geometry. A bar like a desktop's: workspace pills for the sections, a lease tag, the stop key as keycaps.

STORY: The visitor sees an agent run a real errand on a real niri desktop, watches each call light up with its accepted/observed answer while the bar's lease tag goes held and free, meets the stop key before scrolling, then reads tools, the gate, safety, limits, and copies the install steps.

FIRST VIEWPORT: Under the bar, full-bleed: the focused promise window (≈33vw, ring, headline, tagline, Install from source, How the safety works, requirements line); a column of two windows (the errand recording on its plate, then the agent's calls following the video); a stop-key window running off the right edge below ~1900px wide, scrollable sideways. Mobile: stacked in that order.

FORM: Desktop at page scale, the dealt lead of the surface roll (my list #4 of 7). seed: da34d443

Signature interaction: the hero recording drives the call log (the call shown wears the ring, later calls dim) and the bar's lease tag; the ring and the workspace pill follow the section in view by scroll timeline. All off under reduced motion except the ring's position.

FINISH: unreviewed and undocumented is unfinished; this build ends with the finish review, the verdict, DESIGN.md, and every shipping raster carrying its provenance
