---
name: niri-computer-use
description: The manual page niri-computer-use(1) as man shows it in a terminal, set in JetBrains Mono, for the home page and the docs. On a wide screen the home page sits in a niri workspace.
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
  column-man: "88ch"
  column-example: "50rem"
  column-gap: "16px"
  column-ring: "4px"
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
  column:
    backgroundColor: "{colors.ground}"
    textColor: "{colors.ink}"
  workspace-backdrop:
    backgroundColor: "{colors.plate}"
  link:
    textColor: "{colors.accent}"
  selection:
    backgroundColor: "{colors.select}"
    textColor: "{colors.bold}"
---

# Design System: niri-computer-use

One system covers the home page (`site/src/pages/index.astro`, words in `src/lib/home.ts`), the 404 (`src/pages/404.astro`) and the Starlight docs. All take their colours from `src/styles/tokens.css`. The home page and the 404 share `src/styles/man.css`; the pager line, less help dialog and keys come from `src/components/Less.astro` and `src/scripts/less.ts`. The docs map them onto Starlight in `src/styles/theme.css`, and the docs running head and foot are `src/components/PageTitle.astro` and `Footer.astro`. Running-head names come from `src/lib/man.ts`.

## Overview

**Creative North Star: "The Manual Page in a Terminal"**

The home page is the manual page `niri-computer-use(1)` as man shows it in a terminal. It has a running head and foot, capital section names (NAME, SYNOPSIS, DESCRIPTION, INSTALLATION, SAFETY, EXAMPLE, HISTORY, BUGS, SEE ALSO), bold for what you type, underline for paths, and less's reverse-video status line, the pager line, pinned to the bottom of the window. On a screen at least 75rem wide the page is also a niri workspace: the man page is the focused column in a 4px amber ring, and EXAMPLE is the next column, peeking in at the right. The docs are the rest of the manual: each page opens on a running head and ends on a running foot, and its section headings are capitals.

Everything is set in JetBrains Mono Variable. Dark is a warm black with parchment ink, light is paper with brown-black ink, and one amber marks links and focus. The home page follows the system colour scheme (`prefers-color-scheme`, `color-scheme: light dark`). The docs follow Starlight's theme switch, which sets `data-theme` on the root. One script, `src/scripts/less.ts`, adds less's keys, search, copy buttons, column focus and the example's replay. The home page, the 404 and the docs read and work without it.

**Key Characteristics:**
- One face, JetBrains Mono Variable 400 and 700; ligatures are off on the home page.
- One `light-dark()` palette for both themes; amber is the only accent.
- Text is indented 7ch under its section name, in a column at most 84ch wide (80ch plus 2ch padding each side).
- Running head and foot on the home page and on every docs page.
- The pager line: reverse video, sticky, with a percentage that follows scrolling, `(END)` at the bottom, and messages and the search prompt in the same line.
- Wide screens: two columns on the plate backdrop, 16px apart, the focused one in a 4px amber ring and the other in a 4px `rule` ring; the strip slides on niri's spring.
- Narrow screens: one column, with EXAMPLE after the sections.
- Print is black on white, with each outside link's address after it.

## Colors

A warm, low-saturation palette. Each token has a light and a dark value, picked by `light-dark()`.

| Token | Light | Dark | Use |
|---|---|---|---|
| `ground` | #f7f5ee | #10110f | page ground; text on the pager line |
| `ink` | #2b2a25 | #c9c6b9 | body text; pager line background; search matches and the call in flight |
| `bold` | #0c0c0a | #f4f1e6 | groff bold, section names, code, table heads, selection text |
| `dim` | #5c584e | #9a9588 | running head and foot, captions, copy button, calls still to come |
| `rule` | #d4cfbf | #2f2d27 | hairlines, still frame, scrollbar track, the unfocused column's ring |
| `accent` | #8a5300 | #e3a83c | links, focus ring, the focused column's ring, caret, `accent-color` |
| `select` | #f0dca8 | #3a3220 | text selection; Starlight accent-low |
| `plate` | #eeeadd | #0a0b09 | code plate, copy button, image background in docs, the workspace backdrop on wide screens |
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

