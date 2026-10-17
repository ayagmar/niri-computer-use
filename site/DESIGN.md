---
name: niri-computer-use
description: A hand-made Linux project page from around 2000 for the home page, with a separate Archivo and JetBrains Mono system for the docs.
colors:
  desk: "#c0c0c0"
  paper: "#ffffff"
  face: "#e4e4dc"
  face-hover: "#efefe8"
  ink: "#000000"
  ink-2: "#3c3c38"
  band: "#336666"
  band-ink: "#ffffff"
  band-stop: "#ffdd77"
  bar: "#dfe9e9"
  bar-ink: "#133333"
  bar-rule: "#9fbcbc"
  bevel-hi: "#ffffff"
  bevel-lo: "#808080"
  bevel-dk: "#404040"
  well: "#f4f4ee"
  link: "#0000cc"
  link-visited: "#663399"
  link-active: "#cc0000"
  note: "#ffffcc"
  note-edge: "#996600"
  ledger-gap: "#fff8ee"
  ring: "#ffcc33"
  ring-edge: "#0f2626"
typography:
  title:
    fontFamily: "Tinos, 'Times New Roman', Times, 'Liberation Serif', serif"
    fontSize: "30px"
    fontWeight: 700
    lineHeight: 1.1
  heading:
    fontFamily: "Tinos, 'Times New Roman', Times, 'Liberation Serif', serif"
    fontSize: "19px"
    fontWeight: 700
    lineHeight: 1.45
  subheading:
    fontFamily: "Tinos, 'Times New Roman', Times, 'Liberation Serif', serif"
    fontSize: "16px"
    fontWeight: 700
    lineHeight: 1.45
  body:
    fontFamily: "Tinos, 'Times New Roman', Times, 'Liberation Serif', serif"
    fontSize: "17px"
    fontWeight: 400
    lineHeight: 1.45
  table:
    fontFamily: "Tinos, 'Times New Roman', Times, 'Liberation Serif', serif"
    fontSize: "16px"
    fontWeight: 400
    lineHeight: 1.45
  caption:
    fontFamily: "Tinos, 'Times New Roman', Times, 'Liberation Serif', serif"
    fontSize: "15px"
    fontWeight: 400
    lineHeight: 1.45
  status:
    fontFamily: "Tinos, 'Times New Roman', Times, 'Liberation Serif', serif"
    fontSize: "14px"
    fontWeight: 400
    lineHeight: 1.45
  code:
    fontFamily: "'DejaVu Sans Mono', 'Liberation Mono', 'Courier New', monospace"
    fontSize: "13.5px"
    fontWeight: 400
    lineHeight: 1.45
  button-88:
    fontFamily: "Verdana, 'DejaVu Sans', sans-serif"
    fontSize: "11px"
    fontWeight: 700
    lineHeight: 1.05
spacing:
  gap-tight: "2px"
  gap-button: "6px"
  pad-cell: "3px 8px"
  pad-main: "16px 22px 18px"
  side-width: "160px"
  page-width: "760px"
components:
  page:
    backgroundColor: "{colors.paper}"
    width: "760px"
  title-band:
    backgroundColor: "{colors.band}"
    textColor: "{colors.band-ink}"
    typography: "{typography.title}"
    padding: "14px 18px"
  side-button:
    backgroundColor: "{colors.face}"
    textColor: "{colors.ink}"
    padding: "3px 8px"
  side-button-hover:
    backgroundColor: "{colors.face-hover}"
  heading-bar:
    backgroundColor: "{colors.bar}"
    textColor: "{colors.bar-ink}"
    typography: "{typography.heading}"
    padding: "2px 8px"
  code-well:
    backgroundColor: "{colors.well}"
    textColor: "{colors.ink}"
    typography: "{typography.code}"
    padding: "6px 10px"
  notice:
    backgroundColor: "{colors.note}"
    textColor: "{colors.ink}"
    padding: "4px 10px"
  table-head-cell:
    backgroundColor: "{colors.face}"
    textColor: "{colors.ink}"
    padding: "3px 8px"
  status-bar:
    backgroundColor: "{colors.face}"
    textColor: "{colors.ink}"
    typography: "{typography.status}"
    padding: "3px"
  button-88:
    textColor: "#ffffff"
    typography: "{typography.button-88}"
    width: "88px"
    height: "31px"
---

# Design System: niri-computer-use

