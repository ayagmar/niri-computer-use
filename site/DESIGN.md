---
name: niri-computer-use
description: A niri screen at page scale. Graphite windows on a strip, one amber focus ring, and every claim shown as a call with the answer that came back.
colors:
  amber-ring: "#f2a33a"
  amber-high: "#ffd08a"
  amber-low: "#3a2a12"
  amber-ring-light: "#9a5a06"
  amber-deep-light: "#8a5300"
  amber-ink-light: "#4d2e00"
  amber-low-light: "#f6e3c4"
  stop-red: "#ff6b6b"
  stop-red-light: "#c62828"
  graphite-black: "#121416"
  graphite-panel: "#181b1e"
  graphite-rule: "#2a2e32"
  graphite-ring-off: "#3a3f44"
  graphite-gray-6: "#1c1f22"
  graphite-gray-5: "#2e3337"
  graphite-gray-4: "#555c62"
  graphite-gray-3: "#8b9399"
  graphite-gray-2: "#b4bbc0"
  graphite-gray-1: "#dde0e2"
  graphite-white: "#eef0f1"
  paper: "#f3f4f5"
  paper-panel: "#fbfbfc"
  paper-rule: "#d5d9dc"
  paper-ring-off: "#c9ced2"
  paper-gray-3: "#555d62"
  paper-gray-2: "#3a4145"
  paper-ink: "#15181a"
typography:
  display:
    fontFamily: "'Archivo Variable', ui-sans-serif, system-ui, sans-serif"
    fontSize: "clamp(2.2rem, 1rem + 2.6vw, 4.75rem)"
    fontWeight: 680
    lineHeight: 1.02
    letterSpacing: "-0.03em"
  headline:
    fontFamily: "'Archivo Variable', ui-sans-serif, system-ui, sans-serif"
    fontSize: "clamp(1.75rem, 1.2rem + 1.6vw, 2.6rem)"
    fontWeight: 660
    lineHeight: 1.08
    letterSpacing: "-0.025em"
  title:
    fontFamily: "'Archivo Variable', ui-sans-serif, system-ui, sans-serif"
    fontWeight: 660
    letterSpacing: "-0.01em"
  lead:
    fontFamily: "'Archivo Variable', ui-sans-serif, system-ui, sans-serif"
    fontSize: "clamp(var(--sl-text-base), 0.9rem + 0.3vw, var(--sl-text-lg))"
    lineHeight: 1.55
  body:
    fontFamily: "'Archivo Variable', ui-sans-serif, system-ui, sans-serif"
  call:
    fontFamily: "'JetBrains Mono Variable', ui-monospace, 'DejaVu Sans Mono', monospace"
    fontSize: "0.75rem"
    lineHeight: 1.6
  label:
    fontFamily: "'JetBrains Mono Variable', ui-monospace, 'DejaVu Sans Mono', monospace"
    fontSize: "0.7rem"
    fontFeature: "tnum"
rounded:
  pill: "999px"
  tag: "6px"
  keycap-bar: "5px"
  keycap: "8px"
  button: "8px"
  panel: "10px"
  plate: "12px"
  ring: "14px"
spacing:
  gap: "1rem"
  column: "1.25rem"
  window: "clamp(1.25rem, 0.9rem + 1vw, 2.25rem)"
  section: "clamp(3rem, 2rem + 3vw, 5rem)"
