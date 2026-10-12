# README example

The source of the README's example: one graded run of the `errand` skill eval, in which a model chose every call. The README's recording is a different thing, a script replaying fixed calls.

| | |
|---|---|
| Command | `make nested-eval SCENARIO=errand SKILL=skills/niri-computer-use MODEL=sonnet` |
| Run | `1791814792-1684005`, 2026-10-12 |
| Server | debug build of `main` at `8faae79`; the branch had changed no server source |
| Model | `claude-sonnet-5-5`, through Claude Code 2.1.296 (`claude -p`) |
| Grading | 9 of 9 expectations passed |
| Calls and time | 8 desktop calls in 11 agent turns, 13.8 s |

Environment: niri 26.04 (8ed0da4) in the private headless cage, one 1280x720 `winit` output at scale 1, Noctalia running, no accessibility bus. The run reported C1's host snapshot unchanged, no entries from its processes in the host's journal, and no leftover processes.

The prompt:

> Open my notes app, then type exactly `Meeting moved to 3pm` into the wev window and press Enter once to send it, then close the notes window you opened, and leave me on the window I was on.

## Files

- [`readme-example/calls.jsonl`](readme-example/calls.jsonl): each desktop call the agent made, its arguments and the server's structured result, in order. Taken from the run's `transcript.jsonl` with the agent's own text, the skill's text and the screenshot image left out, the test directory written as `$TEST_DIR`, the home directory as `~`, and the host cgroup removed.
- [`readme-example/grading.json`](readme-example/grading.json): the grader's nine expectations with their evidence, as the run wrote them.