This file records two systems. The home page (`site/src/pages/index.astro`, `src/components/home/`, words in `src/lib/home.ts`) has its own styles and is described first. The Starlight docs pages have a separate system, described at the end; the home page shares none of it.

## Overview

**Creative North Star: "The Project Page From Around 2000"**

A hand-made Linux project page, done with care and not as parody. A 760-pixel white page sits on the desktop grey, with a teal title band on top, a column of bevelled buttons down the side, facts in tables, one framed still that opens a clip, and a sunken status bar at the foot. Everything is set in one serif, Tinos, with system monospace for commands. The page is drawn the way a person would have drawn it with tables and border colors: no radius, no gradients, no blur, no cards.

The density is that of a document. Facts go in tables and short lists, not sentences. The era shows in the bevels, the link colors, the 88x31 buttons and the status bar, and stops there. The page is light only and has no JavaScript, and it says so in its status bar.

The direction contract holds the thesis: refuse the marketing hero, gradients, cards and autoplay video. The build matches it. Where the contract names the first-viewport order (band, side buttons, niri-only notice, still, facts table), the grid and source order produce it.

**Key Characteristics:**
- One 760px page on desk grey, bevelled with a 2px border (white top-left, dark bottom-right).
- Teal title band with a 24x16 pixel mark and a yellow stop line.
- One serif (Tinos 400 and 700), 17px body, monospace only for code.
- Raised things are bevelled buttons; sunken things are tables, code wells, the ledger and status cells.
- Links are blue, purple when visited, red when pressed.
- The focus ring is niri's: amber around a dark edge.
- Light only, no JavaScript, a print stylesheet.

## Colors

A grey, white and teal palette with one yellow and one amber. Color names follow the era: desk, paper, face, ink.

### Primary
- **Title Band Teal** (`band`): the title band background and text selection. Also the theme-color meta. White text on it.
- **Stop Yellow** (`band-stop`): the stop line in the band only, set in bold.

### Secondary
- **Heading Bar Teal** (`bar`): the light teal bar behind each h2, with `bar-ink` text and `bar-rule` top and bottom hairlines.

### Tertiary
- **Notice Yellow** (`note`): the one niri-only notice, with a `note-edge` brown border.
- **Focus Ring Amber** (`ring`) around a **Ring Edge** dark teal (`ring-edge`): keyboard focus, and also the pixel mark's focused column.

### Neutral
- **Desk Grey** (`desk`): the browser background around the page, and the scrollbar track.
- **Paper** (`paper`): the page and the skip link.
- **Face Grey** (`face`): side column, buttons, table header cells, frame around the still, status bar. `face-hover` is the button hover.
- **Ink** (`ink`) and **Second Ink** (`ink-2`): text, and captions and "Read it" labels.
- **Well** (`well`): code wells, a shade off paper.
- **Bevel highlight, low, dark** (`bevel-hi`, `bevel-lo`, `bevel-dk`): the only edge colors. Raised edges use hi on top-left and dk on bottom-right; sunken edges use lo top-left and hi bottom-right.
- **Link** (`link`), **Visited** (`link-visited`), **Active** (`link-active`): the default link states. The active red is also the text caret color.
- **Ledger Gap** (`ledger-gap`): a warm tint behind the "Not covered" half of the safety ledger.

### Named Rules
**The Edge Rule.** Depth comes only from the three bevel colors. Raised is hi top-left, dk bottom-right. Sunken reverses it with lo and hi. No other shadow or edge color is added.

**The Browser Defaults Rule.** Link colors are the era's blue, purple and red. Do not restyle them to the teal. Buttons that are links override to ink and keep it when visited.

## Typography

**Display, Body Font:** Tinos 400 and 700, self-hosted through `@fontsource/tinos` (latin 400 preloaded), with Times New Roman, Times and Liberation Serif behind it.
**Code Font:** system monospace (DejaVu Sans Mono, Liberation Mono, Courier New). Not self-hosted.
**88x31 Font:** Verdana with DejaVu Sans behind it, in the six era buttons only.

**Character:** a plain serif at a document size, with bold doing the work that size does elsewhere. No letter-spacing, no uppercase labels, no display face.

