---
name: niri-computer-use
description: The manual page niri-computer-use(1) as man shows it in a terminal, set in JetBrains Mono, for the home page and the docs.
colors:
  ground: "#f7f5ee"
  ink: "#2b2a25"
  bold: "#0c0c0a"
  dim: "#5c584e"
  rule: "#d4cfbf"
  accent: "#8a5300"
  select: "#f0dca8"
  plate: "#eeeadd"
  stop: "#b3261e"
  ground-dark: "#10110f"
  ink-dark: "#c9c6b9"
  bold-dark: "#f4f1e6"
  dim-dark: "#9a9588"
  rule-dark: "#2f2d27"
  accent-dark: "#e3a83c"
  select-dark: "#3a3220"
  plate-dark: "#0a0b09"
  stop-dark: "#ff6a57"
typography:
  body:
    fontFamily: "'JetBrains Mono Variable', ui-monospace, 'DejaVu Sans Mono', monospace"
    fontSize: "16px"
    fontWeight: 400
    lineHeight: 1.55
  running:
    fontFamily: "'JetBrains Mono Variable', ui-monospace, 'DejaVu Sans Mono', monospace"
    fontSize: "16px"
    fontWeight: 400
    lineHeight: 1.55
  section:
    fontFamily: "'JetBrains Mono Variable', ui-monospace, 'DejaVu Sans Mono', monospace"
    fontSize: "16px"
    fontWeight: 700
    lineHeight: 1.55
  doc-title:
    fontFamily: "'JetBrains Mono Variable', ui-monospace, 'DejaVu Sans Mono', monospace"
    fontSize: "1.75rem"
    fontWeight: 700
    lineHeight: 1.2
  doc-body:
    fontFamily: "'JetBrains Mono Variable', ui-monospace, 'DejaVu Sans Mono', monospace"
    fontSize: "1rem"
    fontWeight: 400
    lineHeight: 1.65
  code:
    fontFamily: "'JetBrains Mono Variable', ui-monospace, 'DejaVu Sans Mono', monospace"
    fontSize: "0.9375rem"
    fontWeight: 400
    lineHeight: 1.65
spacing:
  indent: "7ch"
  indent-phone: "3ch"
  measure: "84ch"
  gap-running: "2ch"
  pad-pre: "0.15em 1ch"
  pad-less: "0.2rem 2ch"
  docs-content-width: "48rem"
components:
  page:
    backgroundColor: "{colors.ground}"
    textColor: "{colors.ink}"
    typography: "{typography.body}"
  running-line:
    textColor: "{colors.dim}"
    typography: "{typography.running}"
  section-name:
    textColor: "{colors.bold}"
    typography: "{typography.section}"
  code-plate:
    backgroundColor: "{colors.plate}"
    textColor: "{colors.bold}"
    typography: "{typography.code}"
    padding: "0.15em 1ch"
  less-line:
    backgroundColor: "{colors.ink}"
    textColor: "{colors.ground}"
    typography: "{typography.body}"
    padding: "0.2rem 2ch"
  link:
    textColor: "{colors.accent}"
  selection:
    backgroundColor: "{colors.select}"
    textColor: "{colors.bold}"
---

# Design System: niri-computer-use

One system covers the home page (`site/src/pages/index.astro`, words in `src/lib/home.ts`) and the Starlight docs. Both take their colours from `src/styles/tokens.css`. The docs map them onto Starlight in `src/styles/theme.css`, and the docs running head and foot are `src/components/PageTitle.astro` and `Footer.astro`. Running-head names come from `src/lib/man.ts`.

## Overview

**Creative North Star: "The Manual Page in a Terminal"**

The home page is the manual page `niri-computer-use(1)` as man shows it in a terminal. It has a running head and foot, capital section names (NAME, SYNOPSIS, DESCRIPTION, INSTALLATION, SAFETY, EXAMPLE, HISTORY, BUGS, SEE ALSO), bold for what you type, underline for paths, and less's reverse-video status line pinned to the bottom of the window. The docs are the rest of the manual: each page opens on a running head and ends on a running foot, and its section headings are capitals.

Everything is set in JetBrains Mono Variable. Dark is a warm black with parchment ink, light is paper with brown-black ink, and one amber marks links and focus. The home page follows the system colour scheme (`prefers-color-scheme`, `color-scheme: light dark`). The docs follow Starlight's theme switch, which sets `data-theme` on the root. The home page has no JavaScript.

**Key Characteristics:**
- One face, JetBrains Mono Variable 400 and 700; ligatures are off on the home page.
- One `light-dark()` palette for both themes; amber is the only accent.
- Text is indented 7ch under its section name, in a column at most 84ch wide (80ch plus 2ch padding each side).
- Running head and foot on the home page and on every docs page.
- The less line: reverse video, sticky, with a percentage that follows scrolling.
- Print is black on white, with each outside link's address after it.

