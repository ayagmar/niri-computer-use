---
name: niri-computer-use
description: A datasheet on paper and ink, with a highlighter for what is held or pointed at, one red for the stop key and refused answers, and numbered figures and tables drawn from real sessions.
colors:
  paper: "#f4f5f2"
  ink: "#121413"
  ink-2: "#545a56"
  rule: "#c8ccc5"
  grid: "#e3e6e0"
  red: "#c9281a"
  red-lip: "#8a1a10"
  mark: "#fdf0a6"
  mark-strong: "#ffd60a"
  plate: "#121413"
  plate-ink: "#eceee9"
  plate-rule: "#3a3f3c"
  paper-dark: "#0f1110"
  ink-dark: "#e9ebe6"
  ink-2-dark: "#a3aaa5"
  rule-dark: "#343935"
  grid-dark: "#1c201e"
  red-dark: "#ff6150"
  red-lip-dark: "#a3271b"
  mark-dark: "#39340f"
  plate-dark: "#1b1e1c"
  plate-rule-dark: "#3d433f"
  on-red: "#ffffff"
  on-red-dark: "#0f1110"
  red-on-mark-dark: "#ff7a6b"
  docs-ground: "#e6e8e3"
  docs-ink: "#0c0e0d"
  docs-ink-2: "#464c48"
  docs-plate: "#0b0c0b"
  docs-stop: "#c42a1b"
  docs-ground-dark: "#161917"
  docs-ink-dark: "#e6e8e3"
  docs-ink-2-dark: "#a5aca7"
  docs-plate-dark: "#070807"
  docs-stop-dark: "#ff6a57"
typography:
  headline:
    fontFamily: "'Archivo Variable', ui-sans-serif, system-ui, sans-serif"
    fontSize: "clamp(2.375rem, 1.1rem + 4vw, 4.75rem)"
    fontWeight: 780
    lineHeight: 0.95
    letterSpacing: "-0.03em"
    fontVariation: "'wdth' 84"
  section:
    fontFamily: "'Archivo Variable', ui-sans-serif, system-ui, sans-serif"
    fontSize: "clamp(1.875rem, 1.2rem + 2vw, 2.875rem)"
    fontWeight: 760
    lineHeight: 1
    letterSpacing: "-0.025em"
    fontVariation: "'wdth' 84"
  wordmark:
    fontFamily: "'Archivo Variable', ui-sans-serif, system-ui, sans-serif"
    fontSize: "1.125rem"
    fontWeight: 760
    letterSpacing: "-0.01em"
    fontVariation: "'wdth' 88"
  lede:
    fontFamily: "'Archivo Variable', ui-sans-serif, system-ui, sans-serif"
    fontSize: "clamp(1.0625rem, 1rem + 0.25vw, 1.1875rem)"
    fontWeight: 400
    lineHeight: 1.5
  body:
    fontFamily: "'Archivo Variable', ui-sans-serif, system-ui, sans-serif"
    fontSize: "1rem"
    fontWeight: 400
    lineHeight: 1.5
  small:
    fontFamily: "'Archivo Variable', ui-sans-serif, system-ui, sans-serif"
    fontSize: "0.875rem"
    fontWeight: 400
    lineHeight: 1.45
  label:
    fontFamily: "'Archivo Variable', ui-sans-serif, system-ui, sans-serif"
    fontSize: "0.6875rem"
    fontWeight: 650
    letterSpacing: "0.07em"
  key-legend:
    fontFamily: "'Archivo Variable', ui-sans-serif, system-ui, sans-serif"
    fontSize: "0.8125rem"
    fontWeight: 700
    lineHeight: 1
    letterSpacing: "0.04em"
  docs-title:
    fontFamily: "'Archivo Variable', ui-sans-serif, system-ui, sans-serif"
    fontSize: "clamp(2.5rem, 1.6rem + 3vw, 3.75rem)"
    fontWeight: 840
    lineHeight: 0.95
    letterSpacing: "-0.025em"
    fontVariation: "'wdth' 72"
  docs-section:
    fontFamily: "'Archivo Variable', ui-sans-serif, system-ui, sans-serif"
    fontWeight: 760
    letterSpacing: "-0.015em"
    fontVariation: "'wdth' 84"
  answer:
    fontFamily: "'JetBrains Mono Variable', ui-monospace, 'DejaVu Sans Mono', monospace"
    fontSize: "0.875rem"
    lineHeight: 1.65
  readout:
    fontFamily: "'JetBrains Mono Variable', ui-monospace, 'DejaVu Sans Mono', monospace"
    fontSize: "0.8125rem"
    lineHeight: 1.55
  command:
    fontFamily: "'JetBrains Mono Variable', ui-monospace, 'DejaVu Sans Mono', monospace"
    fontSize: "0.8125rem"
    lineHeight: 1.6
  call-list:
    fontFamily: "'JetBrains Mono Variable', ui-monospace, 'DejaVu Sans Mono', monospace"
    fontSize: "0.75rem"
    lineHeight: 1.5
  trace-label:
    fontFamily: "'JetBrains Mono Variable', ui-monospace, 'DejaVu Sans Mono', monospace"
    fontSize: "12.5px"
  figure:
    fontFamily: "'JetBrains Mono Variable', ui-monospace, 'DejaVu Sans Mono', monospace"
    fontSize: "0.75rem"
    fontWeight: 500
    lineHeight: 1.4
