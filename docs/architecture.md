# Architecture

`niri-computer-use` is one binary. `serve` runs an MCP server over stdin and stdout, one process per agent session. `status` prints the readiness report and exits. `stop` and `resume` set and clear the stop flag, and `recover` clears the input-dirty marker.

## Modules

| Module | Does |
|---|---|
| `main.rs` | Reads `NIRI_SOCKET` and `PATH` once, picks the subcommand, and starts the server on a single-threaded Tokio runtime. |
| `tools.rs` | The rmcp tool definitions. Each tool turns the call into one module call and the result into MCP content. |
| `niri.rs`, `niri/request.rs`, `niri/events.rs` | The only code that talks to niri: one connection per request, and one long-lived event stream. |
| `niri/version.rs` | The version rule (pure). |
| `policy.rs` | The policy file and the lease decision (pure, apart from reading the file). |
| `status.rs` | Builds the readiness report shared by the tool and the subcommand. |
| `observe.rs` | Screenshots: picks the output, plans grim's arguments and the image size they must produce, and checks the result. |
| `clipboard.rs` | Reads the clipboard's text with `wl-paste`. |
| `noctalia.rs` | The only code that talks to Noctalia: its `status` over the IPC socket. |
| `control.rs` | The lock state: logind's `LockedHint`, then Noctalia. |
| `control/runtime.rs` | The per-instance runtime directory and its stop flag. |
| `control/stop.rs` | Watches the runtime directory for the stop flag. |
| `control/lease.rs` | The lease lock and the holder record. |
| `control/marker.rs` | Reads the input-dirty marker. |
| `control/recover.rs` | The `recover` subcommand. |
| `control/procs.rs` | Process state from `/proc`: start times and the user's `wtype` processes. |
| `control/desk.rs` | Whether this server holds the lease: takes it, gives it up, and lets the stop flag take it back. |
| `audit.rs` | The audit log. |
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

## Cancellation and the audit log

rmcp marks a request as cancelled when the client cancels it, but it keeps running the tool. Each tool therefore runs its work through one helper that races it against the request's cancellation and drops the work when cancellation wins. Dropping a niri request closes its connection; dropping a `desktop_state` call only stops that call's wait, and the shared event stream keeps running.

The same helper writes one JSON line per call to `$XDG_STATE_HOME/niri-computer-use/audit.jsonl`, or `~/.local/state/niri-computer-use/audit.jsonl`, creating the directory, and any missing parent such as `~/.local/state`, with mode `0700` and the file with mode `0600`. Each line has the start time, the session (the MCP client's name and the server's PID), the niri instance, the tool, its argument metadata, the outcome and the duration. The outcome is read only from the result's error name: a stable name, `invalid_arguments`, `cancelled`, `internal` when the tool couldn't build its result, or null on success. Screenshot arguments are logged because they hold only targets, sizes and formats; no line ever holds an image, the clipboard's text or a window title. `accepted` and `observed` are null until action tools exist. A failed write doesn't fail the call; `status` reports the last one. Calls whose arguments rmcp rejects before the tool runs are not logged.

## Subprocesses

Every program the server runs goes through `runner::run`: no stdin, stdout and stderr collected, a deadline, and a process group of its own. Stderr is read to the end, keeping the first 16 KiB, so a verbose child never finds it closed. The call ends only when both pipes close, so a descendant that keeps one open holds it until the deadline. If the call times out or is cancelled before the child has been reaped, the runner kills the whole group, so anything the child started dies with it. Until the child is reaped its process ID can't be reused, so the kill can't reach another group. A child that exits normally is left alone, together with anything it left running. Stdout over the caller's limit is an error, and an error keeps the exit status and the kept stderr. After a kill, Tokio reaps the child in the background, so a zombie can briefly remain.

## Screenshots

`screenshot` reads niri's outputs, picks the target (a named output, the focused output, or the one output a region lies inside), and runs `grim -t jpeg -q 80` or `grim -t png` with an explicit `-s` scale and `-o <output>` or `-g "x,y wxh"`. Without `max_width` the limit is 1280 image pixels. The scale is the output's own, lowered when the capture's logical width times that scale is wider than `max_width`.

