---
name: niri-computer-use
description: Condensed heavy posters on a cool concrete ground, real recordings on a black plate with their calls as subtitles, and one red for the stop key and a stopped answer.
colors:
  concrete-ground: "#e6e8e3"
  concrete-raised: "#dcdfd9"
  ink: "#0c0e0d"
  ink-quiet: "#464c48"
  concrete-ground-dark: "#161917"
  concrete-raised-dark: "#1f2320"
  ink-dark: "#e6e8e3"
  ink-quiet-dark: "#a5aca7"
  plate: "#0b0c0b"
  plate-dark: "#070807"
  plate-ink: "#e6e8e3"
  plate-ink-quiet: "#9ba39e"
  plate-control: "#1d201e"
  plate-control-hover: "#2b2f2c"
  stop-red: "#d8301f"
  stop-red-lip: "#8f1f13"
  stop-text: "#c42a1b"
  stop-text-dark: "#ff6a57"
  stop-text-plate: "#ff5a45"
  key-legend: "#ffffff"
typography:
  poster:
    fontFamily: "'Archivo Variable', ui-sans-serif, system-ui, sans-serif"
    fontSize: "clamp(3.75rem, 17vw, 28rem)"
    fontWeight: 860
    lineHeight: 0.86
    letterSpacing: "-0.035em"
    fontVariation: "'wdth' 70"
  footer-wordmark:
    fontFamily: "'Archivo Variable', ui-sans-serif, system-ui, sans-serif"
    fontSize: "calc(100cqi / 5.62)"
    fontWeight: 880
    lineHeight: 0.8
    letterSpacing: "-0.04em"
    fontVariation: "'wdth' 62"
  heading:
    fontFamily: "'Archivo Variable', ui-sans-serif, system-ui, sans-serif"
    fontSize: "clamp(2.5rem, 7vw, 6rem)"
    fontWeight: 820
    lineHeight: 0.9
    letterSpacing: "-0.03em"
    fontVariation: "'wdth' 72"
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
  wordmark:
    fontFamily: "'Archivo Variable', ui-sans-serif, system-ui, sans-serif"
    fontSize: "clamp(1.0625rem, 1rem + 0.3vw, 1.25rem)"
    fontWeight: 760
    letterSpacing: "-0.01em"
    fontVariation: "'wdth' 88"
  lede:
    fontFamily: "'Archivo Variable', ui-sans-serif, system-ui, sans-serif"
    fontSize: "clamp(1.125rem, 0.9rem + 0.6vw, 2rem)"
    fontWeight: 400
    lineHeight: 1.35
  sub-head:
    fontFamily: "'Archivo Variable', ui-sans-serif, system-ui, sans-serif"
    fontSize: "1.25rem"
    fontWeight: 600
    lineHeight: 1.25
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
  key-legend:
    fontFamily: "'Archivo Variable', ui-sans-serif, system-ui, sans-serif"
    fontSize: "0.32em"
    fontWeight: 700
    letterSpacing: "0.04em"
    fontVariation: "'wdth' 100"
  call:
    fontFamily: "'JetBrains Mono Variable', ui-monospace, 'DejaVu Sans Mono', monospace"
    fontSize: "clamp(0.875rem, 0.7rem + 0.6vw, 1.25rem)"
    lineHeight: 1.45
  command:
    fontFamily: "'JetBrains Mono Variable', ui-monospace, 'DejaVu Sans Mono', monospace"
    fontSize: "0.875rem"
    lineHeight: 1.6
rounded:
  pill: "999px"
  code: "0.75rem"
  still: "4px"
  key: "0.09em"
spacing:
  margin: "clamp(1rem, 3.2vw, 3rem)"
  touch: "2.75rem"
  plate-inset: "clamp(1rem, 3vw, 2.5rem)"
  section: "clamp(4rem, 10vh, 8rem)"
  section-open: "clamp(6rem, 18vh, 12rem)"
  stack: "2rem"
  stack-tight: "1.25rem"