components:
  button-primary:
    backgroundColor: "{colors.amber-ring}"
    textColor: "{colors.graphite-black}"
    rounded: "{rounded.button}"
    padding: "0 1.15rem"
    height: "2.75rem"
  button-primary-hover:
    backgroundColor: "{colors.amber-high}"
  button-primary-light:
    backgroundColor: "{colors.amber-ring-light}"
    textColor: "#ffffff"
  button-primary-light-hover:
    backgroundColor: "{colors.amber-deep-light}"
  button-quiet:
    textColor: "{colors.graphite-white}"
    rounded: "{rounded.button}"
    padding: "0 1.15rem"
    height: "2.75rem"
  window:
    backgroundColor: "{colors.graphite-panel}"
    rounded: "{rounded.panel}"
    padding: "{spacing.window}"
  column:
    backgroundColor: "{colors.graphite-panel}"
    rounded: "{rounded.panel}"
    padding: "{spacing.column}"
  plate:
    backgroundColor: "{colors.graphite-black}"
    rounded: "{rounded.plate}"
    padding: "2px"
  workspace-pill:
    textColor: "{colors.graphite-gray-3}"
    rounded: "{rounded.pill}"
    padding: "0.15rem 0.6rem"
  workspace-pill-lit:
    textColor: "{colors.graphite-white}"
    rounded: "{rounded.pill}"
    padding: "0.15rem 1rem"
  lease-tag:
    textColor: "{colors.graphite-gray-3}"
    typography: "{typography.label}"
    rounded: "{rounded.tag}"
    padding: "0.15rem 0.6rem"
  lease-tag-held:
    textColor: "{colors.graphite-white}"
  keycap:
    backgroundColor: "{colors.graphite-black}"
    textColor: "{colors.graphite-white}"
    rounded: "{rounded.keycap}"
    padding: "0 0.8rem"
    height: "2.5rem"
  keycap-bar:
    textColor: "{colors.graphite-gray-2}"
    typography: "{typography.label}"
    rounded: "{rounded.keycap-bar}"
    padding: "0.05rem 0.4rem"
  call:
    typography: "{typography.call}"
    rounded: "{rounded.tag}"
    padding: "0.3rem 0.5rem"
---

# Design System: niri-computer-use

## Overview

**Creative North Star: "The Column in Focus"**

The home page is drawn as a niri screen. A bar runs across the top with the page's sections as workspace pills, a lease tag and the stop key. Under it, edge to edge, sits a strip of graphite windows split by niri's 16px gaps: the focused promise window wearing the amber ring, a column holding the recording above the agent's call log, and a stop-key window that runs off the right edge until the screen is wide enough for it. Below the strip each section is a workspace of columns, and as the page scrolls the ring moves to the focused column of the section in view while its pill in the bar lights, the way niri marks the window and workspace you moved to. Starlight supplies search, the sidebar on docs pages and the theme switch; the system lives in the theme tokens and the components layered on top.

Evidence is the decoration. The recordings are real Claude Code sessions on a real niri desktop, each with a note saying so and naming its output. The calls beside them are the requests and answers from the same run, written as `→ request` and `← answer · ms`; while the hero recording plays, the call it shows wears the ring, the calls still to come dim, and the bar's lease tag turns held and free with the run. There are no illustrations, gradient fills, glows or shadows. The page is dense and quiet, and color is spent only on focus, action and refusal.

Both themes are first class. Dark is graphite with a bright amber ring; light is pale paper with the amber deepened to a brown that holds contrast on white. The browser's own parts follow: text selection takes the low amber, the caret is amber, and scrollbars are gray on the page color.

**Key Characteristics:**
- Graphite windows and 1px rules on niri's 16px gaps, no shadows.
- One accent, amber, taken from the focus ring.
- Red only for an answer that refused or stopped.
- Archivo for prose and headings, JetBrains Mono for calls, keys, geometry and metadata.
- State shown the way niri shows it: a ring that moves, a pill that lights, a tag that goes held.

## Colors

Cool graphite neutrals with a single warm amber, and a red held back for refusals.

