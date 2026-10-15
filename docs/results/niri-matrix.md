# niri compatibility matrix

Written by `make niri-matrix`: this checkout's server, with `niri-ipc` 26.4.0, on each niri built by `scripts/niri-build.sh`, nested in the headless cage. `docs/development.md` says what each column checks.

| niri | Commit | Expected | `Version` reply | `compat` | Event stream | Observation | Without the lease | `acquire_desktop` | Suites | Result |
|---|---|---|---|---|---|---|---|---|---|---|
| v25.08 | 01be0e6 | Observe | – | – | – | – | – | – | – | not built |
| v25.11 | b35bcae | Observe | `25.11 (v25.11)` | `read_only` | `connected` | 5/5 answer | 14/14 `lease_required` | `read_only` | – | as expected |
| v26.04 | 8ed0da4 | Full | `26.04 (v26.04)` | `ok` | `connected` | 5/5 answer | 14/14 `lease_required` | `ok` | 5/5 pass | as expected |
| main | fc9e8c4 | Record | `26.04 (v26.04-193-gfc9e8c42)` | `ok` | `connected` | 5/5 answer | 14/14 `lease_required` | `ok` | 4/5 pass | recorded |

## What went otherwise

### v25.08

- build: The system library `libdisplay-info` required by crate `libdisplay-info-sys` was not found.

### main

- `--input`: supervisor: fail: expected a successful call; saw {"content":[{"text":"{\"detail\":\"typed 1 of 1000 characters; then the lock state was last read over 250 ms before the call's next part could go out, so it isn't known to be unlocked now\",\"error\":\"lock_stale\",\"may_have_sent\":true}","type":"text"}],"isError":true,"structuredContent":{"detail":"typed 1 of 1000 characters; then the lock state was last read over 250 ms before the call's next part could go out, so it isn't known to be unlocked now","error":"lock_stale","may_have_sent":true}}

## Runs

Each run's artifacts are in `target/e2e/<run>`.

- v25.11: 1792029348-2551485-1-0
- v26.04: 1792029348-2551485-2-0, 1792029348-2551485-2-1, 1792029348-2551485-2-2, 1792029348-2551485-2-3, 1792029348-2551485-2-4, 1792029348-2551485-2-5
- main: 1792029348-2551485-3-0, 1792029348-2551485-3-1, 1792029348-2551485-3-2, 1792029348-2551485-3-3, 1792029348-2551485-3-4, 1792029348-2551485-3-5