rounded:
  corner: "3px"
  chart: "2px"
  key: "6px"
  docs-still: "4px"
  docs-code: "0.75rem"
spacing:
  margin: "clamp(1rem, 3vw, 2.5rem)"
  sheet: "86rem"
  section: "clamp(4rem, 10vh, 6.5rem)"
  head-gap: "2rem"
  touch: "2.75rem"
  lanes: "5.5rem"
  docs-content: "46rem"
components:
  button-primary:
    backgroundColor: "{colors.ink}"
    textColor: "{colors.paper}"
    rounded: "{rounded.corner}"
    padding: "0 1.4rem"
    height: "2.75rem"
  button-primary-hover:
    backgroundColor: "{colors.ink-2}"
    textColor: "{colors.paper}"
  button-small:
    backgroundColor: "{colors.ink}"
    textColor: "{colors.paper}"
    rounded: "{rounded.corner}"
    padding: "0 1rem"
    height: "2.5rem"
  version-tag:
    textColor: "{colors.ink-2}"
    typography: "{typography.figure}"
    padding: "0.05rem 0.35rem"
  version-tag-hover:
    backgroundColor: "{colors.mark}"
    textColor: "{colors.ink}"
  tab:
    textColor: "{colors.ink}"
    typography: "{typography.small}"
    padding: "0 0.95rem"
    height: "2.5rem"
  tab-hover:
    backgroundColor: "{colors.grid}"
  tab-checked:
    backgroundColor: "{colors.ink}"
    textColor: "{colors.paper}"
  stop-key:
    backgroundColor: "{colors.red}"
    textColor: "{colors.on-red}"
    typography: "{typography.key-legend}"
    rounded: "{rounded.key}"
    width: "3.4rem"
    height: "2.9rem"
  readout:
    backgroundColor: "{colors.mark}"
    textColor: "{colors.ink}"
    typography: "{typography.readout}"
    padding: "0.55rem 0.75rem"
    height: "3.4rem"
  check:
    backgroundColor: "{colors.paper}"
    textColor: "{colors.ink}"
    rounded: "{rounded.corner}"
    padding: "0.55rem 0.6rem"
  check-ok:
    backgroundColor: "{colors.ink}"
    textColor: "{colors.paper}"
  check-no:
    backgroundColor: "{colors.red}"
    textColor: "{colors.on-red}"
  code-block:
    backgroundColor: "{colors.plate}"
    textColor: "{colors.plate-ink}"
    typography: "{typography.command}"
    rounded: "{rounded.corner}"
    padding: "0.35rem 0.35rem 0.35rem 1rem"
  nav-link:
    textColor: "{colors.ink}"
    typography: "{typography.small}"
    padding: "0 0.6rem"
    height: "2.75rem"
