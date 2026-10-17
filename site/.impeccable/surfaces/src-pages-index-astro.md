---
version: 1
slug: "src-pages-index-astro"
primary_target: "src/pages/index.astro"
related_targets: ["src/lib/home.ts","src/styles/tokens.css"]
---

# Landing page (site home)

Scope: the standalone home page `src/pages/index.astro`, its words in `src/lib/home.ts` (which `pages/index.md.ts` also serves as Markdown), and the colours in `src/styles/tokens.css` that it shares with the Starlight docs. The docs carry the same manual through `src/styles/theme.css` and the `PageTitle` and `Footer` overrides. Visitor mode: Persuade.

Audience and job: a niri user with an MCP coding agent deciding whether to let it act on their desktop. Action: install (commands on the page). Proof: one real recording, as a still that links to a 223 KB clip, with six of its calls and their round trips; the SAFETY list; the last three releases from CHANGELOG.md. Constraints: no JavaScript, dark and light from the system, AA in both, about 280 words, nothing invented.

## Direction contract

THESIS: The server's own man page, niri-computer-use(1), as a terminal shows it. Section headings in capitals, groff bold for names and underline for files, facts in short lines. Refuses the marketing hero, cards, gradients, fake prompts and blinking cursors.

OWN-WORLD: One monospace, JetBrains Mono. A warm black with parchment ink, or paper with brown-black ink, from `light-dark()`. One amber for links and the focus ring, as niri rings the focused column. Code on a darker or lighter plate. A running head and foot, and less's reverse-video line stuck to the bottom of the window.

STORY: The visitor reads the NAME line and the SYNOPSIS, learns in DESCRIPTION what the agent can do and that it is niri 26.04 only, copies the INSTALLATION steps and the stop key bind, reads what SAFETY enforces and that it is not a sandbox, sees the EXAMPLE session, then HISTORY, BUGS and SEE ALSO into the docs.

FIRST VIEWPORT: The running head, NAME, SYNOPSIS, DESCRIPTION and the start of INSTALLATION, with the pager line at the bottom.

FORM: seed: none. Built from mock A (`git show 9c6fba6:site/mocks/manpage.astro`), cut to about half its words; HISTORY from CHANGELOG.md and the clip's length and size from the MP4 at build time.

Signature interaction: the pager line links to install, safety, docs and source, and where the browser ties animation to scrolling it shows how far down the page you are, as less does. Print drops the pager line and prints outside links' addresses.

FINISH: finish review, fixes, DESIGN.md and design.json from the built page.
