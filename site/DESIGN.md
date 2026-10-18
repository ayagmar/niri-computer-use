---
name: niri-computer-use
description: The manual page niri-computer-use(1) as man shows it in a terminal, set in JetBrains Mono, for the home page and the docs. The home page opens on a drawing of a real session's errand, in the agent's own screenshot pixels, that plays its calls at their real times.
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
  wordmark:
    fontFamily: "'JetBrains Mono Variable', ui-monospace, 'DejaVu Sans Mono', monospace"
    fontSize: "clamp(2.1rem, 6.4vw, 4.6rem)"
    fontWeight: 800
    lineHeight: 1
    letterSpacing: "-0.03em"
  tagline:
    fontFamily: "'JetBrains Mono Variable', ui-monospace, 'DejaVu Sans Mono', monospace"
    fontSize: "clamp(1.05rem, 1.8vw, 1.35rem)"
    fontWeight: 400
    lineHeight: 1.4
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
  home-gutter: "16ch"
  home-measure: "76ch"
  stage-width: "92rem"
  window-gap: "16px"
  window-ring: "4px"
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
  window:
    backgroundColor: "{colors.ground}"
    textColor: "{colors.ink}"
  stage:
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

The home page is the manual page `niri-computer-use(1)` as man shows it in a terminal. It has a running head and foot, capital section names (NAME, SYNOPSIS, DESCRIPTION, INSTALLATION, SAFETY, EXAMPLE, HISTORY, BUGS, SEE ALSO), bold for what you type, underline for paths, and less's reverse-video status line, the pager line, pinned to the bottom of the window. It opens on the name at display size, the commands and the install line, then EXAMPLE: the errand drawn in the agent's own 1280 by 720 screenshot pixels, with rulers, a terminal where the real calls appear and Files where they land. The other sections follow with their names hanging in a left gutter. The docs are the rest of the manual: each page opens on a running head and ends on a running foot, and its section headings are capitals.

Everything is set in JetBrains Mono Variable. Dark is a warm black with parchment ink, light is paper with brown-black ink, and one amber marks links and focus. The home page follows the system colour scheme (`prefers-color-scheme`, `color-scheme: light dark`) until the visitor picks one with the switch in its running head or Starlight's in the docs; both keep the choice in `localStorage` under `starlight-theme` and set `data-theme` on the root, and an inline head script applies it before the page paints. `src/scripts/less.ts` adds less's keys, search and copy buttons, `src/scripts/stage.ts` plays the drawn errand, and `src/scripts/top.ts` adds the theme switch and the star count. The home page, the 404 and the docs read and work without them.

**Key Characteristics:**
- One face, JetBrains Mono Variable 400 to 800; ligatures are off on the home page. Only the home page's name is set at display size.
- One `light-dark()` palette for both themes; amber is the only accent.
- Docs and the 404 indent text 7ch under its section name. The home page hangs section names in a 16ch gutter beside a 76ch measure, and stacks them above the text under 48rem.
- Running head and foot on the home page and on every docs page.
- The pager line: reverse video, sticky, with a percentage that follows scrolling, `(END)` at the bottom, and messages and the search prompt in the same line.
- EXAMPLE is a line drawing in SVG, viewBox 1280 by 720, the space the agent's screenshots and clicks use: a `ground` bar, two `ground` windows in 4px rings (amber on the focused one, `rule` on the other), icons as 1.5px `ink` strokes, and the pointer as an amber dot with dashed guides across the focused window and its coordinates beside it. Rulers in those pixels run along the top and left.
- Print is black on white, with each outside link's address after it.

## Colors

A warm, low-saturation palette. Each token has a light and a dark value, picked by `light-dark()`.

| Token | Light | Dark | Use |
|---|---|---|---|
| `ground` | #f7f5ee | #10110f | page ground; text on the pager line |
| `ink` | #2b2a25 | #c9c6b9 | body text; pager line background; search matches and the call in flight |
| `bold` | #0c0c0a | #f4f1e6 | groff bold, section names, code, table heads, selection text |
| `dim` | #5c584e | #9a9588 | running head and foot, captions, copy button, calls still to come |
| `rule` | #d4cfbf | #2f2d27 | hairlines, rulers, scrollbar track, the unfocused window's ring |
| `accent` | #8a5300 | #e3a83c | links, focus ring, the focused window's ring, caret, `accent-color` |
| `select` | #f0dca8 | #3a3220 | text selection; Starlight accent-low |
| `plate` | #eeeadd | #0a0b09 | code plate, copy button, image background in docs, the drawing's wallpaper and Files sidebar |
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