---

# Design System: niri-computer-use

## Overview

**Creative North Star: "The Datasheet"**

The home page reads like a component datasheet: a running head across the top, ruled sections, one numbered figure (Figure 1, the recorded sessions as timing diagrams) and one numbered table (Table 1, when an action is refused), then the install steps. Paper and ink carry everything. Rules do the work that panels and cards would do elsewhere: a 2px ink rule opens each section, 1px ink rules head tables and lists, and 1px light rules separate rows.

Colour has two jobs and no decorative ones. The highlighter (mark, mark-strong) shows what is held or pointed at: the stretch where the agent holds the lease, the call under the cursor, the answer lines a field explains, the table row being run, the action travelling through the checks. Red is the stop key, the stop line of the headline, and every refused, cut or stopped answer.

The home page (`src/pages/index.astro`) sits outside Starlight and defines its own palette in a global style block, light and dark. The docs pages are Starlight restyled by `src/styles/theme.css` and keep the older concrete palette from `src/styles/tokens.css` (the docs-* tokens). The two palettes are separate. Both kinds of page share Archivo, JetBrains Mono, the theme stored under `starlight-theme`, and the crossfade between pages.

**Key Characteristics:**
- Paper, ink and rules; no panels, cards or section backgrounds.
- A yellow highlighter for state and attention, never for decoration.
- One red, for the stop key and refused answers.
- Archivo for text; JetBrains Mono for calls, commands, answers and the numbers that go with them (times, version).
- Numbered figure and table captions in the datasheet manner.
- Motion only where it explains, all of it off under reduced motion, and every part readable without JavaScript.

## Colors

Pale neutral paper with near-black ink, two greys for rules and grid lines, a pale and a strong yellow, and one red with a darker lip.

### Primary
- **Stop Red** (red, red-dark): the Esc keycap face, the headline's stop line, the "set by your stop key" note, a refused check in the block diagram, red traces and labels in the timing diagram (your stop, a cut call, a refused call, the lease taken back), the cursor and pen after a stop, and refused answers in readouts and call lists.
- **Key Lip** (red-lip, red-lip-dark): the keycap's lower edge, and nowhere else.
- **On Red** (on-red, on-red-dark): text on a red fill, the Esc legend and a refused check; white in light, the dark paper in dark, where white on the lighter red is too faint.
- **Red on the Highlighter** (red-on-mark-dark): refused and stopped lines in the readouts and the highlighted call; the light theme uses the stop red itself.

### Secondary
- **Highlighter** (mark, mark-dark): the held-lease band in the diagram, the readouts under the timing diagram and the block diagram, the answer lines and field rows in Result, the running table row, the current call in the call list, and the version tag on hover.
- **Strong Highlighter** (mark-strong): the stroke drawn under the headline's stop line when a stop lands, the packet that travels through the checks, and text selection. It has one value for both themes.

### Neutral
- **Paper** (paper, paper-dark): the page, the blocks of the block diagram, the pinned lane names, and the cover that hides the diagram until it is drawn. The meta `theme-color` uses the same two values.
- **Ink** (ink, ink-dark): text, the rules that open sections and head tables, traces, block outlines, buttons, the checked tab and a passed check.
- **Second Ink** (ink-2, ink-2-dark): captions, ledes beside section heads, lane and column labels, answers in call lists, times, and the Install button's hover.
- **Rule** (rule, rule-dark): row separators, the version tag's border and the dashed path through the checks.
- **Grid** (grid, grid-dark): the one-second lines in the timing diagram and the tab hover.
- **Plate** (plate, plate-dark), **Plate Ink** (plate-ink) and **Plate Rule** (plate-rule, plate-rule-dark): the install command blocks and their Copy button. In the light theme the plate is the same value as ink.

