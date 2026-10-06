# Decisions

Dependency and design decisions, newest last. Each entry says what was chosen, why, and what was considered instead.

Version rule: use the newest stable release that is at least 7 days old, and record its publish date here.

## 2026-10-06: Rust 1.99.0

- Pinned in `rust-toolchain.toml`, with `rust-version = "1.99"` in `Cargo.toml`.
- Released 2026-09-28. That's the newest stable release at least 7 days old.
- This is an application, so there's no reason to support older compilers. Both values move together, by hand, in one `build:` commit.

## 2026-10-06: `const fn main` in the skeleton

- Clippy 1.99 reports `missing_const_for_fn` for an empty `main`. Marking it `const fn` satisfies the lint without an `#[expect]`.
- It goes away once `main` does real work. Don't copy it as a pattern.

## 2026-10-06: CI actions

| Action | Pin | Latest release when chosen |
|---|---|---|
| `actions/checkout` | `v7` | v7.0.1, 2026-07-20 |
| `taiki-e/install-action` | `v2` | v2.87.26, 2026-10-06 |

- Actions are pinned to a major tag, so patch releases arrive without the 7-day delay. `install-action` publishes almost daily.
- Dependabot only proposes major-version bumps for these, with a 7-day cooldown.
- Pinning exact versions or commit SHAs would enforce the delay, at the cost of frequent update PRs. Revisit if that trade-off changes.

## 2026-10-06: CI tool versions

These match the versions installed locally.

| Tool | Version | Published |
|---|---|---|
| cargo-deny | 0.20.2 | 2026-07-09 |
| cargo-machete | 0.9.2 | 2026-04-15 |
| typos-cli | 1.50.3 | 2026-09-25 |
| shellcheck | 0.11.0 | 2025-08-04 |

- typos-cli 1.51.0 came out on 2026-10-06, so it's too new.
- Dependabot doesn't update these pins. Bump them by hand in `.github/workflows/ci.yml`, following the version rule.
