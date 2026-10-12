---
name: niri-computer-use
description: Graphite columns on niri's strip, one amber focus ring, and every claim shown as a request with the answer that came back.
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
    fontSize: "clamp(2.1rem, 1.5rem + 1.4vw, 2.7rem)"
    fontWeight: 680
    lineHeight: 1.04
    letterSpacing: "-0.025em"
  headline:
    fontFamily: "'Archivo Variable', ui-sans-serif, system-ui, sans-serif"
    fontSize: "clamp(1.6rem, 1.2rem + 1.2vw, 2.25rem)"
    fontWeight: 660
    lineHeight: 1.12
    letterSpacing: "-0.02em"
  lead:
    fontFamily: "'Archivo Variable', ui-sans-serif, system-ui, sans-serif"
    fontSize: "clamp(var(--sl-text-base), 0.9rem + 0.4vw, var(--sl-text-lg))"
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
  button: "8px"
  panel: "10px"
  plate: "12px"
  ring: "14px"
spacing:
  gutter: "1rem"
  column: "1.25rem"
  section: "3rem"
  hero-gap: "3rem"
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
  column:
    backgroundColor: "{colors.graphite-panel}"
    rounded: "{rounded.panel}"
    padding: "{spacing.column}"
  plate:
    backgroundColor: "{colors.graphite-black}"
    rounded: "{rounded.plate}"
    padding: "2px"
---

# Design System: niri-computer-use

## Overview

**Creative North Star: "The Column in Focus"**

The site is drawn as niri's own desktop: a strip of graphite columns split by 1px rules, with one amber focus ring that sits on whatever has focus. On the home page each section is a column, and the ring lights the section in the middle of the view as the page scrolls, the way niri's ring follows the column you scrolled to. Starlight supplies the header, search, sidebar and theme switch; the system lives in the theme tokens and four components layered on top.

Evidence is the decoration. Recordings sit on plates with registration ticks at the corners of the nested output's 1280×720, a mono geometry line names that output, and the calls under each recording are the real requests and answers from the same run, written as `→ request` and `← answer` with their milliseconds. There are no illustrations, gradient fills, glows or shadows. The page is dense and quiet, and color is spent only on focus, action and refusal.

Both themes are first class. Dark is graphite with a bright amber ring; light is pale paper with the amber deepened to a brown that holds contrast on white.

**Key Characteristics:**
- Graphite panels and 1px rules, no shadows.
- One accent, amber, taken from the focus ring.
- Red only for an answer that refused or stopped.
- Archivo for prose and headings, JetBrains Mono for calls, code, geometry and metadata.
- Media registered like printed plates, with corner ticks and their real output geometry.

## Colors

Cool graphite neutrals with a single warm amber, and a red held back for refusals.

### Primary
- **Focus Amber** (amber-ring): the focus ring. It marks the section in view, the primary button, the hero's second headline line, the call currently playing in a transcript, the `→ ←` marks, the act path and gate in the diagram, the hero recording's plate, and keyboard focus everywhere (2px outline, 2px offset). Starlight's accent uses the same value, so links and active sidebar items share it.
- **Lit Amber** (amber-high): hover on the dark primary button.
- **Amber Wash** (amber-low): Starlight's low accent background in dark.
- **Deepened Ring** (amber-ring-light): the focus ring in the light theme, deepened so it reads on paper. Primary buttons carry white text on it.
- **Deep Amber** (amber-deep-light) and **Amber Ink** (amber-ink-light): Starlight's light accent and its high step; the deep amber is also the light primary button's hover. **Pale Amber** (amber-low-light) is the light low accent.

### Secondary
- **Stop Red** (stop-red dark, stop-red-light light): only the answer line of a call that ended in an error, `stopped` or a refusal, mark included.

### Neutral
- **Graphite** (graphite-black): the dark page and the recording plate's backing. The plate keeps this graphite in the light theme too, because the recordings are of a dark desktop.
- **Panel Graphite** (graphite-panel): column and diagram box fill in dark.
- **Rule Graphite** (graphite-rule): 1px rules between sections, column borders, quiet button border, diagram box strokes.
- **Unlit Column** (graphite-ring-off dark, paper-ring-off light): the unlit columns in the favicon, the state of a column without focus.
- **Gray steps** (graphite-gray-6 to graphite-gray-1): Starlight's gray scale. Gray 4 draws diagram wires, arrowheads and plate ticks; gray 3 is muted metadata (timestamps, geometry, `asked`, milliseconds, the requirements line); gray 2 is secondary text such as the tagline, captions and answer lines.
- **Graphite White** (graphite-white): primary text and headings in dark.
- **Paper** (paper), **Paper Panel** (paper-panel), **Paper Rule** (paper-rule), **Paper Grays** (paper-gray-3, paper-gray-2) and **Ink** (paper-ink): the same roles in the light theme.

### Named Rules
**The One Ring Rule.** Amber means focus: what is in view, what is held, what acts, what to press. It never fills a panel, tints a background or decorates a heading for emphasis alone.

**The Refusal Red Rule.** Red appears only on an answer that refused or stopped. Nothing else on the site is red.

## Typography

**Display Font:** Archivo Variable (with ui-sans-serif, system-ui)
**Body Font:** Archivo Variable
**Label/Mono Font:** JetBrains Mono Variable (with ui-monospace, DejaVu Sans Mono)

**Character:** A firm, slightly condensed grotesque set tight at large sizes, beside a plain mono that carries everything the machine said.

