---
version: 1
slug: "src-pages-index-astro"
primary_target: "src/pages/index.astro"
related_targets: ["src/lib/home.ts","src/styles/tokens.css","src/styles/man.css","src/scripts/less.ts"]
---

# Landing page (site home)

Scope: the standalone home page `src/pages/index.astro`, its words in `src/lib/home.ts` (which `pages/index.md.ts` also serves as Markdown), and the colours in `src/styles/tokens.css` that it shares with the Starlight docs. The docs carry the same manual through `src/styles/theme.css` and the `PageTitle` and `Footer` overrides. Visitor mode: Persuade.

Audience and job: a niri user with an MCP coding agent deciding whether to let it act on their desktop. Action: install (commands on the page). Proof: one real session's eleven calls, when each went out and its round trip, replayed at their real times, and its 223 KB recording, which plays in place; the SAFETY list; the last three releases from CHANGELOG.md. Constraints: reads in full without JavaScript; one script of about 2 KB gzipped adds the keys, search and the replay. Dark and light from the system, AA in both, about 280 words, nothing invented, every motion off under reduced motion.

## Direction contract

THESIS: The server's own man page, niri-computer-use(1), as a terminal shows it. Section headings in capitals, groff bold for names and underline for files, facts in short lines. Refuses the marketing hero, cards, gradients, fake prompts and blinking cursors.

OWN-WORLD: One centred flow. NAME is the name at display size in the same mono at 800, with section names hung in a gutter as man hangs them. EXAMPLE is a band across the window on the plate: the session's calls are a niri window in the amber focus ring, which opens as niri opens a window when the band scrolls in; the recording opens under it, 16 px away, when asked for. One monospace, JetBrains Mono. A warm black with parchment ink, or paper with brown-black ink, from `light-dark()`. One amber for links and the focus ring, as niri rings the focused column. Code on a darker or lighter plate. A running head and foot, and less's reverse-video line stuck to the bottom of the window.

STORY: The visitor reads the name and what it does, sees the install line and that it is niri 26.04 only, watches the EXAMPLE session's calls go out at their real times, learns in DESCRIPTION what the agent can do, reads what SAFETY enforces and that it is not a sandbox, copies the INSTALLATION steps and the stop key bind, then HISTORY, BUGS and SEE ALSO into the docs.

FIRST VIEWPORT: The running head, the name large with its tagline, SYNOPSIS, the one-line install with copy and "niri 26.04 on Linux only", with the pager line at the bottom and the top of the EXAMPLE band below.

FORM: seed: none. Built from mock A (`git show 9c6fba6:site/mocks/manpage.astro`), cut to about half its words; HISTORY from CHANGELOG.md and the clip's length and size from the MP4 at build time.

Signature interaction: less's keys (j, k, f, b, d, u, g, G, / with n and N, ? for help, q), with (END) and messages in the pager line; the calls replay at their real times when in view unless motion is reduced, the recording plays in place on request, and each call's time plays it from that call; pages slide sideways between the home page and the docs. Print drops the pager line and prints outside links' addresses.

FINISH: finish review, fixes, DESIGN.md and design.json from the built page.
