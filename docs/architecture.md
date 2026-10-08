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
| `observe.rs` | Screenshots: picks the output, plans grim's arguments and the image size they must produce, and checks the result. |
| `clipboard.rs` | Reads the clipboard's text with `wl-paste`. |
| `runner.rs` | The only code that starts processes. |
| `image_header.rs` | Reads a PNG's or JPEG's size from its header. The harness includes the same file. |
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

## Cancellation

rmcp marks a request as cancelled when the client cancels it, but it keeps running the tool. Each tool therefore races its work against the request's cancellation and drops the work when cancellation wins. Dropping a niri request closes its connection; dropping a `desktop_state` call only stops that call's wait, and the shared event stream keeps running.

## Subprocesses

Every program the server runs goes through `runner::run`: no stdin, stdout and stderr collected, a deadline, and a process group of its own. Stderr is read to the end, keeping the first 16 KiB, so a verbose child never finds it closed. The call ends only when both pipes close, so a descendant that keeps one open holds it until the deadline. If the call times out or is cancelled before the child has been reaped, the runner kills the whole group, so anything the child started dies with it. Until the child is reaped its process ID can't be reused, so the kill can't reach another group. A child that exits normally is left alone, together with anything it left running. Stdout over the caller's limit is an error, and an error keeps the exit status and the kept stderr. After a kill, Tokio reaps the child in the background, so a zombie can briefly remain.

## Screenshots

`screenshot` reads niri's outputs, picks the target (a named output, the focused output, or the one output a region lies inside), and runs `grim -t jpeg -q 80` or `grim -t png` with an explicit `-s` scale and `-o <output>` or `-g "x,y wxh"`. Without `max_width` the limit is 1280 pixels. The scale is the output's own, lowered when the capture would be wider than `max_width`.

grim 1.5.0 sizes its image as `int width = logical width × scale`, which truncates (`render.c:145–146`). The server expects the same, and it nudges a lowered scale up by the smallest step until the truncated width is exactly `max_width`, because `max_width / width` can land just below it in floating point. A capture whose PNG or JPEG header disagrees with the expected size is an `upstream_error`. The metadata returns the output, its transform and layout origin, the captured rectangle in layout coordinates, the scale, the image size, and the capture time.

Downscaling happens in grim and costs time: on a 2560x1440 output, JPEG took about 14 ms at full size and about 100 ms at the default 1280 pixels.

## Version rule

`niri-ipc` is pinned to `=26.4.0`. `status` compares niri's version reply, for example `26.04 (8ed0da4)`, with that pin: equal major and minor is `ok`, a different patch is `patch_warning`, and anything else, including a version that doesn't parse, is `read_only`.

## Errors

A failed tool returns `isError: true` with `{"error": <name>, "detail": <text>}` as structured content. The names are a stable contract for agents. An error inside the readiness report, such as niri being unreachable, is reported in the report rather than failing the `status` call.
