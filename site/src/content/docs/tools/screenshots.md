---
title: Screenshots
description: The screenshot tool, screenshot refs for the pointer, settled screenshots and saving PNG files.
sidebar:
  order: 3
---

Prefer `desktop_state` when structured data answers the question; take a screenshot when you need pixels. A screenshot is read-only and needs no lease, but a screenshot taken under the lease also serves the [pointer tools](../acting/#pointer-tools).

## `screenshot`

| Argument | Value |
|---|---|
| `target` (required) | `focused_output`, `output:<name>` with a name from `outputs`, or `region` |
| `region` | with target `region`: `{x, y, width, height}` in layout coordinates, inside one output |
| `max_width` | the widest image to return, in pixels; default 1280 |
| `format` | `jpeg` (default, quality 80) or `png` |
| `save_path` | also save a full-resolution PNG at this path under `capture_dir`; see [below](#saving-a-screenshot) |

The capture uses the output's own scale, lowered when the captured width times that scale is wider than `max_width`. For small text, take a region around it.

While one of this server's actions is running, `screenshot` waits for it to end before capturing, so a screenshot sent alongside an action shows the screen after it.

The result is an image block, then the metadata as text and as `structuredContent`:

| Field | Value |
|---|---|
| `output`, `transform` | the captured output and its transform |
| `output_origin` | the output's top-left corner in layout coordinates |
| `captured` | the captured rectangle in layout coordinates |
| `scale` | image pixels per logical pixel |
| `width`, `height`, `mime_type` | the image, checked against its own header |
| `captured_at_unix_ms`, `capture_ms` | when the capture started and how long it took |
| `screenshot_ref` | an id such as `shot-4` for this capture while this server holds the lease, or null. The server keeps the last 64 of the current lease in memory and drops them all when the lease is taken or given up |
| `settled` | only on a screenshot that waited for the screen to stop changing (an action's `screenshot: true`, or `wait_for`'s `screen_stable`): true when the last two captures were the same image, false when the screen still changed at the limit |
| `saved` | with `save_path`: the saved file's `path`, `width` and `height` |

`grim` gets five seconds and at most 64 MiB of output.

## Screenshot refs

While the agent holds the lease, every screenshot gets a `screenshot_ref`. The pointer tools take it with pixel coordinates in that image, so the agent aims at what it saw, never at raw layout coordinates. A ref can't be used once it is over 60 seconds old, once its output has moved, resized or changed scale or transform, or after niri's event stream reconnects; the pointer tools then refuse with `ref_invalid`, and the agent takes a new screenshot.

## Settled screenshots

Every action, and `wait_for`, takes `screenshot: true`. The result then comes with a screenshot of the focused output taken once the screen stopped changing: the server looks 50 ms after the action, then captures every 100 ms until two captures in a row are the same image, for at most 1.5 seconds. `settled` says whether they were. Two matching images mean the screen was still for 100 ms, not that the app is ready.

This is faster and more reliable than a `screenshot` call sent after the action, and the screenshot is taken before the next action can start.

## Saving a screenshot

With `save_path` and a [`capture_dir`](../../concepts/configuration/#capture_dir) in the policy file, the server also writes the capture as a PNG at the output's full resolution, whatever `max_width` is. It is a separate capture, taken just before the image the tool returns.

```json
{"target": "focused_output", "save_path": "readme/editor.png"}
```

`save_path` is relative to `capture_dir`, made of plain names, and names a new `.png` file; its directories must exist. An existing file is never replaced: a second save to the same name is an argument mistake, `invalid arguments: <path> already exists; pick another save_path`, and nothing is written. Without `capture_dir`, the call fails with `save_not_enabled`.

