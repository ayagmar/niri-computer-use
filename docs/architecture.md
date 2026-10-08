# Architecture

`niri-desktop-mcp` is one binary. `serve` runs an MCP server over stdin and stdout, one process per agent session. `status` prints the readiness report and exits.

## Modules

| Module | Does |
|---|---|
| `main.rs` | Reads `NIRI_SOCKET` and `PATH` once, picks the subcommand, and starts the server on a single-threaded Tokio runtime. |
| `tools.rs` | The rmcp tool definitions. Each tool turns the call into one module call and the result into MCP content. |
| `niri.rs`, `niri/request.rs`, `niri/events.rs` | The only code that talks to niri: one connection per request, and one long-lived event stream. |
| `niri/version.rs` | The version rule (pure). |
| `status.rs` | Builds the readiness report shared by the tool and the subcommand. |
| `error.rs` | Tool failures with their stable names. |
| `cli.rs` | Terminal output for the subcommands. Nothing else may print, because stdout is the MCP transport. |

## niri requests

Each request opens a new connection to `NIRI_SOCKET`, writes one JSON line and reads one reply line, the way `niri msg` does. niri's replies carry no request id, so a reused connection could read a late reply as the answer to the next request. A fresh connection is dropped at its two-second deadline, so that can't happen.

A failed connect, or a connection that closes before replying, is `niri_unavailable`. niri's own error message, or a reply that doesn't parse, is `upstream_error`, with the message kept in `detail`. A request that passes the deadline is `deadline_exceeded`.

## Event stream

`serve` keeps one connection to niri open for its event stream. A task reads each event line, parses it as niri-ipc's `Event`, and applies it to niri-ipc's `EventStreamState`, the same reducer niri uses for its own replies. Tools read that state through a Tokio `watch` channel, so a snapshot comes from one moment and never waits on the reader.

On connect, niri sends its current state as a burst of events: workspaces, windows, keyboard layouts (if any), overview, config and screencasts. The state counts as initialized once the workspaces, windows and overview events have arrived. `desktop_state` waits up to two seconds for that, for example right after the server starts.

- If the connection ends, the state is dropped and the task reconnects after one second. Meanwhile `desktop_state` returns the last connection's error under the same names a request uses: `niri_unavailable` for a failed connect or a closed connection, `upstream_error` for a refusal or an unreadable reply from niri, and `deadline_exceeded` when niri doesn't answer within two seconds. `status` reports the stream as `disconnected`.
- If an event doesn't parse, the state is dropped and the task reconnects at once. A second unparsable event stops the stream until the server restarts: `status` reports `schema_incompatible`, and `desktop_state` returns `upstream_error` naming the event type. Ordinary disconnects don't count toward this.
- The `status` subcommand keeps no stream open, so it reports `event_stream` as null.
- The task stops, closing its connection, once the server drops its last handle on the stream.

## Version rule

`niri-ipc` is pinned to `=26.4.0`. `status` compares niri's version reply, for example `26.04 (8ed0da4)`, with that pin: equal major and minor is `ok`, a different patch is `patch_warning`, and anything else, including a version that doesn't parse, is `read_only`.

## Errors

A failed tool returns `isError: true` with `{"error": <name>, "detail": <text>}` as structured content. The names are a stable contract for agents. An error inside the readiness report, such as niri being unreachable, is reported in the report rather than failing the `status` call.