### Hierarchy
- **Title** (700, 30px, 1.1; 24px on phones): the h1 in the band.
- **Heading** (700, 19px): h2 on a light teal bar. In the side aside the h2 is 15px with a hairline underneath.
- **Subheading** (700, 16px): h3 over each half of the safety ledger.
- **Body** (400, 17px, 1.45; 16px on phones): main text. The main column is about 540px wide at full size, which gives a measure near 70 characters.
- **Table and list** (400, 16px): facts, releases and ledger lists. Release version and date cells use tabular numerals and do not wrap.
- **Caption** (400, 15px, `ink-2`): figcaption and "Read it:" labels. The 88x31 notes and side buttons are 15px bold.
- **Status** (400, 14px): status bar cells.
- **Code** (400, 13.5px, 1.45): install commands, wrapping with `pre-wrap` so nothing scrolls sideways.

### Named Rules
**The One Serif Rule.** Everything but code and the 88x31 buttons is Tinos. Do not add a second text face.

## Layout

The page is a CSS grid in a 760px box: a 160px side column and a flexible main column, with named areas band, side, main, more and status. The `more` area ("Built on", six 88x31 buttons) sits under the side buttons in the left column. Source order is band, side nav, main, more, status, so the content comes before the extras.

Spacing is small and fixed, in pixels: 2px between table cells and ledger halves, 6px between buttons, `3px 8px` cell and button padding, `16px 22px 18px` main padding, 26px above h2 and 10px below. The body has `24px 12px 40px` of desk around the page.

**Phones (640px and below):** one column in the order band, side, main, more, status. The page loses its border and the body padding drops to zero. The side buttons become a row of three equal columns, and two columns at 380px and below. The ledger stacks to one column. Body size drops to 16px and the title to 24px.

**Print:** the page becomes a block with no border, the side column, "Built on" aside, status bar and skip link are hidden, the band loses its color and gets a 2px black rule, and every outside link in main prints its address after it.

**Light only.** `color-scheme` is `light` in CSS and in a meta tag. The page imitates a period when there was no dark mode, and the bevel colors depend on a light face. Do not add a dark variant.

**No JavaScript.** Navigation is anchors, the clip opens from a link, and the build reads the changelog and the clip's size and length.

## Elevation & Depth

Flat plus bevel. The system has no blur and no spread. Depth is made of one- and two-pixel borders in the bevel colors.

### Shadow Vocabulary
- **Raised border** (`border: 2px solid; border-color: hi dk dk hi`, with `inset -1px -1px 0 lo` on buttons): the page and side buttons.
- **Pressed** (`border-color: dk hi hi dk; box-shadow: inset 1px 1px 0 lo`): `.bevel:active`. The 88x31 buttons swap their inset highlight and shade on `:active`.
- **Sunken** (`border-color: lo hi hi lo`): tables, cells, the ledger and its halves, the frame around the still, status cells. Code wells add `inset 1px 1px 0 dk`.
- **Page edge** (`1px 1px 0 ink` on the page): a one-pixel dark outer edge that finishes the bevel. It is not a drop shadow and is not used on anything else.
- **Focus ring** (`0 0 0 2px ring-edge, 0 0 0 5px ring, 0 0 0 6px ring-edge`): on `a:focus-visible` and the 88x31 buttons. A transparent 2px outline is kept for forced-colors modes. The framed still adds a 3px outline offset.

### Named Rules
**The Press Rule.** Raised controls press in on `:active`: side buttons and 88x31 buttons. Nothing animates; there are no transitions.

## Shapes

Square. There is no border radius anywhere on the home page. Silhouettes are rectangles on whole pixels. The pixel mark is drawn with `crispEdges` rectangles on a 24x16 grid, each grid pixel three CSS pixels (72x48), and shows niri's strip of columns running off both edges, the focused column ringed in amber, and a pointer inside it. The still is shown in a face-grey frame with a 1px ink border on the image.

## Components

### Title Band
Teal full-width bar: the mark, the h1 (white, 700, 30px) and one tagline line in 16px with the stop line in bold `band-stop` yellow. Padding `14px 18px`, 16px gap.

### Side Buttons
Face-grey bevelled link buttons, bold 15px ink, no underline, a 6px gap, one per anchor or outside page (Install, Safety, Releases, Documentation, Source, llms.txt). Hover lightens to `face-hover`. Active presses in. Visited stays ink. On phones they are centered in a 3-then-2-column grid.

### Heading Bars
h2 on a light teal bar with hairlines above and below. No rule or number elsewhere.

