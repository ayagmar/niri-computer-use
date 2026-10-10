---
title: For agents
description: Plain-Markdown versions of this site, llms.txt, and the agent skill at stable URLs.
sidebar:
  order: 4
---

Everything on this site is also available as plain Markdown, for agents and other tools that read text rather than HTML.

## llms.txt

- [`llms.txt`](../../llms.txt) lists every page with a one-line description and a link to its Markdown, following the [llms.txt](https://llmstxt.org/) format.
- [`llms-full.txt`](../../llms-full.txt) is every page's Markdown in one file, in the order of the sidebar.

## Markdown for every page

Every page has a Markdown version at its address with `.md` in place of the trailing slash. This page, for example, is at [`reference/agents.md`](../agents.md), and the home page at [`index.md`](../../index.md). Links in the Markdown are absolute, so they work outside the site.

## The skill

The agent skill from the repository's [`skills/niri-computer-use`](https://github.com/ayagmar/niri-computer-use/tree/main/skills/niri-computer-use) directory is published here, unchanged, at stable paths:

| File | Address |
|---|---|
| `SKILL.md` | [`skill/SKILL.md`](../../skill/SKILL.md) |
| `references/tools.md` | [`skill/references/tools.md`](../../skill/references/tools.md) |
| `references/errors.md` | [`skill/references/errors.md`](../../skill/references/errors.md) |

The skill's own links are relative, so keep the three files in the same layout if you copy them. To install it from a clone instead, see [Client setup](../../start/clients/#the-skill).