### Docs palette
The docs pages keep `tokens.css`: concrete ground, ink, second ink, plate and stop red (the docs-* tokens, light and dark). `theme.css` maps Starlight's grey ramp from the ink to the ground (`#0c0e0d` to `#e6e8e3` in light, `#e6e8e3` to `#161917` in dark), makes links and the current page ink, and sets Starlight's red to docs-stop. Code blocks are Expressive Code with `github-dark` on docs-plate, 0.75rem corners, no frame, lines wrapped.

### Named Rules
**The Highlighter Rule.** Yellow marks state or attention: held, current, running, pointed at. If nothing is held or pointed at, nothing is yellow.

**The One Red Rule.** Red is the stop key and the answers it causes (refused, cut, stopped, taken back). Never use red for links, emphasis or calls to action.

## Typography

**Display and Body Font:** Archivo Variable (with ui-sans-serif, system-ui, sans-serif)
**Mono Font:** JetBrains Mono Variable (with ui-monospace, 'DejaVu Sans Mono', monospace)

**Character:** a narrow, heavy grotesque for heads (84% and 88% width) over the same face at full width for reading; mono wherever the text is something a machine said or a reader could type.

The home page preloads a cut of Archivo made by `scripts/subset-font.mjs` (width axis 62% to 100%), with Fontsource's full face under the same name for anything the cut lacks.

### Hierarchy
- **Headline** (780, 84% width, clamp(2.375rem, 1.1rem + 4vw, 4.75rem), 0.95): the hero's two lines, "Your agent can use your niri desktop." and the red "One key takes it back.", each an inline block, balanced.
- **Section** (760, 84% width, clamp(1.875rem, 1.2rem + 2vw, 2.875rem), 1): the head of each ruled section, with its lede (1.0625rem, second ink, at most 56ch) in the next column.
- **Wordmark** (760, 88% width, 1.125rem): the name in the header and footer. On docs pages it is clamp(1.0625rem, 1rem + 0.3vw, 1.25rem).
- **Lede** (400, clamp(1.0625rem, 1rem + 0.25vw, 1.1875rem), 1.5, at most 58ch): the sentence under the headline.
- **Body** (400, 1rem, 1.5): running text, list items, table cells, the prompt beside a recording.
- **Small** (0.875rem, 1.45): captions, notes, the header, tabs, the footer, the stop hint.
- **Label** (650, 0.6875rem to 0.75rem, 0.07em, uppercase, second ink): lane names, table column heads, the two list heads under the limits, and "Asked" before the prompt.
- **Key legend** (700, 0.8125rem, 0.04em, uppercase): "Esc" on the keycap.
- **Mono roles:** answer JSON (0.875rem, 1.65); readouts and command blocks (0.8125rem); call lists and table error codes (0.75rem); trace labels in the diagram (12.5px); axis ticks and wire labels (0.6875rem); the version tag (500, 0.75rem); install step numbers (700, 1.25rem).
- **Docs title** (840, 72% width) and **Docs section** (760, 84% width): Starlight page titles and h2/h3.

### Named Rules
**The Caption Rule.** A figure or table that carries evidence is captioned with its number in bold ink ("Figure 1.", "Table 1.") followed by the caption in second ink.

**The Mono for Machines Rule.** JetBrains Mono is for calls, commands, answers and the numbers attached to them. Headings, labels and prose stay in Archivo.

## Layout

One centred sheet, at most 86rem wide (sheet), with a fluid side margin (margin). The header is a three-column grid (wordmark and version tag, the running head, the nav) over a 1px ink rule. Sections follow at a vertical gap (section), each opened by a 2px ink rule and a two-column head (head and lede, 1fr to 1.35fr). The footer sits on a 1px ink rule.