components:
  button-primary:
    backgroundColor: "{colors.ink}"
    textColor: "{colors.concrete-ground}"
    rounded: "{rounded.pill}"
    padding: "0 1.75rem"
    height: "3.25rem"
  button-primary-hover:
    backgroundColor: "{colors.ink-quiet}"
    textColor: "{colors.concrete-ground}"
  button-plate:
    backgroundColor: "{colors.plate-control}"
    textColor: "{colors.plate-ink}"
    typography: "{typography.small}"
    rounded: "{rounded.pill}"
    padding: "0 1rem"
    height: "2.75rem"
    width: "4.5rem"
  button-plate-hover:
    backgroundColor: "{colors.plate-control-hover}"
  lease-tag-free:
    backgroundColor: "{colors.plate-control}"
    textColor: "{colors.plate-ink-quiet}"
    typography: "{typography.small}"
    rounded: "{rounded.pill}"
    padding: "0.15rem 0.6rem"
  lease-tag-held:
    backgroundColor: "{colors.plate-ink}"
    textColor: "{colors.plate}"
  lease-tag-stopped:
    backgroundColor: "{colors.stop-red}"
    textColor: "{colors.key-legend}"
  stop-key:
    backgroundColor: "{colors.stop-red}"
    textColor: "{colors.key-legend}"
    typography: "{typography.key-legend}"
    rounded: "{rounded.key}"
    padding: "0 0.2em"
    height: "0.74em"
  code-block:
    backgroundColor: "{colors.plate}"
    textColor: "{colors.plate-ink}"
    typography: "{typography.command}"
    rounded: "{rounded.code}"
    padding: "0.5rem 0.5rem 0.5rem clamp(1rem, 2vw, 1.25rem)"
  stage-bar:
    backgroundColor: "{colors.plate}"
    textColor: "{colors.plate-ink}"
    typography: "{typography.call}"
    padding: "0.9rem clamp(1rem, 3vw, 2.5rem) 1.1rem"
  nav-link:
    textColor: "{colors.ink}"
    typography: "{typography.body}"
    padding: "0 0.45rem"
    height: "2.75rem"
---

# Design System: niri-computer-use

## Overview

**Creative North Star: "Poster and Plate"**

Two materials carry the whole site. The concrete ground holds type set like a street poster: Archivo condensed to 62–72% width at weights 820–880, lines packed to 0.8–0.9 leading, sizes tied to the viewport width. The black plate holds the evidence: real desktop recordings, the calls under them in JetBrains Mono, copyable commands, and the footer. Nothing else is decorated. There are no shadows, gradients or ornaments on the home page; a section changes by changing material (ground, raised ground, plate), not by drawing a line.

The home page (`src/pages/index.astro`) sits outside Starlight and loads only its own styles. The docs are Starlight restyled through `src/styles/theme.css`: the same tokens, the same wordmark, Archivo titles a size down from the posters, and Starlight's greys remapped onto the concrete. Both read `src/styles/tokens.css`, so both themes (light and dark, stored under Starlight's `starlight-theme` key) are the same on every page, and the header and wordmark hold still while pages crossfade.

Motion follows the recordings rather than decorating the page: the frame in the headline grows into the first recording, subtitles and the lease tag change at each call's real time, and the stop key sinks when the recorded stop lands. Measured on the built home page: Lighthouse mobile Performance 100 on three runs, LCP 1.65–1.73 s, CLS 0, TBT 0, and 3.3 KB of JS gzipped.

**Key Characteristics:**
- One Archivo variable face from 62% to 100% width; width carries the hierarchy as much as size does.
- Root text at 17px (`html` font-size 1.0625rem), so every rem on the home page is 17px.
- Recordings, calls and commands always sit on the black plate, in both themes.
- One red, in four steps, reserved for the stop key and stopped or refused answers.
- Pill controls in ink on the ground, in a dark grey on the plate.
- Light and dark themes share the plate; only the ground, raised ground, inks and on-ground red change.

## Colors

A cool, slightly green-grey concrete with near-black ink, a black plate that does not change much between themes, and one red.

