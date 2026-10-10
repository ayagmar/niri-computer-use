---
title: CLI reference
description: The niri-computer-use subcommands, what each one prints and when it fails.
sidebar:
  order: 2
---

```text
niri-computer-use serve | status | stop | resume | recover | engine | guard <server-pid> | paste-keeper
```

Each takes exactly one subcommand and no options. Anything else prints that usage line to stderr and exits 1. Every subcommand works on the niri instance in `NIRI_SOCKET`, or, without it, the one running niri of yours the server [finds](../../start/clients/#session-variables). A niri bind passes `NIRI_SOCKET` on.

## `serve`

```sh
niri-computer-use serve
```

Speaks MCP over stdin and stdout until the client closes stdin. Your MCP client runs it; see [Client setup](../../start/clients/). Stdout carries only MCP messages. It reads its environment once at startup (see [Configuration](../../concepts/configuration/#environment)) and starts the crash guardian before anything else.

## `status`

```sh
niri-computer-use status
```

Prints the readiness report as JSON and exits 0, whatever the report says. It is the same report as the `status` tool, except that `niri.event_stream` is null, because the subcommand opens no event stream, and `lease.held_by_me` is false. The fields are described under [`status`](../../tools/looking/#status).

It asks niri for its version and outputs, logind for the lock state and, when `noctalia` is on `PATH`, Noctalia for its status. It changes nothing.

## `stop`

```sh
niri-computer-use stop
```

Sets the stop flag for this niri instance, prints nothing and exits 0. A running action ends with `stopped`, the lease holder gives the lease up, and `acquire_desktop` and every action refuse with `stopped` until `resume`. Bind it to a key: see [First session](../../start/first-session/#bind-the-stop-key).

It fails, exiting 1, when niri's socket or the runtime directory is neither set nor found, or the flag can't be written.

## `resume`

```sh
niri-computer-use resume
```

Clears the stop flag, prints nothing and exits 0, also when no flag was set. It doesn't clear the input-dirty marker; `recover` does.

## `recover`

```sh
niri-computer-use recover
```

Clears the input-dirty marker that blocks every action with `recovery_required`. It takes the lease while it works, so it fails while a server holds it:

```text
niri-computer-use: a server holds the lease: PID … (…); end that agent or its server first
```

Without a marker it prints `No input-dirty marker: nothing to recover.` and exits 0. With one, it prints the marker, then, depending on what the marker names:

- a running input program, such as `wtype`: ends it, if it is still that process
- held pointer buttons: sends their release from a fresh virtual pointer, or, if it can't, asks you to press and release each of them
- native keys: sends their release and zero modifiers from a fresh virtual keyboard, which also makes niri send the compositor's keymap again
- none of these: lists your running `wtype` processes, if any, and asks before ending them

Then it asks you to check by hand:

```text
Check that no input is held: press and release Shift, Ctrl, Alt and Super, click once in an empty area, and check the application the input went to. A physical key doesn't always release a key wtype pressed.
Is all input released? Type yes to confirm:
```

Only `yes` clears the marker, and it prints:

```text
Marker cleared. `niri-computer-use resume` clears the stop flag if it is set.
```

Any other answer exits 1 with `not confirmed; the marker stays`.

## Internal subcommands

You don't run these yourself.

- `engine` is the shared engine `serve` starts in [shared mode](../../concepts/configuration/#shared) when its niri instance has none. It serves every shared-mode client of the instance over `engine.sock` in the runtime directory, writes its errors to `engine.log` there, and exits two seconds after its last client is gone.
- `guard <server-pid>` is the crash guardian `serve` or the engine starts. It waits for that server to end, and if the server's marker names native keys or pointer buttons it still held, releases them at once.
- `paste-keeper` is the clipboard keeper `paste` starts. It holds the pasted text on the clipboard, then puts the saved clipboard back.

## Exit status and errors

Every subcommand exits 0 on success and 1 on failure, printing `niri-computer-use: <message>` to stderr.