**The One Amber Rule.** Amber is links, the focus ring, the focused column's ring, the caret and `accent-color`. The only amber-family fill is the pale `select` tint for selection and Starlight's low-accent backgrounds.

## Typography

**Font:** JetBrains Mono Variable (weights 400 to 700), self-hosted through `@fontsource-variable/jetbrains-mono`, with `ui-monospace`, DejaVu Sans Mono and `monospace` behind it. The home page preloads the latin woff2. Starlight's `--sl-font` and `--sl-font-mono` both point at it.

**Character:** one monospace for words and commands alike, with bold doing the work size does elsewhere.

### Hierarchy
- **Home body** (400, 16px, 1.55; 14px under 40rem): all home and 404 text, and the less help dialog. Section names, NAME and the h1 are also 1em; section names are 700.
- **Running head and foot** (400, same size, `dim`): three parts, space-between with a 2ch gap. The middle part is hidden under 40rem on the home page.
- **Docs title** (700, 1.75rem, letter-spacing 0): the h1 under the running head, which is 0.875-size Starlight small text in gray-3.
- **Docs body** (line height 1.65): `--sl-text-body` drops to 0.9375rem under 50rem. h2 is 1.125rem, bold, uppercase, 0.02em tracking; h3 and h4 are 1rem bold.
- **Code** (inherits the page font): home `pre` wraps with `pre-wrap` and `overflow-wrap: anywhere`; docs code is 0.9375rem. Code inside docs headings loses its chip and uppercase.

### Named Rules
**The One Face Rule.** JetBrains Mono is the only face. Do not add a second text face or a display face.

## Layout

Home, narrow (under 75rem): one column; the running head, sections, EXAMPLE, foot and pager line stack, with EXAMPLE after the sections and before the foot. Body padding `1.5rem 0 0`, header, main and footer at most 84ch wide with 2ch side padding. Section names sit at the left edge; paragraphs, `pre`, definition lists, tables, figures and the h1 are indented 7ch. A block following a block gets 1em above, and consecutive `pre` 0.25em. The definition list in SAFETY is an 11ch term column and a flexible description column. The still is at most 70ch wide. Tables have collapsed borders and 3ch column gaps; the call table right-aligns its last column.

Home, wide (75rem and up): a niri workspace. The body is the `plate` backdrop. A strip of columns sits 16px from the top and left, with 16px gaps. The man page column is 88ch wide; the EXAMPLE column is 50rem wide, sticky 16px from the top, at most the window height less 32px, and scrolls on its own. Columns are `ground` with a 4px outline: amber on the focused column, `rule` on the other. The man page is focused first; EXAMPLE is focused by `#example` in the address, or by `data-column` set by the script. Focusing EXAMPLE translates the strip left until its right edge meets the window's, on niri's spring over 0.4s. Under `prefers-reduced-motion: reduce` the strip and ring do not transition.

404: the same page as the home man page, at most 88ch wide and at least the window high, with the pager line at the bottom. The h1 reads "No manual entry for <path>" in bold, with the path filled in by the script (without it, "No manual entry"). SEE ALSO lists every page, as `man -k` does: the page name and its one-line description, one row each, stacked per page under 40rem.

Docs: Starlight's layout, with content width 48rem and header and sidebar on the ground colour.

**Phones.** Home at 40rem and below: base size 14px, indent 3ch, the definition list becomes one column, the running head's middle part and the pager line's long title are hidden. Docs at 50rem and below: smaller body, halved cell padding, reference code names may break. At 30rem the docs running head and foot hide their middle part. On touch screens (`pointer: coarse`) docs heading anchors and code copy buttons are 44px.

**Print.** Home prints black on white at 11pt, without the pager line, copy buttons and skip link, with each outside link in `main` followed by its address in angle brackets.

**Progressive enhancement.** Navigation is anchors; the clip opens from a link; version, date, release rows and clip size and length are read at build time. `less.ts` only adds to this. It is 2.1 KB gzipped, with Starlight's 1.1 KB prefetch script as the only other JavaScript. The home page weighs about 53 KB.

