# Contributing

Run `make check` before you commit. Add a test with every behaviour change and a regression test with every bug fix. Don't loosen a lint, threshold or dependency rule to make a check pass.

Use Conventional Commits: `type(scope): summary`. Keep the subject lowercase, imperative, at most 72 characters, and without a trailing period.

New dependencies follow the version rule: use the newest stable release that is at least 7 days old, and record it in [docs/decisions.md](docs/decisions.md).

More detail:

- [AGENTS.md](AGENTS.md): the full set of code, commit, docs and version rules.
- [docs/development.md](docs/development.md): make targets and checks.