### Primary
- **Focus Amber** (amber-ring): the focus ring. It rings the promise window, the focused column of the section in view, the lit workspace pill, the lease tag while held, and the call currently playing. It also fills the primary button, sets the second line of the hero headline (the stop key's promise), colors the `→ ←` marks and the caret, draws the act path in the diagram, and outlines keyboard focus everywhere (2px outline, 2px offset). Starlight's accent uses the same value, so links and the active sidebar item share it.
- **Lit Amber** (amber-high): hover on the dark primary button.
- **Amber Wash** (amber-low): Starlight's low accent in dark, and the text selection background.
- **Deepened Ring** (amber-ring-light): the focus ring in the light theme, deepened so it reads on paper. Primary buttons carry white text on it.
- **Deep Amber** (amber-deep-light) and **Amber Ink** (amber-ink-light): Starlight's light accent and its high step; the deep amber is also the light primary button's hover. **Pale Amber** (amber-low-light) is the light low accent and selection.

### Secondary
- **Stop Red** (stop-red dark, stop-red-light light): only the answer line of a call that ended in an error, `stopped` or a refusal, mark included.

### Neutral
- **Graphite** (graphite-black): the dark page, the keycaps' face, and the recording plate's backing. The plate keeps this graphite in the light theme too, because the recordings are of a dark desktop.
- **Panel Graphite** (graphite-panel): window, column and diagram box fill in dark.
- **Rule Graphite** (graphite-rule): 1px rules between sections, window and column borders, the recording plate's border, workspace pill borders, the quiet button border, diagram box strokes, and the color the promise window's ring rises from as the page opens.
- **Unlit Column** (graphite-ring-off dark, paper-ring-off light): the unlit columns in the favicon. Paper Ring-off is also light gray 5.
- **Gray steps** (graphite-gray-6 to graphite-gray-1): Starlight's gray scale. Gray 5 draws scrollbars and the divider before the bar's icons; gray 4 draws keycap borders, the free lease tag's dashes and diagram wires; gray 3 is muted metadata (timestamps, milliseconds, the `asked` tag, recording notes and output, the requirements line, unlit pills, calls still to come, the user's own `$` lines); gray 2 is secondary text such as the tagline, section intros and answer lines.
- **Graphite White** (graphite-white): primary text and headings in dark.
- **Paper** (paper), **Paper Panel** (paper-panel), **Paper Rule** (paper-rule), **Paper Grays** (paper-gray-3, paper-gray-2) and **Ink** (paper-ink): the same roles in the light theme.

### Named Rules
**The One Ring Rule.** Amber means focus: what is in view, what is held, what acts, what to press. It never fills a panel, tints a background or decorates a heading for emphasis alone.

**The Refusal Red Rule.** Red appears only on an answer that refused or stopped. Nothing else on the site is red.

## Typography

**Display Font:** Archivo Variable (with ui-sans-serif, system-ui), preloaded so headings never shift
**Body Font:** Archivo Variable
**Label/Mono Font:** JetBrains Mono Variable (with ui-monospace, DejaVu Sans Mono)

**Character:** A firm, slightly condensed grotesque set tight at large sizes, beside a plain mono that carries everything the machine said and every key you press.