### Primary
- **Stop Red** (stop-red): the face of the Esc keycap in the stop headline and the background of the lease tag when the lease reads `stopped`. Text on it is white (key-legend).
- **Key Lip** (stop-red-lip): the keycap's bottom edge, a 0.08em border that shrinks to 0.02em when the key is pressed. It exists only on the key.
- **Stop Text on Ground** (stop-text, stop-text-dark in the dark theme): red as text or stroke on the concrete. It is the caret colour, Starlight's red (caution asides) on the docs, and the stopped path in the docs diagrams.
- **Stop Text on Plate** (stop-text-plate): a refused or stopped answer in the subtitles and the call list, set on the plate.

### Neutral
- **Concrete Ground** (concrete-ground, concrete-ground-dark): the page background. The meta `theme-color` uses the same two values.
- **Raised Concrete** (concrete-raised, concrete-raised-dark): one tonal step off the ground (darker in the light theme, lighter in the dark), used for the band behind "More real sessions" and as the docs' low-accent grey.
- **Ink** (ink, ink-dark): text, links, the Install pill, the focus outline, text selection (ink background, ground text) and form accents.
- **Quiet Ink** (ink-quiet, ink-quiet-dark): the Install pill's hover, the note under the clips, the scrollbar thumb.
- **Plate** (plate, plate-dark): recordings, their caption bars, command blocks, the footer, and the background behind docs stills. Darker than the dark ground, so a plate reads as a plate in the dark theme too.
- **Plate Ink** (plate-ink) and **Quiet Plate Ink** (plate-ink-quiet): text on the plate; the quiet step carries prompts, answers, times, notes and the commands the user ran.
- **Plate Control** (plate-control, plate-control-hover): the Play/Pause and Copy pills and the free lease tag. These two are written as literals in `Stage.astro` and `Code.astro`, not as custom properties.

The docs map Starlight's grey ramp onto these: in the dark theme from `#e6e8e3` (white) through `#d3d7d1`, `#b9bfba`, `#8a918c`, `#5d645f`, `#343a36`, `#1f2320` to `#161917` (black); in the light theme from `#0c0e0d` through `#1d211f`, `#2d332f`, `#464c48`, `#6c736e`, `#b8bdb7`, `#dcdfd9`, `#e1e4de` to `#e6e8e3`. The docs accent is grey, not a colour: links and the current page are ink.

### Named Rules
**The One Red Rule.** Red means the stop key, or an answer that was stopped or refused. Pick the step by surface: stop-red with stop-red-lip for the key face, stop-text on the ground, stop-text-plate on the plate. Never use red for emphasis, links or calls to action.

**The Plate Rule.** Anything recorded or copyable (a recording, its calls, a command) sits on the plate in plate ink, in both themes. The ground never carries monospace call text.

## Typography

**Display and Body Font:** Archivo Variable (with ui-sans-serif, system-ui, sans-serif)
**Mono Font:** JetBrains Mono Variable (with ui-monospace, 'DejaVu Sans Mono', monospace)

**Character:** one grotesque doing two jobs by width: condensed and very heavy for posters, full width and regular for reading. Mono appears only where the text is a call or a command someone could type.

Archivo is cut by `scripts/subset-font.mjs` to printable ASCII plus the copy's typographic punctuation (no-break space, ©, ·, ×, en and em dashes, curly quotes, ellipsis) and to the 62–100% width range, then declared after Fontsource's faces under the same name with matching weight and width ranges, so any other character falls back to the full face. The home page preloads the cut file.

### Hierarchy
- **Poster** (860, 70% width, clamp(3.75rem, 17vw, 28rem), 0.86): the home page's three section headlines, "Your agent drives niri.", "One key takes it back.", "Install". Each line is its own block.
- **Footer wordmark** (880, 62% width, 100cqi / 5.62, 0.8): the name across the full width of the footer, sized to the footer's container.
- **Heading** (820, 72% width, clamp(2.5rem, 7vw, 6rem), 0.9): section heads that are not posters ("More real sessions").
- **Docs title** (840, 72% width, clamp(2.5rem, 1.6rem + 3vw, 3.75rem), 0.95) and **Docs section** (760, 84% width): page titles and h2/h3 on docs pages.
- **Wordmark** (760, 88% width, clamp(1.0625rem, 1rem + 0.3vw, 1.25rem)): the site name in the header of every page.
- **Lede** (400, clamp(1.125rem, 0.9rem + 0.6vw, 2rem), 1.35, at most 30ch): the sentence under a poster.
- **Sub-head** (600–650, 1.25rem, 1.25): clip titles, install step labels, the stop facts.
- **Body** (400, 1rem = 17px, 1.5): running text and the prompts on the plate.
- **Small** (0.875rem, 1.45): notes, the call list, the lease tag, plate buttons. Notes stop at 46ch.
- **Key legend** (700, 100% width, 0.32em of the poster, uppercase, 0.04em): "Esc" on the keycap, and nowhere else.
- **Call** (JetBrains Mono, clamp(0.875rem, 0.7rem + 0.6vw, 1.25rem), 1.45): subtitles under a recording.
- **Command** (JetBrains Mono, 0.875rem, 1.6): copyable commands and the full call list.

