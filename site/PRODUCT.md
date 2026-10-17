# Product

<!-- impeccable:product-schema 1 -->

## Platform

web

## Users

People who already run the niri Wayland compositor and use a coding agent with MCP support (Claude Code, Codex, Pi or any other stdio MCP client), deciding whether to let that agent look at and act on their real desktop. Readers who don't use niri should learn early that it is niri-only, and leave.

A second audience is the agent itself: an agent installing the server for its user, or an agent operating the desktop, reads the site through `llms.txt` and the per-page Markdown.

## Product Purpose

niri-computer-use is an MCP server. It gives an agent niri's own view of the desktop (windows, workspaces, focus, outputs, screenshots that map back to the layout, accessible elements by name) and, while the agent holds a lease, lets it focus windows, launch preset apps, run niri actions, click, scroll, drag, type, paste and open Noctalia panels. Success: a niri user installs it, binds the stop key, and trusts it enough to let an agent run an errand on their desktop.

## Positioning

It talks to niri through niri's IPC and event stream, not pixels alone: every action reports `accepted` (niri took the request) and `observed` (what niri saw afterwards), and `changes` lists what the action changed. Elements are found by accessible name, screenshots carry a `screenshot_ref` that aims clicks, and the user holds a stop key the agent can't press. It acts on the real desktop, not a VM.

## Operating Context

The user's own niri session on Arch-style Linux, often with Noctalia as the shell. The agent runs in a terminal (Claude Code, Codex, Pi) or a desktop client and starts `niri-computer-use serve` over stdio. One shared engine per niri instance serves every client. Installed from a release, with its install script or by hand from the tarball (x86_64 and aarch64 Linux), or with cargo from crates.io or a clone.

## Capabilities and Constraints

- Up to 22 tools; five look and need no lease (`status`, `desktop_state`, `screenshot`, `elements`, `wait_for`). A session without Noctalia or an accessibility bus has 17.
- niri 26.04 only (the `niri-ipc` crate is pinned to it); Linux, Wayland; release tarballs for x86_64 and aarch64 (glibc 2.39 or newer), or a build with Rust 1.99.0.
- Pointer tools need one monitor at transform Normal.
- Accessibility (elements) varies by toolkit; GTK 4 works best.
- Guardrails, not a sandbox: the lease, stop key, lock gate, deny list, secret-field refusal and audit log limit mistakes; an agent with the lease can do what the user could with a keyboard and mouse.
- `unrestricted` (off by default) lets `niri_action` `Spawn` run any program.

## Brand Commitments

- Display name stays `niri-computer-use`, set as a typographic wordmark; no logo.
- Accent colour of its own: not niri's default focus-ring blue, not Catppuccin.
- Dark and light themes.
- Voice: short declarative sentences, measured numbers, no claims of magic, no hype words.
- No Show HN or launch framing on the site.

## Evidence on Hand

- Measured results in `docs/results/`: GTK 4 element clicks 100/100 (`m9.md`), a stop mid-drag ends the call `stopped` and releases the button (`m4.md`), Noctalia panels open and observed in 50–51 ms (`m5.md`), a killed server's held button released by the next server in 3–4 ms (`input-robustness.md`).
- Demo recordings from the user's own niri 26.04 desktop (one 2560×1440 output, Noctalia): real Claude Code sessions (Opus 5.5) with the installed server, nothing scripted. Thinking time between calls is shortened to 1.2 s and calls play in real time; every place a recording is shown must say so. Each recording is `site/public/demo/<name>.{png,json}`, the JSON holding the prompt, the calls with their times, the note to show and the recorded output, and `site/public/clips/<name>.mp4`, the desktop without the caption band.
- No testimonials, users, stars or adoption numbers exist; don't invent any.

## Product Principles

- Show the real call and its real result; never a mock-up of output.
- Put safety where the first objection is: next to the promise, not below the fold.
- Tell the wrong reader to leave early (niri-only, Linux only, one monitor for the pointer).
- Plain, specific prose; every claim backed by code or a measured result.

## Accessibility & Inclusion

Body text at 4.5:1 contrast or better, focus-ring UI at 3:1. Videos respect `prefers-reduced-motion` (no autoplay, poster plus play), carry an `aria-label` and a text transcript of the calls. Visible keyboard focus everywhere.
