# niri compatibility matrix

Written by `make niri-matrix`: this checkout's server, with `niri-ipc` 26.4.0, on each niri built by `scripts/niri-build.sh`, nested in the headless cage. `docs/development.md` says what each column checks.

| niri | Commit | Expected | `Version` reply | `compat` | Event stream | Observation | Without the lease | `acquire_desktop` | Suites | Result |
|---|---|---|---|---|---|---|---|---|---|---|
| v25.08 | 01be0e6 | Observe | – | – | – | – | – | – | – | not built |
| v25.11 | b35bcae | Observe | `25.11 (v25.11)` | `read_only` | `connected` | 5/5 answer | 14/14 `lease_required` | `read_only` | – | as expected |
| v26.04 | 8ed0da4 | Full | `26.04 (v26.04)` | `ok` | `connected` | 5/5 answer | 14/14 `lease_required` | `ok` | 5/5 pass | as expected |
| main | fc9e8c4 | Record | `26.04 (v26.04-193-gfc9e8c42)` | `ok` | `schema_incompatible` | 4/5 answer | 14/14 `lease_required` | `read_only` | 1/5 pass | recorded |

## What went otherwise

### v25.08

- build: The system library `libdisplay-info` required by crate `libdisplay-info-sys` was not found.

### main

- probe: wait_for: expected ok, got upstream_error (niri sent an event this build can't parse (CastsChanged); the event stream stays stopped until the server restarts)
- probe: acquire_desktop: expected ok, got read_only (niri sent events this build can't parse)
- `--actions`: supervisor: fail: expected a successful call; saw {"content":[{"text":"{\"detail\":\"niri sent events this build can't parse\",\"error\":\"read_only\"}","type":"text"}],"isError":true,"structuredContent":{"detail":"niri sent events this build can't parse","error":"read_only"}}
- `--input`: supervisor: fail: expected a successful call; saw {"content":[{"text":"{\"detail\":\"niri sent events this build can't parse\",\"error\":\"read_only\"}","type":"text"}],"isError":true,"structuredContent":{"detail":"niri sent events this build can't parse","error":"read_only"}}
- `--shell`: supervisor: fail: expected a successful call; saw {"content":[{"text":"{\"detail\":\"niri sent events this build can't parse\",\"error\":\"read_only\"}","type":"text"}],"isError":true,"structuredContent":{"detail":"niri sent events this build can't parse","error":"read_only"}}
- `--a11y`: supervisor: fail: expected a successful call; saw {"content":[{"text":"{\"detail\":\"niri sent events this build can't parse\",\"error\":\"read_only\"}","type":"text"}],"isError":true,"structuredContent":{"detail":"niri sent events this build can't parse","error":"read_only"}}

## Runs

Each run's artifacts are in `target/e2e/<run>`.

- v25.11: 1792027381-1514413-1-0
- v26.04: 1792027381-1514413-2-0, 1792027381-1514413-2-1, 1792027381-1514413-2-2, 1792027381-1514413-2-3, 1792027381-1514413-2-4, 1792027381-1514413-2-5
- main: 1792027381-1514413-3-0, 1792027381-1514413-3-1, 1792027381-1514413-3-2, 1792027381-1514413-3-3, 1792027381-1514413-3-4, 1792027381-1514413-3-5