### Named Rules
**The Width Ladder Rule.** Archivo is set only at 62%, 70%, 72%, 84%, 88% and 100% width; narrower is heavier and larger. A new role takes one of these steps; the cut face holds nothing outside 62–100%.

**The Mono for Machines Rule.** JetBrains Mono is for calls and commands only, never for labels, headings or decoration.

## Layout

One column with a fluid side margin (spacing.margin), shared by header, sections and footer. Sections are separated by vertical padding in viewport-height clamps (spacing.section, and spacing.section-open before the stop and install posters), not by lines. Recordings run edge to edge on a phone; on wide screens the stop recording bleeds off the right margin.

- **Hero:** the poster on two lines, with the recording's frame inline after "Your agent", 0.74em tall. Below 48rem the frame is an 8:9 crop of the still; from 48rem it is 16:9. On a screen wider than tall (aspect ratio at least 5:4 and at least 48rem wide) the hero fills the first view less the header, so the recording waits below it. The lede and Install button follow the poster after a clamp(2rem, 6vh, 4.5rem) gap, the lede left and the button right, and the spare height falls below them.
- **Stop:** poster with the keycap inline after "One key", then the facts and the recording. From 62rem they sit in two columns, minmax(16rem, 1fr) and 2fr.
- **Clips:** on the raised ground; one column, then two from 62rem with the layout clip spanning both.
- **Install:** poster, lede, three numbered steps each with a command, up to 60rem wide.
- **Footer:** on the plate, the full-width wordmark, then links.

Breakpoints are 48rem (frame and recording crop) and 62rem (two columns). Every interactive target is at least 2.75rem tall; on docs pages, heading anchors and copy buttons grow to that size under `pointer: coarse`. Docs content is 46rem wide.

**The Phone Crop Rule.** Below 48rem a recording shows the half of the desktop where the agent works (left, centre or right): the still is cut to that half at 8:9 and the video is drawn at twice the width and shifted, so the text in it stays legible.

## Elevation & Depth

Flat. Depth is material and order: the ground, a raised ground one step off it, and the plate. The only shape that suggests height is the keycap, whose lip is a solid bottom border in stop-red-lip, not a shadow. The recording rides one stacking level above the hero (z-index 1) so it can grow over the text.

### Named Rules
**The Material Not Line Rule.** On the home page a change of surface is a change of colour: no borders, outlines, dividers, shadows or gradients. The keycap's lip is the one border, and it is part of the key.

## Shapes

Pills (rounded.pill) for every button, the theme toggle and the lease tag. Command blocks have a 0.75rem radius. Recordings and their plates are square-cornered; docs stills on the plate are rounded 4px. The keycap is rounded 0.09em, so it scales with the poster. The theme icon is an inline SVG circle, half filled, that turns 180° in the dark theme.

## Components

### Buttons
- **Primary (Install):** ink pill on the ground, 3.25rem tall, 1.75rem side padding, 650 weight. Hover goes to quiet ink; press scales to 0.97 over 140ms.
- **Plate buttons (Play/Pause, Copy):** plate-control pills, at least 4.5rem by 2.75rem, small text at 600. Hover lightens to plate-control-hover; press scales to 0.96. Labels change in place: Play and Pause; Copy, then Copied (or "Select it" if the clipboard refuses) for 1.6 s.
- **Text links ("Guardrails, not a sandbox", "Full install guide"):** 600 weight, underlined 1px at 0.2em offset, 2.75rem tall.
- **Focus:** a 2px outline in ink, 3px off; on the plate the outline is plate ink.