grim 1.5.0 sizes its image as `int width = logical width × scale`, which truncates (`render.c:145–146`). The server expects the same, and it nudges a lowered scale up by the smallest step until the truncated width is exactly `max_width`, because `max_width / width` can land just below it in floating point. A capture whose PNG or JPEG header disagrees with the expected size is an `upstream_error`. The metadata returns the output, its transform and layout origin, the captured rectangle in layout coordinates, the scale, the image size, and the capture time.

Downscaling happens in grim and costs time: on a 2560x1440 output, JPEG took about 14 ms at full size and about 100 ms at the default 1280 pixels.

## Noctalia and the lock state

Noctalia counts as installed when `noctalia` is an executable on `PATH` at startup. Only then does the server list `shell_status`, and `status` reports the same answer for the whole session, so the two never disagree. It counts as running when its socket, `$XDG_RUNTIME_DIR/noctalia-$WAYLAND_DISPLAY.sock`, answers `status` with a JSON object within two seconds. The server writes the whole fixed payload `/\x1estatus`, shuts down its write half and reads to the end, as Noctalia's own client does. It never sends text from a tool's arguments. Anything other than a JSON object is `noctalia_unavailable`, except an `error:` reply, which keeps Noctalia's text as `upstream_error`. `status` checks again on every call, so a Noctalia restart shows up.

The lock state comes from logind's `LockedHint` and from Noctalia's `locked`, and locked wins. niri sets the hint on the session in its own `XDG_SESSION_ID`, and only when it runs as the session instance (`niri --session`), so the server asks logind about that session, not its own: it finds niri's PID from the peer credentials of a connection to `NIRI_SOCKET`, checks that `/proc/<pid>/cmdline` has `--session`, reads `XDG_SESSION_ID` from `/proc/<pid>/environ`, and runs `loginctl show-session <id> -p LockedHint --value`. A server started from SSH, a TTY or a client that passes a minimal environment therefore reads the same hint as one started in the session. If niri's session can't be found, niri has none, or niri runs without `--session`, logind doesn't answer and Noctalia decides; without either answer the state is `unknown`. `status` reports the session asked, the source and logind's error, if any. A session ID that isn't plain letters and digits is refused before `loginctl` runs, so it can't be read as an option, and so are `self` and `auto`, which would name the server's own session. A plain `niri`, such as a nested one started by hand from a terminal, inherits the host's `XDG_SESSION_ID` but never sets the hint, so logind would say `no` on a locked screen; the `--session` check makes that `unknown` instead, and Noctalia decides. The nested harness also unsets the variable.

## Runtime directory and the stop flag

Each niri instance has a runtime directory, `$XDG_RUNTIME_DIR/niri-computer-use/<instance>/`, where `<instance>` is the basename of `NIRI_SOCKET` without `.sock`, for example `niri.wayland-1.1487`. Servers for the same niri share it; a server for another niri, such as the nested harness, has its own. `niri-computer-use stop` creates the directory with mode `0700` and the empty file `stop` in it with mode `0600`; `status` reports `stop: true` while that file exists. `niri-computer-use resume` removes it, and refuses while `input-dirty` exists in the same directory. Both need `NIRI_SOCKET`, which niri sets for the commands it spawns, and `XDG_RUNTIME_DIR`, which comes from the session.

## The lease

One server at a time holds the lease on a niri instance. It is an exclusive, non-blocking `flock` on `<runtime dir>/lease`, a file created once with mode `0600` that the server never removes, so every server locks the same file. The kernel releases the lock when the holder's file is closed, which includes the process dying. The holder writes its PID, label (the MCP client's name and its own PID) and the time to `lease.json` for other servers' `status`; it empties the file before unlocking, and a record whose PID no longer exists names nobody.

`acquire_desktop` takes the lease only if the stop flag and the input-dirty marker are both absent. A runtime directory that can't be read counts as neither absent: the call fails rather than guess. A server watches its runtime directory with inotify from startup; whenever anything in it changes, it checks the stop flag again, and when the flag is set it gives the lease up. A directory that can't be read counts as stopped. If the watch can't be set up, `acquire_desktop` refuses, because a stop couldn't take the lease back.