**The One Amber Rule.** Amber is links, the focus ring, the focused window's ring, the caret and `accent-color`. The only amber-family fill is the pale `select` tint for selection and Starlight's low-accent backgrounds.

## Typography

**Font:** JetBrains Mono Variable (weights 400 to 700), self-hosted through `@fontsource-variable/jetbrains-mono`, with `ui-monospace`, DejaVu Sans Mono and `monospace` behind it. The home page preloads the latin woff2. Starlight's `--sl-font` and `--sl-font-mono` both point at it.

**Character:** one monospace for words and commands alike, with bold doing the work size does elsewhere.

### Hierarchy
- **Home body** (400, 16px, 1.55; 17px from 64rem, 14px under 40rem): all home and 404 text, and the less help dialog. Section names are 1em and 700.
- **Wordmark** (800, `clamp(2.1rem, 6.4vw, 4.6rem)`, line height 1, -0.03em): the name in the home page's h1, in `bold`. The tagline under it is 400, `clamp(1.05rem, 1.8vw, 1.35rem)`, 1.4, balanced. DESCRIPTION's paragraph is 1.1rem and the call table 0.875rem.
- **Running head and foot** (400, same size, `dim`): three parts, space-between with a 2ch gap. The middle part is hidden under 40rem on the home page.
- **Docs title** (700, 1.75rem, letter-spacing 0): the h1 under the running head, which is 0.875-size Starlight small text in gray-3.
- **Docs body** (line height 1.65): `--sl-text-body` drops to 0.9375rem under 50rem. h2 is 1.125rem, bold, uppercase, 0.02em tracking; h3 and h4 are 1rem bold.
- **Code** (inherits the page font): home `pre` wraps with `pre-wrap` and `overflow-wrap: anywhere`; docs code is 0.9375rem. Code inside docs headings loses its chip and uppercase.

### Named Rules
**The One Face Rule.** JetBrains Mono is the only face. Do not add a second text face or a display face.

## Layout

Home: the running head, the opening and the drawing are `min(100% - 4ch, 92rem)` wide; the drawing is also at most as wide as lets it fit the window height less 6rem. The opening starts `clamp(2rem, 7vh, 5rem)` below the running head: the wordmark and the tagline 1rem under it across the top, then a grid of the commands on the left and, in a column at most 60ch wide, the install line in a plate with a 1px `rule` frame and the facts line in `dim` with links to INSTALLATION and SAFETY. The install line breaks only after a slash. The drawing follows 2.5rem below, then its line: EXAMPLE in `bold`, the caption in `dim`, and "Watch the recording" with the clip's facts and the replay button. The other sections, and the running foot, sit in a centred column `min(100% - 4ch, 16ch + 76ch)` wide: each section is a grid of a 16ch gutter, where its name sits, and its text, 4.5rem apart. SAFETY is a two-column grid of term and description, then the sandbox line under a hairline. Tables have collapsed borders. The pager line's inline padding lines its text up with that column.

Narrow: under 60rem the calls leave the drawn terminal and follow the drawing as a list, with calls still to come in `dim`. Under 48rem the gutter goes: section names sit above their text, text is indented 3ch, the install line drops under the commands, and SAFETY is one column. Under 40rem the rulers hide.

404: the same page as the home man page, at most 88ch wide and at least the window high, with the pager line at the bottom. The h1 reads "No manual entry for <path>" in bold, with the path filled in by the script (without it, "No manual entry"). SEE ALSO lists every page, as `man -k` does: the page name and its one-line description, one row each, stacked per page under 40rem.

Docs: Starlight's layout, with content width 48rem and header and sidebar on the ground colour.

**Phones.** Home at 40rem and below: base size 14px, indent 3ch, the definition list becomes one column, the running head's middle part and the pager line's long title are hidden. Docs at 50rem and below: smaller body, halved cell padding, reference code names may break. At 30rem the docs running head and foot hide their middle part. On touch screens (`pointer: coarse`) docs heading anchors and code copy buttons are 44px.

**Print.** Home prints black on white at 11pt, without the pager line, copy buttons and skip link, with each outside link in `main` followed by its address in angle brackets.