## Elevation & Depth

Flat. There are no shadows, gradients or blur. Separation is the code plate (a `plate` fill with 1ch side padding), a 1px `rule` line (the still's frame, the docs foot's top border, Starlight hairlines) and reverse video.

### Named Rules
**The Reverse Video Rule.** The current place is shown as ground text on an ink fill. The pager line and the search matches do this, and so does the docs sidebar's current page.

## Shapes

Square. The home page has no radius. The docs set 2px on code blocks (the expressiveCode `borderRadius`) and on images, so a still sits on the plate with a barely rounded corner.

## Components

### Running Head and Foot
Home: the header repeats `NIRI-COMPUTER-USE(1)` on both sides around "User Commands". The footer has a link to the release (`niri-computer-use <version>`), the release date from the changelog and the name. Docs: `PageTitle.astro` prints the page name and section on both sides (`SAFETY(7)`) around "niri-computer-use manual", hidden from assistive technology, then the real h1. `Footer.astro` adds the default Starlight footer, then a rule and the same release link, date and page name. Names come from `manName`: the last path part in capitals, a group's name for its overview page, section 1 for `reference/cli`, 5 for `concepts/configuration`, and 7 for the rest. The footer's link is amber, the rest `dim`.

### Section
A bold capital name at the left edge with 1.5em above, then text indented 7ch. NAME is a styled paragraph (`.sh`, hidden from assistive technology) over the page's h1, so the h1 is the line `niri-computer-use - <summary>`, indented 7ch. SYNOPSIS lists the bold commands separated by ` | `.

### Less Line (pager line)
A sticky `nav` at the bottom, labelled "Pager": `ink` fill, `ground` text, links in `ground`. On the home page it holds "Manual page niri-computer-use(1)", a percentage, and links to install, safety, example and docs, then a help button the script reveals. On the 404 it reads "Manual page index" with links home and docs. The percentage is replaced by `(END)` at the bottom of the page. A status message from the script (`copied: <command>`, `Pattern not found`, `No previous search pattern`, `Nothing to go back to`, `Copy failed: the browser refused the clipboard`) replaces the prompt in the same line. `/` opens the search prompt in the line, with a `/` label and an input; Enter searches, Escape leaves it. Matches are marked by the `find` highlight in reverse video (`ink` fill, `ground` text); `n` and `N` step through them and scroll the match to the top line. Search is on the home page and the 404; the docs keep `/` for Starlight's search. Where `animation-timeline: scroll()` is supported, a registered integer `--ncu-read` runs from 0 to 100 as the page scrolls and the percentage is printed from it; elsewhere the percentage is absent. Focus rings on its links use the ground colour. `scroll-padding-bottom` of 3.5rem keeps anchors clear of it.

### Less Help
`Less.astro` adds a full-window `dialog` (ground fill, ink text) that `h`, `?` or the help button opens. Its title is SUMMARY OF LESS COMMANDS, then a table of keys in bold with what each does, then a button to turn the keys off or on, and a reverse-video line at the bottom, "HELP -- Press q when done", with a close button. The keys: `j e`, `k y`, `f`, `b`, `d u`, `g <`, `G >`, `/pattern`, `n N`, `h ?`, `q`, and `→ ←` for the columns on the home page. They are ignored while typing in a field or video, and with Ctrl, Alt or Super held. The button "Turn these keys off" stores the choice in `localStorage` and applies on every page, which satisfies WCAG 2.1.4. `q` closes the help, or else goes back a page when the visitor came from this site.

### Columns
Focus moves between the man page and EXAMPLE by `→` and `←`, by clicking or tabbing into a column, by a horizontal wheel or trackpad swipe, and by the example links (`#example`). The keys scroll the focused column. Under 75rem the arrows do nothing and the page is one column.

### Copy Button
The script adds a `copy` button to each command, in the plate's top right corner: `plate` fill, `dim` text, `bold` on hover. It copies the command and reports it in the pager line. Hidden in print.