Removing the runtime directory or the `lease` file while a server holds the lease would let another server lock a new file, and would leave the holder watching a directory nobody can reach. So the watcher checks, on every event and once a second, that the directory's path still names the inode it watches; when it doesn't, the watcher reports the flag as set and ends, the holder gives the lease up, and `acquire_desktop` refuses from then on with a detail that says to restart the server. Separately, the holder checks once a second that `lease` still names the file it locked, and gives the lease up if not. The once-a-second checks exist because the kernel delays a directory's own deletion event while a file inside it is open, as the held lease is. The lease's mutex is the action mutex: later action tools hold it while they run. `status` reads a copy of the holder kept beside the lease, so it never waits for a running action.

## The policy file and the lease decision

`$XDG_CONFIG_HOME/niri-computer-use/policy.toml`, or `~/.config/niri-computer-use/policy.toml`, is read once at startup. It lists `deny_input_app_ids` and `[[preset]]`s, each with a `name`, a fixed `argv` and an `app_id`; unknown keys are errors. A preset whose program runs any command it is given (a shell, an interpreter, `env`, `sudo`, `setsid`, `busybox`, `uwsm` and similar, matched by file name with any version suffix removed, or `flatpak` with `--command`) is refused, and so is a terminal with arguments, because terminals run their trailing arguments as a command. Two presets with one name, an empty name, `app_id` or `argv` are refused too. A missing file is valid; a file that can't be read or breaks a rule makes the policy `invalid`, and so does a missing `XDG_CONFIG_HOME` and `HOME`, because then nobody can tell whether a file exists.

`acquire_desktop` asks `policy::refuse_lease` after the stop flag and the input-dirty marker, using the same report `status` builds: niri's version error first, then `read_only` for an unsupported niri version, events this build can't parse, or an invalid policy, then `screen_locked` for a locked screen and for an unknown lock state, so input only ever goes to a screen known to be unlocked. Nothing uses the presets or the deny list yet; the action tools will.

## The input-dirty marker and `recover`

`<runtime dir>/input-dirty` says input may be stuck. It is one JSON object: the `operation`, the `phase` (`pending` before the input child starts, `running` once its PID is known), the writing server's PID, the time, the `child`'s PID and `/proc` start time, and any pointer `buttons` pressed. While the file exists, whatever it holds, `acquire_desktop` refuses with `recovery_required`, `resume` refuses, and `status` reports it as `input_dirty`; a file that can't be read or parsed blocks the same way. No tool writes it yet; the input tools will, before they send any input.

`niri-computer-use recover` is the only way to clear it, and only a human runs it:

1. It takes the lease, so no server can act meanwhile. If a server holds the lease, it refuses and names that server.
2. With a `child` in the marker, it checks that the PID still has the recorded start time, so a reused PID is never touched, then kills the child's process group, or the child alone if it doesn't lead one, and waits up to five seconds for it to exit. Without one, it lists the user's running `wtype` processes and ends them only if the human types `yes`, and only those that are still the processes it listed.
3. If the marker names pointer buttons, it asks the human to press and release each one, because this version has no virtual pointer to release them.
4. It prints the manual check (press and release Shift, Ctrl, Alt and Super, click once, check the application) and clears the marker only after the human types `yes`. Anything else, including end of input or a child that didn't exit, leaves the marker in place.

## Version rule

`niri-ipc` is pinned to `=26.4.0`. `status` compares niri's version reply, for example `26.04 (8ed0da4)`, with that pin: equal major and minor is `ok`, a different patch is `patch_warning`, and anything else, including a version that doesn't parse, is `read_only`.

## Errors

A failed tool returns `isError: true` with `{"error": <name>, "detail": <text>}` as structured content. The names are a stable contract for agents. A mistake in the arguments is different: rmcp answers arguments that don't fit the schema with `isError` and a plain-text message, and the server answers arguments that don't fit the desktop, such as an unknown output, the same way. An error inside the readiness report, such as niri being unreachable, is reported in the report rather than failing the `status` call.