- **Hero:** the headline, then the lede on the left and the actions (Install, the stop key link) on the right, wrapping on narrow screens.
- **Figure 1:** the caption and the tabs on one row; then the recording (5fr) beside a side column (at least 15rem, 2fr) with the prompt and, under it, the keycap and its hint; under both, the timing diagram across the full width with four lanes (Calls, Lease, niri saw, You) whose names (lanes) stay pinned as it scrolls; then the readout, four lines high so the page below never moves, a call's lines cut short rather than wrapped and the stop's answer wrapped, the call list and the note. The diagram is at least 44rem wide plus the lanes and scrolls sideways on smaller screens, keeping the cursor in view.
- **Result:** the answer JSON (1.1fr) beside the field list (1fr).
- **Gates:** Agent, the server's checks in order, niri, joined by arrowed wires; then the answer readout and Table 1.
- **Limits:** two lists side by side. **Requirements:** a two-column table. **Install:** numbered steps, at most 60rem wide.

Breakpoints, all max-width: 60rem (the block diagram stands upright and Table 1 stacks), 56rem (the running head hides; section heads and limits go to one column), 52rem (the recording, prompt and keycap stack; Result goes to one column), 40rem (narrower lanes; tabs become separate boxes), 30rem (the header's Install button hides). Targets are at least 2.75rem (touch) tall, except the tabs and the header's Install button at 2.5rem.

## Elevation & Depth

Flat. Depth is rules and fills: an ink outline around a block, an ink fill when a check passes or a tab is checked, a highlighter fill for state. Two elements cast shadows, both physical objects: the keycap (`0 4px 0 var(--red-lip), 0 6px 12px -4px rgb(0 0 0 / 0.35)`, shrinking to `0 1px 0` and `0 2px 4px -2px` when pressed) and the Play button laid over the still (`0 4px 14px -6px rgb(0 0 0 / 0.5)`).

### Named Rules
**The Rule Not Panel Rule.** Separate content with rules (2px ink to open a section, 1px ink to head a table or list, 1px rule between rows), not with background panels or shadows.

## Shapes

Small corners: 3px (corner) on buttons, tabs, command blocks, diagram blocks and the skip link; 2px on the diagram's focus area; 6px on the keycap. The recording frame is square with a 1px ink border. The packet is a circle with a 2px ink ring. Wires end in solid arrowheads; the path through the checks is dashed. Traces in the diagram are square steps (miter joins, 1.75 stroke). The theme icon is an inline SVG circle, half filled, turned 180 degrees in the dark theme.

## Components

### Buttons
- **Install:** ink on paper, 2.75rem tall, 1.4rem side padding, 650 weight, 3px corners. Hover goes to second ink; press scales to 0.97 over 140ms. A smaller copy (2.5rem) sits in the header.
- **Text links:** underlined 1px at 0.2em offset, 2px on hover; standalone ones are 600 weight and 2.75rem tall.
- **Copy:** on the plate, a 1px plate-rule outline that fills on hover, press 0.96. It reads Copied, or "Select it" if the clipboard refuses, for 1.6 s, and is hidden without JavaScript.
- **Focus:** a 2px ink outline, 3px off (4px around the diagram; plate ink on the plate).

### Navigation
- **Header:** wordmark with the version tag (mono, 1px rule border, highlighter on hover, linking to the release), the running head "MCP server for niri 26.04 on Linux", then Docs, GitHub, the theme toggle (hidden without JavaScript) and Install. Below 24rem the version tag is left to the footer, and below 21rem the header's gaps tighten so it fits 320px. Links are 550 weight, underlined on hover.
- **Footer:** wordmark and version; Docs, GitHub, llms.txt, MIT license.

### Figure 1: Sessions
Tabs are CSS radio buttons inside a 1px ink box; the checked tab is ink, and `:has()` picks the panel shown, so switching works without JavaScript. Each panel holds the still (with a `<noscript>` video with controls), its caption, the prompt, the keycap and hint, the timing diagram drawn at build time, the readout and "All N calls, as text". Without JavaScript the diagram is complete and the keycap and Play button stay hidden.

The video element is created only when the recording starts, muted and inline. The first session starts when 60% of its frame is in view; under reduced motion the diagram is shown whole, with Save-Data on it draws itself in without the recording, and in both the recording waits for Play. Played from the start, the diagram draws in with it. Hovering or the arrow keys move the cursor over the calls, and a click plays from that time. The arrow keys step call by call, including calls that start at the same moment, and the call stepped to stays shown while the recording plays on to the next one. Leaving the view, or turning on reduced motion, pauses the recording and shows the diagram whole. The recording runs a little past the last call; the cursor waits at the diagram's end. The readout is announced to screen readers only for what the reader asked for: an arrow key or a stop.

### Stop Key
A red keycap with an uppercase Esc legend. Pressing it, or Esc while a session runs in view, stops the session at that moment: the action running then is cut and turns red (a look runs on to its answer), later actions are struck through and dashed red, later looks fade to 0.55 and everything else later to 0.18, the lease drops to "taken back" in red, a red "your stop key" pulse appears in the You lane, the cursor and pens turn red, the video pauses, and the readout states what the server answers from then on. Pressing again replays.

### Block diagram and Table 1
Each check is a paper block with an ink outline; passed checks fill ink, a refused check fills red. In Table 1 each row's check name is a button that runs that case; the running row is highlighted and the answer appears in the readout above the table, announced to screen readers only for a row the reader picked. Picking a row, even before the diagram has been seen, cancels the demo run. Without JavaScript the readout is hidden.

### Result
The answer JSON between two ink rules, beside its fields. Pointing at a field highlights its lines; the fields aren't focus stops, since they do nothing.

### Motion
One easing, `cubic-bezier(0.22, 1, 0.36, 1)`. Section rules draw across from the left on a view timeline (entry 10% to entry 70%). In the timing diagram a paper cover pulls back, labels fade up 5px as the cursor reaches them, and pens ride the call and lease traces, in step with the recording. While the agent works a ring pulses out from the keycap every 1.6 s; when a stop lands the diagram flashes red for 750ms, the key sinks 3px over 90ms, and the highlighter stroke draws under the headline's stop line over 700ms. In the block diagram the packet hops 260ms per block and a refused check shakes for 420ms; in view, it runs one pass and then the stop flag. In Result the fields light up in turn every 1.3 s once 60% is in view, until the reader points. Under reduced motion there is no rule draw, theme icon turn, headline stroke, cover, packet, shake, flash, ring, field sequence, smooth scroll or page crossfade; the diagram and answers appear complete.

## Do's and Don'ts

### Do:
- **Do** caption evidence as numbered figures and tables, with the number in bold ink.
- **Do** use the highlighter only for what is held, current, running or pointed at.
- **Do** keep red for the stop key and the answers it causes, with on-red text on it.
- **Do** separate sections and rows with rules: 2px ink to open, 1px ink to head, 1px rule between.
- **Do** draw diagrams at build time so they read complete without JavaScript, and give every interactive part a no-JS state.
- **Do** tie each motion to what it explains, and give it a complete reduced-motion state.
- **Do** keep the docs pages on the `tokens.css` palette and the home page on its own; change either on purpose, not by drift.

### Don't:
- **Don't** add card panels, background bands or decorative shadows; the keycap and the Play button over the still are the only shadows.
- **Don't** use yellow or red as decoration, emphasis or link colour.
- **Don't** set headings, prose or labels in JetBrains Mono.
- **Don't** put a small uppercase label above a heading; uppercase labels name lanes, columns and lists.
- **Don't** show a recording without its calls as text.