**Progressive enhancement.** Navigation is anchors; the clip opens from a link; version, date, release rows and clip size and length are read at build time. `less.ts` only adds to this. It is 2.3 KB gzipped, with Starlight's 1.1 KB prefetch script as the only other JavaScript. The home page weighs about 53 KB.

## Elevation & Depth

Flat. There are no shadows, gradients or blur. Separation is the code plate (a `plate` fill with 1ch side padding), a 1px `rule` line (the still's frame, the docs foot's top border, Starlight hairlines) and reverse video.

### Named Rules
**The Reverse Video Rule.** The current place is shown as ground text on an ink fill. The pager line and the search matches do this, and so does the docs sidebar's current page.

## Shapes

Square. The home page has no radius. The docs set 2px on code blocks (the expressiveCode `borderRadius`) and on images, so a still sits on the plate with a barely rounded corner.

## Components

### Running Head and Foot
Home: the header has `NIRI-COMPUTER-USE(1)` and "User Commands", then on the right a GitHub link, with GitHub's star count in `dim` after it when GitHub answers within 4s, and the theme switch, an underlined amber button naming the other theme. The footer has a link to the release (`niri-computer-use <version>`), the release date from the changelog and the name. Docs: `PageTitle.astro` prints the page name and section on both sides (`SAFETY(7)`) around "niri-computer-use manual", hidden from assistive technology, then the real h1. `Footer.astro` adds the default Starlight footer, then a rule and the same release link, date and page name. Names come from `manName`: the last path part in capitals, a group's name for its overview page, section 1 for `reference/cli`, 5 for `concepts/configuration`, and 7 for the rest. The footer's link is amber, the rest `dim`.

### Section
A bold capital name at the left edge with 1.5em above, then text indented 7ch. NAME is a styled paragraph (`.sh`, hidden from assistive technology) over the page's h1, so the h1 is the line `niri-computer-use - <summary>`, indented 7ch. SYNOPSIS lists the bold commands separated by ` | `.

### Less Line (pager line)
A sticky `nav` at the bottom, labelled "Pager": `ink` fill, `ground` text, links in `ground`. On the home page it holds "Manual page niri-computer-use(1)", a percentage, and links to install, safety, example and docs, then a help button the script reveals. On the 404 it reads "Manual page index" with links home and docs. The percentage is replaced by `(END)` at the bottom of the page. A status message from the script (`copied: <command>`, `Pattern not found`, `No previous search pattern`, `Nothing to go back to`, `Copy failed: the browser refused the clipboard`) replaces the prompt in the same line. `/` opens the search prompt in the line, with a `/` label and an input; Enter searches, Escape leaves it. Matches are marked by the `find` highlight in reverse video (`ink` fill, `ground` text); `n` and `N` step through them and scroll the match to the top line. Search is on the home page and the 404; the docs keep `/` for Starlight's search. Where `animation-timeline: scroll()` is supported, a registered integer `--ncu-read` runs from 0 to 100 as the page scrolls and the percentage is printed from it; elsewhere the percentage is absent. Focus rings on its links use the ground colour. `scroll-padding-bottom` of 3.5rem keeps anchors clear of it.

### Less Help
`Less.astro` adds a full-window `dialog` (ground fill, ink text) that `?` or the help button opens. Its title is SUMMARY OF LESS COMMANDS, then a table of keys in bold with what each does, then a button to turn the keys off or on, and a reverse-video line at the bottom, "HELP -- Press q when done", with a close button. The keys: `j e`, `k y`, `f`, `b`, `d u`, `g <`, `G >`, `/pattern`, `n N`, `?` and `q`. `h` is left free, as vim's left. They are ignored while typing in a field or video, and with Ctrl, Alt or Super held. The button "Turn these keys off" stores the choice in `localStorage` and applies on every page, which satisfies WCAG 2.1.4. `q` closes the help, or else goes back a page when the visitor came from this site.

### Copy Button
The script adds a `copy` button to each command, in the plate's top right corner: `plate` fill, `dim` text, `bold` on hover. It copies the command and reports it in the pager line. Hidden in print.

