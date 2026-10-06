# AGENTS.md

1. Run `make check` before declaring any change done. A red gate means the change is not done.
2. Never add `#[allow]`. Use `#[expect(lint, reason = "…")]` only when the lint is wrong for that one item, and explain why in the reason.
3. Do not loosen a lint, threshold, or `deny.toml` rule to make a change pass. Changing the gates is its own change, with a stated reason.
4. Keep module boundaries clear: `tools.rs` only translates MCP calls to module calls, policy decisions live in `policy.rs`, and every subprocess goes through the runner. Only `niri/` talks to niri, and only `noctalia.rs` talks to Noctalia.
5. Give every external call—subprocess, socket, or wait—a deadline. Preserve upstream error details such as exit codes, stderr, and niri or Noctalia messages.
6. Do not add a dependency without an entry in `docs/decisions.md` that says why it is needed, which version was chosen, and why.
7. Add a test for every behavior change and a regression test for every bug fix. Keep pure modules free of I/O so they remain unit-testable.
8. Keep functions small and flat. Clippy enforces 80 lines, 5 arguments, nesting of 4, and cognitive complexity of 15. Split by responsibility rather than to avoid a limit.
9. Do not leave dead code, commented-out code, or stale TODOs. `todo!` and `unimplemented!` are denied.
10. Prefer types over comments. Use newtypes for IDs and coordinate spaces such as `ImagePx`, `LayoutPt`, `OutputLocalPt`, and `ProtocolPt` so they cannot be mixed accidentally.
11. Make internal items `pub(crate)`.
12. Commits use Conventional Commits: `type(scope): summary`, imperative, lowercase, at most 72 characters, no trailing period. One logical change per commit. Add a body only when the reason isn't obvious, in one or two plain sentences. No AI attribution, `Co-authored-by` lines, emoji or filler words.
13. Docs describe what exists now. Don't write stubs, placeholder pages or promises. Update docs in the same commit as the behaviour they describe, and only include commands you have run. Keep user-facing prose plain and specific.
14. Use the newest stable release that is at least 7 days old for every dependency, tool, action and toolchain. Check the publish date and record it in `docs/decisions.md`. A newer release is allowed only for a security fix, noted in the commit body. The exception is `niri-ipc`, which is pinned to the installed niri version (`=26.4.0` for niri 26.04).
