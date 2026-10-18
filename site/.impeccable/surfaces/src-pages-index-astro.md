---
version: 1
slug: "src-pages-index-astro"
primary_target: "src/pages/index.astro"
related_targets: ["src/lib/home.ts","src/styles/tokens.css","src/styles/man.css","src/scripts/less.ts"]
---

# Landing page (site home)

Scope: the standalone home page `src/pages/index.astro`, its words in `src/lib/home.ts` (which `pages/index.md.ts` also serves as Markdown), and the colours in `src/styles/tokens.css` that it shares with the Starlight docs. The docs carry the same manual through `src/styles/theme.css` and the `PageTitle` and `Footer` overrides. Visitor mode: Persuade.

Audience and job: a niri user with an MCP coding agent deciding whether to let it act on their desktop. Action: install (commands on the page). Proof: one real session's eleven calls, when each went out and its round trip, played at their real times on a drawing of the desktop in the agent's own screenshot pixels, and its 223 KB recording, which plays in the same frame; the SAFETY list; the last three releases from CHANGELOG.md. Constraints: reads in full without JavaScript; one script of about 2 KB gzipped adds the keys and search, and a second script plays the drawing. Dark and light from the system, AA in both, about 280 words, nothing invented, every motion off under reduced motion.

## Direction contract

THESIS: The server's own man page, niri-computer-use(1), as a terminal shows it. Section headings in capitals, groff bold for names and underline for files, facts in short lines. Refuses the marketing hero, cards, gradients, fake prompts and blinking cursors.

OWN-WORLD: The name at display size in the same mono at 800, then the errand as a technical line drawing of a niri desktop in the agent's 1280 by 720 screenshot pixels, with rulers: the terminal with the real calls, Files where they land, the amber focus ring moving between them, and the pointer as an amber dot that shows its coordinates. Section names hang in a gutter as man hangs them. One monospace, JetBrains Mono. A warm black with parchment ink, or paper with brown-black ink, from `light-dark()`. One amber for links and the focus ring, as niri rings the focused column. Code on a darker or lighter plate. A running head and foot, and less's reverse-video line stuck to the bottom of the window.

STORY: The visitor reads the name and what it does, sees the install line and that it is niri 26.04 only, watches the errand play: each call goes out at its real time and the drawing shows what it did, learns in DESCRIPTION what the agent can do, reads what SAFETY enforces and that it is not a sandbox, copies the INSTALLATION steps and the stop key bind, then HISTORY, BUGS and SEE ALSO into the docs.

FIRST VIEWPORT: The running head, the name large with its tagline, SYNOPSIS, the one-line install with copy and "niri 26.04 on Linux only", with the pager line at the bottom and the drawn errand starting to play below.

FORM: seed: none. Built from mock A (`git show 9c6fba6:site/mocks/manpage.astro`), cut to about half its words; HISTORY from CHANGELOG.md and the clip's length and size from the MP4 at build time.

Signature interaction: less's keys (j, k, f, b, d, u, g, G, / with n and N, ? for help, q), with (END) and messages in the pager line; the drawn errand plays its calls at their real times when in view unless motion is reduced, and the recording plays in its frame on request; pages slide sideways between the home page and the docs. Print drops the pager line and prints outside links' addresses.

FINISH: finish review, fixes, DESIGN.md and design.json from the built page.
