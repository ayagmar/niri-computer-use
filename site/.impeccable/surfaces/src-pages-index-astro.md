---
version: 1
slug: "src-pages-index-astro"
primary_target: "src/pages/index.astro"
related_targets: ["src/components/home","src/styles/theme.css"]
---

# Landing page (site home)

Scope: the standalone home page `src/pages/index.astro` and its components in `src/components/home/`; the Starlight docs pages share its type, colour and header through `src/styles/theme.css`. Visitor mode: Persuade.

Audience and job: a niri user with an MCP coding agent deciding whether to let it act on their desktop. Action: install from source (the commands are on the page). Proof: the real-desktop recordings, with their calls as text. Constraints: under about 200 words of prose; Lighthouse mobile Performance ≥ 98, the rest 100, LCP ≤ 1.8 s, CLS ≤ 0.01, TBT ≤ 50 ms; home JS ≤ 15 KB gzipped; reduced motion is calm and complete; dark and light.

## Direction contract

THESIS: The recording lives inside the sentence. A poster-scale line, "Your agent [live recording] drives niri.", carries a frame where a word would be, and scrolling opens that frame into the full recording. Refuses the centred headline over a framed product shot, and the previous niri-screen-at-page-scale costume.

OWN-WORLD: Cool concrete ground (#E6E8E3 light, #161917 dark), near-black ink, recordings on a black plate with their calls as subtitles under them. Archivo at 62–100% width: condensed heavy posters, plain text at 100%. JetBrains Mono only for calls and commands. One red, for the stop key and a stopped answer. No rules, outlines, shadows or gradients; pill buttons in ink.

STORY: The visitor reads what it is in one line, watches the agent work as the frame opens, reads "One key takes it back" while the stop recording shows the refusal, sees three more short clips, copies three commands.

FIRST VIEWPORT: The wordmark, Docs, GitHub and theme at the top. Then the poster on two lines at about 15vw, the live frame inline after "Your agent". Below it, a one-sentence description and the Install button. Phone: the same, the frame a small portrait crop, the hero filling the screen.

FORM: sentence-as-layout (taste-skill draw), chosen over keycaps (B) and the signal box (C, the impeccable roll's assigned index 5; declined because the metaphor needs explaining). Kept from B: the red Esc keycap that sinks when the stop lands. Kept from C: the lease state shown as a lit tag. seed: 0111b2ce

Signature interaction: the frame in the headline grows into the stage on a scroll timeline; subtitles, the lease tag and the stop keycap follow each recording's real call times.

FINISH: unreviewed and undocumented is unfinished; this build ends with the finish review, the verdict, DESIGN.md, and every shipping raster carrying its provenance