### Navigation
- **Header:** the wordmark on the left; Docs, GitHub and the theme toggle on the right. Links are 550 weight with no underline until hover. The docs header shares the wordmark and holds still during view transitions.
- **Footer nav:** Docs, GitHub, "For agents: llms.txt", MIT license, on the plate.

### Stage (recording)
The site's signature unit: a recording on the plate with its calls as subtitles.
- **Frame:** 16:9 (8:9 on a phone with a crop). The AVIF still is the recording's last frame; the video loads after the page, fades in over 400ms, plays while 35% of it is in view, and stays still under reduced motion or Save-Data until Play is pressed. Paused, it keeps the paused frame.
- **Bar:** the prompt in quiet plate ink, clamped to two lines; the Play/Pause pill; the subtitle, three lines tall whatever the call (request on one, answer on two), `→` for a call and `←` for its answer, `$` and `#` for a command the user ran; the lease tag.
- **Lease tag:** reads `lease free`, `lease held` or `lease stopped`, following the calls. Free is plate-control with quiet text, held is plate ink with plate text, stopped is stop red with white text.
- **Caption:** the note on how the recording was made, then a disclosure, "All N calls", listing every call with its time (m:ss.s, tabular figures).
- **Subtitle change:** the new call rises 0.6em and fades in over 260ms (none under reduced motion).

### Stop Key
A red Esc keycap set inline in the poster, as tall as the capitals and standing on the baseline. It sinks 0.06em and loses most of its lip when the stop recording reaches the user's stop command, and over 120ms when pressed. Pressing it replays the recording from 2.5 s before the stop.

### Code Block
A command on the plate, 0.75rem corners, wrapped to the width rather than scrolled, with a Copy pill at its right.

### Docs pages
Starlight with theme.css: tokens, wordmark, Archivo titles at 72% and 84% width, links underlined at 1px and 2px on hover, code in table first columns kept on one line, code in headings without a chip. Code blocks are Expressive Code set in astro.config.mjs to match the home page's: on the plate in both themes with github-dark colours, 0.75rem corners, no border or window frame, lines wrapped to the width, and the first line wrapping short of the copy button.

### Motion
One easing for UI (`cubic-bezier(0.22, 1, 0.36, 1)`). The hero frame either drops into the first recording over 1100ms after 400ms (when the recording is already in the first view, as on an upright phone) or grows into it on the root scroll timeline (when it is below). Below the hero, poster lines, facts, recordings, clips, install steps and the footer wordmark rise 2.5rem and fade in on a view timeline (entry 5% to cover 30%). Under reduced motion none of this runs, scrolling is instant, and pages do not crossfade.

## Do's and Don'ts

### Do:
- **Do** set every surface from `tokens.css`: concrete ground, raised concrete, plate.
- **Do** pick the red step by surface: stop-red and stop-red-lip on the key, stop-text on the ground, stop-text-plate on the plate.
- **Do** put recordings, calls and commands on the plate, with the calls as text under the recording and the note saying how it was recorded.
- **Do** take headline widths from the ladder (62, 70, 72, 84, 88, 100%) and keep running text at 100%.
- **Do** keep every target at least 2.75rem tall and the focus outline visible on both the ground and the plate.
- **Do** give every motion a calm, complete reduced-motion state.
- **Do** keep the home page within its measured budget: Lighthouse mobile Performance 100, CLS 0, 3.3 KB JS gzipped.

### Don't:
- **Don't** add borders, divider rules, shadows or gradients to the home page; change the surface instead.
- **Don't** use red for anything but the stop key and stopped or refused answers.
- **Don't** set labels, headings or decoration in JetBrains Mono.
- **Don't** set Archivo narrower than 62% or use widths off the ladder; the cut face does not hold them.
- **Don't** show a recording without its calls as text, or a mock-up in place of a recording.
- **Don't** add an accent colour; links and the current page are ink.