### Hierarchy
- **Display** (680, fluid 2.2rem to 4.75rem, 1.02, -0.03em): the home page promise only, in the focused window. Balanced wrapping; its second line drops to its own line in amber.
- **Headline** (660, fluid 1.75rem to 2.6rem, 1.08, -0.025em, max 40rem, balanced): one per home section, a plain sentence. The paragraph under it is gray 2 at the same 40rem.
- **Title** (660 on the stop window's heading at Starlight's h4 size; Starlight's h3 size elsewhere; -0.01em): window and column headings and docs subheads. Code inside h2 and h3 loses its chip and drops to 0.85em.
- **Lead** (fluid base to large, 1.55, max 34rem): the hero tagline, in gray 2.
- **Body** (Starlight defaults): prose at a 46rem content width on docs pages; Starlight's sm size inside the hero's windows.
- **Call** (mono 0.75rem, 1.6): request and answer lines, wrapped anywhere with a 1.5ch hanging indent so the marks stay in a column.
- **Label** (mono 0.7rem, tabular numbers): call timestamps, the lease tag, the bar's keycaps and the recording's output line. Mono at Starlight's xs size carries the `asked` tag and the "All N calls as text" toggle.

### Named Rules
**The Machine Speaks Mono Rule.** Anything the server, niri or a shell said or will be told (requests, answers, timings, geometry, commands, keys) is mono. Prose and headings are never mono.

## Layout

On docs pages Starlight's layout holds with a 46rem content width; the home page widens it to 96rem. Spacing follows niri's gap: 1rem between windows and columns, everywhere on the home page.

**The bar.** On the home page the header is a desktop bar: the site name, then from 78rem the six workspace pills (demo, tools, gate, safety, limits, install), then from 50rem the lease tag pushed right, the stop keycaps, search, and a gray divider before the social and theme controls. Below 50rem only the name and search remain. Docs pages keep Starlight's header.

**The screen.** The hero runs the full width of the window directly under the bar. Below 62rem its windows stack: promise, recording, calls (capped at 22rem tall and scrolling inside their window), stop key. From 62rem the strip runs sideways as niri's does: as tall as the view (between 38rem and 72rem), three columns at their own widths (the promise at 24 to 36rem around 33vw, the recording column at 34 to 68rem around 52vw, the stop window at 18 to 26rem around 24vw), centered, and scrolling sideways when wider than the window, which it is until about 2150px. The last window runs off the right edge rather than shrinking, and keyboard focus inside it scrolls it into view. The page itself never scrolls sideways.

**Sections.** Each section is opened by a 1px rule with fluid 3rem to 5rem vertical padding, a headline, an optional intro, then columns 2rem below. From 50rem columns sit side by side: equal columns auto-fit at a 15rem minimum, or a 3:2 split with the wide column first or last (each at least 15rem on its narrow side), where columns stand only as tall as their content. From 72rem a long list in a wide column reads in two text columns. Below 50rem everything stacks.

Calls lay out as a two-column grid: the timestamp spanning two rows on the left, request and answer stacked on the right.

The diagram slot holds an SVG of how a call travels, swapped for a mono list below 40rem; it is being redrawn and is not described further here.

**The Column Strip Rule.** New sections are workspaces on the strip: a rule above, a headline, then content or bordered columns with one focused. The hero screen is the only full-bleed band; there are no floating cards, centered heroes or split heroes with cards below.

## Elevation & Depth

Flat. No element uses a shadow. Depth comes from tonal layering (windows and columns one step off the page), 1px rules, and the amber ring drawn outside the thing it marks: a 2px outline 2px off the promise window, a 2px border 4px outside a focused column. The recording plate is the one recessed surface: graphite backing inside a 2px rule-colored border. Keycaps are the one place a border thickens, to 3px at the bottom in the stop window and 2px in the bar, which draws the key's travel without a shadow.

**The Ring, Not Lift Rule.** Focus and state are shown by a ring or a border color, never by raising something with a shadow.

## Shapes

Gently rounded rectangles throughout. Windows, columns, video and docs images use a 10px radius; a border drawn around a panel adds its offset to the radius (12px for the plate, 14px for a focused column's ring) so the curves stay concentric. Buttons and the stop window's keycaps use 8px, the bar's keycaps 5px, the lease tag and the current call's ring 6px, and workspace pills are fully round (999px). Dashes mean unheld or gated: the free lease tag and the diagram's gate are the only dashed outlines. Lines are 1px for structure and 2px for focus, rings and the plate.

## Components

### Buttons
Two buttons, both 2.75rem tall, semibold, 8px corners, with a 150ms color transition.
- **Primary:** amber fill with graphite text in dark; deepened amber with white text in light. Hover lightens it in dark and deepens it in light.
- **Quiet:** transparent with a 1px rule border and primary text. Hover darkens the border to gray 3.
- **Focus:** the global amber outline.

### Bar
- **Workspace pill:** 1px rule border, fully round, xs text in gray 3. While its section crosses the middle of the view (a scroll timeline on that section), the pill widens its padding from 0.6rem to 1rem, takes the amber border and white text. Hover whitens the text. Each pill links to its section.
- **Lease tag:** mono label, 6px corners, `lease free` in gray 3 inside a dashed gray 4 border. While the hero recording's run holds the lease, the border turns solid amber and the text reads `lease held by the agent` in white. It follows the run's `acquire_desktop`, `release_desktop` and `stop` lines.
- **Stop keycaps:** `Mod Shift Esc` as three small mono keys with 5px corners, gray 4 borders and a 2px bottom edge; hover whitens them. They link to the stop window.

### Window (hero)
- **Structure:** panel fill, 1px rule border, 10px corners.
- **Promise window:** fluid 1.25rem to 2.25rem padding. Display headline, then lead, the two buttons and a gray 3 requirements line pinned to the window's foot like a status line. Wears the ring as a 2px amber outline 2px off.
- **Recording window:** the plate with no window chrome, and under it the recording's note on the left and its output (`DP-1 · 2560×1440, shown at 1920×1080`) in mono on the right, both gray 3 xs.
- **Agent window:** the `asked` tag and prompt, then the full call log, which scrolls inside the window and keeps the current call about a third of the way down.
- **Stop window:** large keycaps (`Mod Shift Escape`, mono sm, 2.5rem tall, graphite face, gray 4 border with a 3px bottom edge, 8px corners), a title, and two short paragraphs in gray 2.
- **Opening:** the strip slides in from 2.5rem right over 520ms (ease-out-expo curve), then the promise window's ring rises from rule color to amber over 260ms. Both are off under reduced motion.

### Section (Workspace)
- **Structure:** a 1px top rule, fluid 3rem to 5rem vertical padding, headline and intro at 40rem.
- **Each section** publishes a named view timeline that its workspace pill and its focused column follow.

### Column
- **Corner Style:** 10px.
- **Background:** panel graphite or paper panel.
- **Border:** 1px rule.
- **Internal Padding:** fluid 1rem to 1.25rem.
- **Focused column:** one per section. A 2px amber ring, 4px outside, 14px corners, fades in and out (scaling from 0.99) while the section crosses the middle of the view. Under reduced motion it moves to the section in view without the fade. Without scroll timelines no ring shows.

### Plate (recording)
- The video sits on a graphite plate with a 2px rule-colored border, 2px padding and 12px corners; the video inside has 10px corners. There are no corner ticks or geometry line above it.
- A still shows first and stays until the video has a frame. The video plays muted and looped while 35% in view, only after the page has loaded, never under reduced motion or save-data, where it shows the still as its poster with controls. Stills below the first view load as they come near.
- Every video carries an `aria-label` describing what happens in it.

### Calls
- Each call: its time (`m:ss.s`), `→ request`, `← answer · 303 ms` with the milliseconds in gray 3. A command the user ran is `$ command` with a `#` line saying what it did, both in gray 3.
- While the recording plays, the call it shows wears a 1px amber ring with 6px corners (200ms transition), and the calls still to come drop to gray 3.
- **In a column (Demo):** under the plate, the `asked` tag and prompt, the one call the recording is about with its answer, the recording's note, and a disclosure, "All N calls as text", holding the full list.

### Diagram
The diagram slot. Its current drawing uses panel boxes with rule strokes, gray wires and an amber act path and dashed gate; it is being replaced.

## Do's and Don'ts

### Do:
- **Do** draw new home sections with the Workspace component and list them in the workspaces file, so the rule, the pill and the ring come with them.
- **Do** mark exactly one column per section as focused.
- **Do** show real requests and answers from a run, with their timings, in `→` and `←` notation, and say how each recording was made.
- **Do** keep the recording plate graphite in both themes.
- **Do** give every video an `aria-label` and its calls as text, and keep autoplay off under reduced motion.
- **Do** check every new color pair in both themes: body text at 4.5:1, ring and focus at 3:1.

### Don't:
- **Don't** add shadows, glows or gradient fills; depth is tone and rules.
- **Don't** use amber outside the One Ring Rule, or red for anything but a refusal or stop.
- **Don't** put a mono label or wordmark above a heading as an eyebrow.
- **Don't** show mocked output or a drawn terminal; recordings and transcripts come from real runs.
- **Don't** let anything but the hero's strip scroll sideways.
- **Don't** add a logo beside the name; the name is set in type, and the favicon's three columns are the only mark.