### Example Replay
The still links to the clip. The script replaces the still with the clip, muted and with controls, and plays it in place; the call table's rows follow the clip. A row is `next` (calls to come, `dim`), `now` (the call in flight, reverse video) or `done` (made, `bold`), which fade over 0.15s. The call table has three columns: at (seconds), call and ms, with the first and last right-aligned.

### Links
Amber with a 1px underline offset 3px, 2px on hover. In docs, links in the article use Starlight's accent text colour with the same offsets. `.refs a` stays on one line. See-also entries read like `safety(7)`.

### Code Plate
A `pre` on `plate`, text in `bold`, indented 7ch with 0.15em 1ch padding. Docs code blocks use expressiveCode with themes `github-dark` and `github-light`, no frame, wrapped lines, no border, `codeBackground` set to `plate` and no frame shadow.

### Definition List and Tables
SAFETY is a definition list of bold terms and descriptions. The example call table (`call`, `ms`) and the history table (version link, date, summary) are plain, with tabular numerals. Docs reference tables keep the first column's code on one line above 50rem.

### Still
One figure in the EXAMPLE column, at most 70ch wide on the narrow layout and the column's width on the wide one: the still links to the clip, in a 1px `rule` frame, loaded eagerly at high priority as AVIF with a JPG fallback. The caption states the clip's length, format and size, read at build time, with a "Play the clip" link. No autoplay and no overlay; the clip plays only after a click.

### Selection, Focus, Caret
Selection is `select` with `bold` text. Focus is a 2px amber outline offset 2px on `:focus-visible`, as niri rings the focused column. The caret and `accent-color` are amber. The scrollbar uses `rule` on `ground`.

### Motion
`--ncu-spring` is niri's default spring (damping ratio 1, stiffness 800) over its 0.4s, written as a `linear()` easing in `tokens.css`. All motion is off under `prefers-reduced-motion: reduce`.
- Pages crossfade by view transition (cross-document, `@view-transition { navigation: auto }`), with the docs header named so it holds still.
- Between the home page and the docs they slide instead: the page goes left when entering the docs and right coming back, on the spring over 0.4s. `Slide.astro` is an inline head script on the `pagereveal` event that adds the `left` or `right` transition type; the home page, the 404 and the docs all include it.
- On a wide screen the strip slides on the spring over 0.4s, and the column ring changes colour over 0.2s.
- The replay's row states fade over 0.15s.
- The less percentage counts with scrolling.

## Do's and Don'ts

### Do:
- **Do** take every colour from `tokens.css`; the home page and docs share it.
- **Do** keep the running head and foot, with version and date taken from the build.
- **Do** write section names in capitals and indent their text by 7ch (3ch under 40rem).
- **Do** keep the focus ring amber, 2px, offset 2px; on the pager line it is the ground colour.
- **Do** keep the home page, 404 and docs readable and usable without JavaScript, and printable, with link addresses after outside links. Scripts only add to them.
- **Do** keep every key off when the visitor turns it off, and ignore keys in fields and with modifiers.
- **Do** put motion on `--ncu-spring` and switch it off under `prefers-reduced-motion`.
- **Do** check any new colour pair for contrast. The lowest recorded pair is accent on select, 4.67 in light.
- **Do** say only what is true: the version, dates, release rows and clip facts come from the build.

### Don't:
- **Don't** add a second typeface or a display face.
- **Don't** add cards, shadows, gradients, glow, blur or hero blocks.
- **Don't** add a theme switch.
- **Don't** let the script own content or navigation: a link or a section must work without it.
- **Don't** use amber as a fill beyond the select tint.
- **Don't** put kickers, eyebrows or small uppercase labels above headings. Capital section names are the heading itself.
- **Don't** autoplay video or put a play overlay on the still.

Not canonized, as defects or drift the build carries: the 2px radius on docs code and images (the home page is square; a future surface should be square); the Starlight-only tints in `theme.css`, tied to Starlight's variables; the stop red's contrast, which is not in the measured set.