### The Drawn Errand
`components/Stage.astro` draws the errand and lists the session's calls over the drawn terminal, each with its time, its call and its round trip; `scripts/stage.ts` plays it. What the drawing shows at each moment comes from `lib/stage.ts`, a pure function of the time read from the calls in `lib/home.ts`: focus moves to Files with `focus` and back with `release_desktop`, the bar shows "agent holds the lease" between `acquire_desktop` and `release_desktop`, the pointer travels to each click over 0.5s and lands on the call's own coordinates with a growing ring, the two screenshots are selected, the right-click opens the menu, its first item opens the Folder Name dialog, `ctrl+a` selects its text, the name types out, and `release-0.1` replaces the two screenshots when `type_text` returns. Keys show as `bold` keycaps under Files. A call's line appears when it goes out and is in reverse video while in flight. The bar's clock counts in seconds, then says `(END)`; the loop rests 3s and starts again. It plays while 30% of the drawing is in view and pauses out of view. "Watch the recording" plays the clip in the drawing's frame, muted and with controls, and the call lines follow it; "back to the drawing" returns. Without the script, or under reduced motion, it shows the finished errand with every call listed, and the link opens the MP4.
### Links
Amber with a 1px underline offset 3px, 2px on hover. In docs, links in the article use Starlight's accent text colour with the same offsets. `.refs a` stays on one line. See-also entries read like `safety(7)`.

### Code Plate
A `pre` on `plate`, text in `bold`, indented 7ch with 0.15em 1ch padding. Docs code blocks use expressiveCode with themes `github-dark` and `github-light`, no frame, wrapped lines, no border, `codeBackground` set to `plate` and no frame shadow.

### Definition List and Tables
SAFETY is a definition list of bold terms and descriptions. The example call table (`call`, `ms`) and the history table (version link, date, summary) are plain, with tabular numerals. Docs reference tables keep the first column's code on one line above 50rem.

### Still
None on the home page: the recording is a link, and its facts (length, format and size) are read at build time.

### Selection, Focus, Caret
Selection is `select` with `bold` text. Focus is a 2px amber outline offset 2px on `:focus-visible`, as niri rings the focused column. The caret and `accent-color` are amber. The scrollbar uses `rule` on `ground`.

### Motion
`--ncu-spring` is niri's default spring (damping ratio 1, stiffness 800) over its 0.4s, written as a `linear()` easing in `tokens.css`. All motion is off under `prefers-reduced-motion: reduce`.
- Pages crossfade by view transition (cross-document, `@view-transition { navigation: auto }`), with the docs header named so it holds still.
- Between the home page and the docs they slide instead: the page goes left when entering the docs and right coming back, on the spring over 0.4s. `Slide.astro` is an inline head script on the `pagereveal` event that adds the `left` or `right` transition type; the home page, the 404 and the docs all include it.
- The drawn errand: window rings change colour over 0.25s, the pointer eases to each click over 0.5s, icons, the menu and the dialog fade over 0.15 to 0.35s, and call lines fade in over 0.2s.
- The less percentage counts with scrolling.

## Do's and Don'ts

### Do:
- **Do** take every colour from `tokens.css`; the home page and docs share it.
- **Do** keep the running head and foot, with version and date taken from the build.
- **Do** write section names in capitals: hung in the gutter on the home page, with text indented 7ch in the docs and the 404 (3ch on phones).
- **Do** keep the focus ring amber, 2px, offset 2px; on the pager line it is the ground colour.
- **Do** keep the home page, 404 and docs readable and usable without JavaScript, and printable, with link addresses after outside links. Scripts only add to them.
- **Do** keep every key off when the visitor turns it off, and ignore keys in fields and with modifiers.
- **Do** put motion on `--ncu-spring` and switch it off under `prefers-reduced-motion`.
- **Do** check any new colour pair for contrast. The lowest recorded pair is accent on select, 4.67 in light.
- **Do** say only what is true: the version, dates, release rows and clip facts come from the build.

### Don't:
- **Don't** add a second typeface or a display face; the wordmark is the same mono at 800.
- **Don't** add cards, shadows, gradients, glow or blur. The opening is the man page's NAME set large, not a marketing hero.
- **Don't** add a second theme switch: the home page's and Starlight's share one stored choice.
- **Don't** let the script own content or navigation: a link or a section must work without it.
- **Don't** use amber as a fill beyond the select tint.
- **Don't** put kickers, eyebrows or small uppercase labels above headings. Capital section names are the heading itself.
- **Don't** animate the drawn errand under reduced motion, or play the clip with sound or unasked.
- **Don't** draw the desktop as a fake screenshot: it stays a line drawing, and every change in it comes from a real call.

Not canonized, as defects or drift the build carries: the 2px radius on docs code and images (the home page is square; a future surface should be square); the Starlight-only tints in `theme.css`, tied to Starlight's variables; the stop red's contrast, which is not in the measured set.