## Colors

A warm, low-saturation palette. Each token has a light and a dark value, picked by `light-dark()`.

| Token | Light | Dark | Use |
|---|---|---|---|
| `ground` | #f7f5ee | #10110f | page ground; text on the less line |
| `ink` | #2b2a25 | #c9c6b9 | body text; less line background |
| `bold` | #0c0c0a | #f4f1e6 | groff bold, section names, code, table heads, selection text |
| `dim` | #5c584e | #9a9588 | running head and foot, captions |
| `rule` | #d4cfbf | #2f2d27 | hairlines, still frame, scrollbar track |
| `accent` | #8a5300 | #e3a83c | links, focus ring, caret, `accent-color` |
| `select` | #f0dca8 | #3a3220 | text selection; Starlight accent-low |
| `plate` | #eeeadd | #0a0b09 | code plate, image background in docs |
| `stop` | #b3261e | #ff6a57 | the one red, for danger asides in the docs |

Measured contrast (WCAG):

| Pair | Light | Dark |
|---|---|---|
| ink on ground | 13.18 | 11.06 |
| dim on ground | 6.50 | 6.34 |
| accent on ground | 5.80 | 8.96 |
| accent on select | 4.67 (lowest) | 5.99 |

The docs also use a few Starlight-only tints in `theme.css` (`accent-high` #5c3800 / #f0c062, `red-high`, `red-low`, and a gray scale from `ground` to `bold`). They stay tied to Starlight's variables and are not general tokens. The home page's `theme-color` meta tags carry the two ground values.

### Named Rules
**The Bold Rule.** Bold is the strongest ink, as a terminal shows groff's bold: bold weight and the `bold` colour together. In the docs, `strong` and `b` take Starlight's white, which is mapped to `bold`. Underline is groff's italic, for paths.

**The One Amber Rule.** Amber is links, the focus ring, the caret and `accent-color`. The only amber-family fill is the pale `select` tint for selection and Starlight's low-accent backgrounds.

## Typography

**Font:** JetBrains Mono Variable (weights 400 to 700), self-hosted through `@fontsource-variable/jetbrains-mono`, with `ui-monospace`, DejaVu Sans Mono and `monospace` behind it. The home page preloads the latin woff2. Starlight's `--sl-font` and `--sl-font-mono` both point at it.

**Character:** one monospace for words and commands alike, with bold doing the work size does elsewhere.

### Hierarchy
- **Home body** (400, 16px, 1.55; 14px under 40rem): all home text. Section names, NAME and the h1 are also 1em; section names are 700.
- **Running head and foot** (400, same size, `dim`): three parts, space-between with a 2ch gap. The middle part is hidden under 40rem on the home page.
- **Docs title** (700, 1.75rem, letter-spacing 0): the h1 under the running head, which is 0.875-size Starlight small text in gray-3.
- **Docs body** (line height 1.65): `--sl-text-body` drops to 0.9375rem under 50rem. h2 is 1.125rem, bold, uppercase, 0.02em tracking; h3 and h4 are 1rem bold.
- **Code** (inherits the page font): home `pre` wraps with `pre-wrap` and `overflow-wrap: anywhere`; docs code is 0.9375rem. Code inside docs headings loses its chip and uppercase.

### Named Rules
**The One Face Rule.** JetBrains Mono is the only face. Do not add a second text face or a display face.

## Layout

Home: body padding `1.5rem 0 0`, header, main and footer at most 84ch wide with 2ch side padding. Section names sit at the left edge; paragraphs, `pre`, definition lists, tables, figures and the h1 are indented 7ch. A block following a block gets 1em above, and consecutive `pre` 0.25em. The definition list in SAFETY is an 11ch term column and a flexible description column. The still is at most 70ch wide. Tables have collapsed borders and 3ch column gaps; the call table right-aligns its last column.

Docs: Starlight's layout, with content width 48rem and header and sidebar on the ground colour.

**Phones.** Home at 40rem and below: base size 14px, indent 3ch, the definition list becomes one column, the running head's middle part and the less line's long title are hidden. Docs at 50rem and below: smaller body, halved cell padding, reference code names may break. At 30rem the docs running head and foot hide their middle part. On touch screens (`pointer: coarse`) docs heading anchors and code copy buttons are 44px.

**Print.** Home prints black on white at 11pt, without the less line and skip link, with each outside link in `main` followed by its address in angle brackets.

**No JavaScript on the home page.** Navigation is anchors; the clip opens from a link; version, date, release rows and clip size and length are read at build time.

## Elevation & Depth

Flat. There are no shadows, gradients or blur. Separation is the code plate (a `plate` fill with 1ch side padding), a 1px `rule` line (the still's frame, the docs foot's top border, Starlight hairlines) and reverse video.

### Named Rules
**The Reverse Video Rule.** The current place is shown as ground text on an ink fill. The home page's less line does this, and so does the docs sidebar's current page.

## Shapes

Square. The home page has no radius. The docs set 2px on code blocks (the expressiveCode `borderRadius`) and on images, so a still sits on the plate with a barely rounded corner.

## Components

### Running Head and Foot
Home: the header repeats `NIRI-COMPUTER-USE(1)` on both sides around "User Commands". The footer has a link to the release (`niri-computer-use <version>`), the release date from the changelog and the name. Docs: `PageTitle.astro` prints the page name and section on both sides (`SAFETY(7)`) around "niri-computer-use manual", hidden from assistive technology, then the real h1. `Footer.astro` adds the default Starlight footer, then a rule and the same release link, date and page name. Names come from `manName`: the last path part in capitals, a group's name for its overview page, section 1 for `reference/cli`, 5 for `concepts/configuration`, and 7 for the rest. The footer's link is amber, the rest `dim`.

### Section
A bold capital name at the left edge with 1.5em above, then text indented 7ch. NAME is a styled paragraph (`.sh`, hidden from assistive technology) over the page's h1, so the h1 is the line `niri-computer-use - <summary>`, indented 7ch. SYNOPSIS lists the bold commands separated by ` | `.

### Less Line
A sticky `nav` at the bottom: `ink` fill, `ground` text, links in `ground`. It holds "Manual page niri-computer-use(1)", a percentage, and four links (install, safety, docs, source). Where `animation-timeline: scroll()` is supported, a registered integer `--ncu-read` runs from 0 to 100 as the page scrolls and the percentage is printed from it; elsewhere the percentage is absent. Focus rings on its links use the ground colour. `scroll-padding-bottom` of 3.5rem keeps anchors clear of it.

### Links
Amber with a 1px underline offset 3px, 2px on hover. In docs, links in the article use Starlight's accent text colour with the same offsets. `.refs a` stays on one line. See-also entries read like `safety(7)`.

### Code Plate
A `pre` on `plate`, text in `bold`, indented 7ch with 0.15em 1ch padding. Docs code blocks use expressiveCode with themes `github-dark` and `github-light`, no frame, wrapped lines, no border, `codeBackground` set to `plate` and no frame shadow.

### Definition List and Tables
SAFETY is a definition list of bold terms and descriptions. The example call table (`call`, `ms`) and the history table (version link, date, summary) are plain, with tabular numerals. Docs reference tables keep the first column's code on one line above 50rem.

### Still
One figure: the still links to the clip, in a 1px `rule` frame, loaded eagerly at high priority as AVIF with a JPG fallback. The caption states the clip's length, format and size, read at build time. No autoplay and no overlay.

### Selection, Focus, Caret
Selection is `select` with `bold` text. Focus is a 2px amber outline offset 2px on `:focus-visible`, as niri rings the focused column. The caret and `accent-color` are amber. The scrollbar uses `rule` on `ground`.

### Motion
Docs pages crossfade by view transition when `prefers-reduced-motion` is no-preference, with the header and site title named so they hold still. The less percentage is the only other motion, and it is tied to scrolling.

## Do's and Don'ts

### Do:
- **Do** take every colour from `tokens.css`; the home page and docs share it.
- **Do** keep the running head and foot, with version and date taken from the build.
- **Do** write section names in capitals and indent their text by 7ch (3ch under 40rem).
- **Do** keep the focus ring amber, 2px, offset 2px; on the less line it is the ground colour.
- **Do** keep the home page working without JavaScript and printable, with link addresses after outside links.
- **Do** check any new colour pair for contrast. The lowest recorded pair is accent on select, 4.67 in light.
- **Do** say only what is true: the version, dates, release rows and clip facts come from the build.

### Don't:
- **Don't** add a second typeface or a display face.
- **Don't** add cards, shadows, gradients, glow, blur or hero blocks.
- **Don't** add a theme switch or JavaScript to the home page.
- **Don't** use amber as a fill beyond the select tint.
- **Don't** put kickers, eyebrows or small uppercase labels above headings. Capital section names are the heading itself.
- **Don't** autoplay video or put a play overlay on the still.

Not canonized, as defects or drift the build carries: the 2px radius on docs code and images (the home page is square; a future surface should be square); the Starlight-only tints in `theme.css`, tied to Starlight's variables; the stop red's contrast, which is not in the measured set.