### Notice
One yellow-cream line in bold, directly under the top of main: the niri-only fact. It is the only use of `note`.

### Framed Still
A figure holding one still in a face-grey sunken frame, linking to one clip. The caption states what it shows and the clip's length, format and size, read from the file at build time. The image loads eagerly at high priority and is served as AVIF with a JPG fallback. No autoplay, no play-button overlay.

### Tables
Facts (a row header and a value, 108px header column) and releases (Release, Date, Changes). Separate borders with 2px spacing, sunken, with face-grey header cells. Release rows come from CHANGELOG.md, three at most.

### Install Steps and Code Wells
An ordered list with bold lead-ins. Each command is in a sunken, off-white `pre` that wraps. Alternatives are introduced with a 15px label ("Read it:", "Then run it:").

### Safety Ledger
Two sunken halves in a sunken frame: Enforced, and Not covered (tinted `ledger-gap`). Each is a short bulleted list. The ledger exists so a visitor can see what is not protected.

### 88x31 Buttons
Six 88x31 links in a wrap row under "Built on" (niri, Noctalia, Wayland, MCP, crates.io, MIT). Each is a solid era color (brown, indigo, blue, charcoal, green, maroon) with a 1px black border, a two-tone inset bevel, white 11px Verdana bold top line and a 9px cream bottom line. They link to real projects the tool stands on. Hover underlines the top line; active presses in.

### Status Bar
Three sunken cells at the foot: "Best viewed with any browser", "No JavaScript", and the current version (with its date when it is the latest release). The last cell does not grow.

### Skip Link
Hidden above the page until focused, then a white tab at the top left.

## Do's and Don'ts

### Do:
- **Do** put facts in tables and short lists, not paragraphs.
- **Do** build raised things from hi/dk bevels and sunken things from lo/hi bevels, and press raised controls in on `:active`.
- **Do** use one still that links to one clip, and state the clip's length, format and size next to the link.
- **Do** keep the page light only, one column on phones, and working without JavaScript.
- **Do** keep a visible focus ring on every link: amber (`ring`) outside a dark teal edge.
- **Do** keep the print view: address after each outside link, side column and status bar dropped.
- **Do** show only things that are true: version, date, clip size and release rows come from the build, and the "Built on" buttons point at real projects.
- **Do** add era touches with taste: a status bar, bevels, 88x31 buttons for real dependencies. One of each, not a collection.

### Don't:
- **Don't** invent era furniture: no visitor counters, guestbooks, awards, "under construction" signs or webrings.
- **Don't** use gradients, glow, blur, border radius, cards, or soft shadows.
- **Don't** autoplay video or put a play overlay on the still.
- **Don't** add a marketing hero, slogan block or testimonial row.
- **Don't** add a dark mode, JavaScript, animation or transitions to the home page.
- **Don't** use blinking, marquees, tiled backgrounds or the Comic Sans-style parody of the period. The page is a careful version of it, not a joke.
- **Don't** put uppercase labels, kickers or eyebrows over headings.
- **Don't** reuse the docs tokens or fonts on the home page, or the home page's bevels on the docs.

## The docs pages: a separate system

The Starlight docs pages keep `src/styles/tokens.css` and `src/styles/theme.css`. This system is real and unchanged, and is recorded here only so nobody mixes the two.

- **Palette:** a cool concrete ground (`--ncu-ground` #e6e8e3), near-black ink (`--ncu-ink` #0c0e0d), second ink #464c48, a black code plate (#0b0c0b) and one red for a stopped answer (`--ncu-stop` #c42a1b). Light and dark both exist; dark swaps to ground #161917, ink #e6e8e3, second ink #a5aca7, plate #070807 and stop #ff6a57. Starlight's gray scale and accent are remapped to these in `theme.css`.
- **Type:** Archivo Variable for text (narrow, heavy titles: h1 weight 840 at 72% width; site title weight 760 at 88% width) and JetBrains Mono Variable for calls and commands.
- **Behaviour:** a 2px ink focus outline offset 3px, ink selection, red caret, pages crossfading by view transition when motion is allowed, and a 46rem content width.
- **Boundary:** the home page imports none of these files, and none of the home page's tokens belong in the docs.

Not canonized, by design: the page's one-off tints (`#efefe8` hover, `#f4f4ee` well, `#fff8ee` ledger, `#9fbcbc` bar rule, `#996600` notice edge) are tied to the components that use them and are not a general palette.