### Hierarchy
- **Display** (680, fluid 2.1rem to 2.7rem, 1.04): the home page promise only. Balanced wrapping; its second line drops to its own line in amber.
- **Headline** (660, fluid 1.6rem to 2.25rem, 1.12, max 40rem): one per home section, a plain sentence.
- **Title** (Starlight's h3 size, -0.01em): column headings inside a section and docs subheads. Code inside h2 and h3 loses its chip and drops to 0.85em.
- **Lead** (fluid base to large, 1.55, max 36rem): the hero tagline, in gray 2.
- **Body** (Starlight defaults): prose at a 46rem content width on docs pages.
- **Call** (mono 0.75rem, 1.6): request and answer lines in transcripts, unwrapped and scrolled sideways when long.
- **Label** (mono 0.7rem, tabular numbers): timestamps and the geometry line above each plate. Mono at Starlight's xs size carries the `asked` tag, the transcript toggle and the requirements line.

### Named Rules
**The Machine Speaks Mono Rule.** Anything the server, niri or a shell said or will be told (requests, answers, timings, geometry, commands) is mono. Prose and headings are never mono.

## Layout

On docs pages Starlight's layout holds with a 46rem content width; the splash home widens it to 76rem. The home page is a stack of sections, each opened by a 1px rule and 3rem of vertical padding. Inside a section, columns sit side by side from 50rem up, auto-fit at a 15rem minimum with 1rem gutters, and stack below that.

The hero is one column on narrow screens (copy, then the recording) and from 62rem becomes a 5:7 grid with the copy left, nudged down 2.5rem, and the recording right, 3rem apart, with 3.5rem above and 4rem below. Transcripts lay each call out as a two-column grid: the timestamp spanning two rows on the left, request and answer stacked on the right.

The architecture diagram is an SVG on a 960×360 view box. Below 40rem its text would be too small, so it is replaced by the same path as a mono list.

**The Column Strip Rule.** New sections are columns on the strip: a rule above, a headline, then content or bordered columns. They are not floating cards, centered heroes or full-bleed bands.

## Elevation & Depth

Flat. No element uses a shadow. Depth comes from tonal layering (panels one step off the page), 1px rules, and the amber ring drawn as a 2px border outside the thing it marks. A recording's plate is the one recessed surface: graphite backing, a 2px border, and 14px registration ticks 9px outside its corners.

**The Ring, Not Lift Rule.** Focus and state are shown by a ring or a border color, never by raising something with a shadow.

## Shapes

Gently rounded rectangles throughout. Panels and video use a 10px radius; a border drawn around a panel adds its offset to the radius (12px for the plate, 14px for the section ring) so the curves stay concentric. Buttons use 8px. Diagram boxes step down with size (10px, 8px, 6px for the gate). The gate is the only dashed outline. Lines are 1px for structure and 2px for focus and the plate.

## Components

### Buttons
Two buttons, both 2.75rem tall, semibold, 8px corners, with a 150ms color transition.
- **Primary:** amber fill with graphite text in dark; deepened amber with white text in light. Hover lightens it in dark and deepens it in light.
- **Quiet:** transparent with a 1px rule border and primary text. Hover darkens the border to gray 3.
- **Focus:** the global amber outline.

### Section (Workspace)
- **Structure:** a 1px top rule and 3rem vertical padding.
- **Focus ring:** a 2px amber border, 14px corners, set 1rem in from the top and bottom and 0.75rem outside the sides. A scroll-linked animation fades it in and scales it from 0.985 to 1 while the section crosses the middle of the view, then out again. It is off under reduced motion and where scroll timelines are unsupported.

### Column
- **Corner Style:** 10px.
- **Background:** panel graphite or paper panel.
- **Border:** 1px rule.
- **Internal Padding:** 1.25rem.
- A recording inside a column takes a rule-colored plate border, leaving the amber to the section ring.

### Plate (Demo recording)
- A mono geometry line, right aligned, names the output (`winit · 1280×720 at 0,0`).
- The video sits on a graphite plate with a 2px border (amber in the hero, rule color in columns) and corner ticks around the 1280×720 area, excluding the caption band the encoder burns in below it.
- Below: an optional caption, the mono `asked` tag then the prompt, and a disclosure, "The calls as text", open in the hero. Each call shows its time, `→ request` and `← answer` with milliseconds; the call playing gets a 2px amber left border.
- Plays muted and looped only while 35% in view, never under reduced motion; posters below the fold load as they approach.
- The lease tag (dashed and gray when free, amber when held) lives inside the recordings, drawn by the encoder, not as a page component.

### Diagram
Boxes in panel fill with 1.5px rule strokes, gray wires with filled arrowheads, mono text throughout. The act path, its arrowhead, its label and the dashed gate are amber; the look path is gray.

## Do's and Don'ts

### Do:
- **Do** draw new home sections with the Workspace component, so the rule and the focus ring come with them.
- **Do** show real requests and answers from a run, with their timings, in `→` and `←` notation.
- **Do** register recordings on a plate with ticks and their real output geometry.
- **Do** keep the recording plate graphite in both themes.
- **Do** give every video an `aria-label` and its calls as text, and keep autoplay off under reduced motion.
- **Do** check every new color pair in both themes: body text at 4.5:1, ring and focus at 3:1.

### Don't:
- **Don't** add shadows, glows or gradient fills; depth is tone and rules.
- **Don't** use amber outside the One Ring Rule, or red for anything but a refusal or stop.
- **Don't** put a mono label or wordmark above a heading as an eyebrow.
- **Don't** show mocked output or a drawn terminal; recordings and transcripts come from real runs.
- **Don't** add a logo beside the name; the name is set in type, and the favicon's three columns are the only mark.
