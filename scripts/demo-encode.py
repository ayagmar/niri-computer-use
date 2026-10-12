"""Turns a demo recording (`make nested-demo SCENE=<scene>`) into the README and site media:

    python3 -I scripts/demo-encode.py target/e2e/<run> --name errand --out site/public/demo \
        [--gif FROM TO] [--clip FROM TO] [--still AT]... [--og AT]

The run's `frames/` are JPEGs named by the Unix time in nanoseconds of their capture, and
its `timeline.jsonl` has each call with its start and end on the same clock. Under the
desktop goes a caption band rendered from the timeline: what the scene stands for, each
request as it went out and a summary of the server's answer when it came back, with the
round trip the harness measured and the lease's state. The one line not from the server is
the narration after a user command such as `stop`, marked `#`.

Writes `<name>.mp4` (H.264) and `<name>.webm` (AV1) of the whole run or of `--clip`,
`<name>.json` with the prompt and the calls shown, timed from the clip's start, and
optionally `<name>.gif` (960 px wide, 15 fps) of `--gif`, `.png` and `.avif`
stills of the frame at each `--still` (the second as `<name>-2`, and so on), and `og.png`
(1280x640, for link previews) from the frame at `--og`. Times are seconds into the video. Needs ffmpeg with libx264 and
libsvtav1, rsvg-convert and avifenc; every call has a deadline.
"""

import argparse
import html
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

# What the band says after the user's stop command. The command prints nothing, so this is
# narration, marked `#` and shown without a time.
USER_ANSWER = "the command the stop key runs; it takes the lease back"
BAND = 116
WIDTH = 1280
FPS = 30
MONO = "JetBrainsMono Nerd Font Mono"
AMBER = "#f2a33a"
RED = "#ff6b6b"
INK = "#e6e8ea"
MUTED = "#8b9399"
GROUND = "#121416"
RULE = "#2a2e32"


def run(argv, timeout):
    # SVT_LOG=1 keeps the AV1 encoder to its warnings and errors.
    env = {**os.environ, "SVT_LOG": "1"}
    subprocess.run(argv, check=True, timeout=timeout, stdout=subprocess.DEVNULL, env=env)


def load(run_dir):
    frames = sorted(run_dir.joinpath("frames").glob("*.jpg"), key=lambda p: int(p.stem))
    if len(frames) < 2:
        sys.exit(f"{run_dir}/frames has no recording")
    timeline = [json.loads(line) for line in run_dir.joinpath("timeline.jsonl").open()]
    return frames, timeline


def seconds(ns, origin):
    return (ns - origin) / 1e9


# What each answer says, in one line, from the result's own fields.
def answer(tool, record):
    result = record.get("result") or {}
    if record.get("is_error"):
        return f"refused: {result.get('error')}", True
    if tool == "acquire_desktop":
        return f"lease taken · users_window: {result.get('users_window')}", False
    if tool == "desktop_state":
        windows = " · ".join(f"{w['app_id']} {w['id']}" for w in result.get("windows", []))
        return f"focused_window: {result.get('focused_window')} · {windows}", False
    if tool == "elements":
        named = [
            f"{e['role']} “{e['name']}”"
            for e in result.get("elements", [])
            if e["role"] in ("text", "button", "entry") and e.get("name")
        ]
        shown = " · ".join(named[-2:][::-1])
        return f"{len(result.get('elements', []))} elements · {shown}", False
    if tool == "release_desktop":
        restored = result.get("restored") or {}
        if not restored:
            return f"released: {str(result.get('released')).lower()}", False
        return (
            f"released · focus back on {restored.get('focused_window')}"
            f" · observed: {restored.get('observed')}",
            False,
        )
    parts = []
    if "accepted" in result:
        parts.append(f"accepted: {str(result['accepted']).lower()}")
    if "observed" in result:
        parts.append(f"observed: {result['observed']}")
    focus = (result.get("changes") or {}).get("focus")
    if focus:
        parts.append(f"focus {focus.get('from')} → {focus.get('to')}")
    for window in (result.get("changes") or {}).get("windows", []):
        if "size" in window:
            parts.append(f"window {window['id']} size {window['size'][0]}×{window['size'][1]}")
    return " · ".join(parts) or "ok", False


def request(record):
    if "user" in record:
        program, args = record["user"]
        return f"{Path(program).name} {' '.join(args)}"
    args = json.dumps(record["arguments"], ensure_ascii=False, separators=(", ", ": "))
    return record["tool"] + ("" if args == "{}" else " " + args)


def calls(timeline, origin):
    """Each call or user command as the band shows it, with its start in seconds."""
    out = []
    for record in timeline:
        if "note" in record:
            continue
        you = "user" in record
        text, error = (USER_ANSWER, False) if you else answer(record["tool"], record)
        out.append({
            "start": seconds(record["start_ns"], origin),
            "request": request(record),
            "answer": text,
            "error": error,
            "you": you,
            "ms": None if you else round((record["end_ns"] - record["start_ns"]) / 1e6),
        })
    return out


def transcript(timeline, origin, span):
    """The prompt and the calls made within `span`, timed from its start."""
    start, stop = span
    prompt = next((r["note"] for r in timeline if "note" in r), "")
    shown = []
    for call in calls(timeline, origin):
        if start - 0.01 <= call["start"] < stop:
            shown.append({**call, "start": round(call["start"] - start, 2)})
    return {"prompt": prompt, "calls": shown}


def states(timeline, origin, end):
    """The band's states as (start second, state), one per change."""
    prompt = next((r["note"] for r in timeline if "note" in r), "")
    lease = "free"
    state = {"prompt": prompt, "lease": lease, "request": None, "answer": None, "you": False}
    out = [(0.0, dict(state))]
    for record in timeline:
        if "note" in record:
            continue
        start = seconds(record["start_ns"], origin)
        finish = seconds(record["end_ns"], origin)
        state.update(request=request(record), answer=None, you="user" in record)
        out.append((start, dict(state)))
        if "user" in record:
            text, error = USER_ANSWER, False
            lease = "stopped"
        else:
            text, error = answer(record["tool"], record)
            if record["tool"] == "acquire_desktop" and not error:
                lease = "held"
            if record["tool"] == "release_desktop":
                lease = "free"
        ms = round((record["end_ns"] - record["start_ns"]) / 1e6)
        state.update(answer=(text, error, ms), lease=lease)
        out.append((finish, dict(state)))
    return [(t, s) for t, s in out if t < end]


def clip(text, limit):
    return text if len(text) <= limit else text[: limit - 1] + "…"


def band_svg(state):
    e = html.escape
    rows = [
        f'<rect width="{WIDTH}" height="{BAND}" fill="{GROUND}"/>',
        f'<rect width="{WIDTH}" height="1" fill="{RULE}"/>',
        f'<text x="24" y="32" fill="{MUTED}">asked</text>',
        f'<text x="96" y="32" fill="{INK}">{e(clip(state["prompt"], 84))}</text>',
    ]
    rows += lease_tag(state["lease"])
    if state["request"]:
        arrow, colour = ("$", RED) if state["you"] else ("→", AMBER)
        rows.append(f'<text x="24" y="66" fill="{colour}">{arrow}</text>')
        tool, _, args = state["request"].partition(" ")
        rows.append(
            f'<text x="50" y="66"><tspan fill="{INK}" font-weight="700">{e(tool)}</tspan>'
            f'<tspan fill="{MUTED}"> {e(clip(args, 96 - len(tool)))}</tspan></text>'
        )
    if state["answer"]:
        text, error, ms = state["answer"]
        if state["you"]:
            rows.append(f'<text x="24" y="98" fill="{MUTED}">#</text>')
            rows.append(f'<text x="50" y="98" fill="{MUTED}">{e(clip(text, 98))}</text>')
        else:
            colour = RED if error else INK
            rows.append(f'<text x="24" y="98" fill="{RED if error else MUTED}">←</text>')
            rows.append(f'<text x="50" y="98" fill="{colour}">{e(clip(text, 98))}</text>')
            rows.append(
                f'<text x="{WIDTH - 24}" y="98" fill="{MUTED}" text-anchor="end">{ms} ms</text>'
            )
    body = "\n".join(rows)
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{WIDTH}" height="{BAND}" '
        f'font-family="{MONO}" font-size="17" xml:space="preserve">\n{body}\n</svg>\n'
    )


def lease_tag(lease):
    label, colour, dash, fill = {
        "free": ("lease free", MUTED, ' stroke-dasharray="4 3"', "none"),
        "held": ("lease held", AMBER, "", "#f2a33a1f"),
        "stopped": ("stopped", RED, "", "#ff6b6b1f"),
    }[lease]
    x = WIDTH - 24 - 132
    return [
        f'<rect x="{x}" y="12" width="132" height="28" rx="14" fill="{fill}" '
        f'stroke="{colour}" stroke-width="1.5"{dash}/>',
        f'<text x="{x + 66}" y="31" fill="{colour}" text-anchor="middle" font-size="15">{label}</text>',
    ]


def concat_list(path, entries):
    """An ffconcat file showing each (file, seconds) in turn."""
    lines = ["ffconcat version 1.0"]
    for file, duration in entries:
        lines += [f"file '{file}'", f"duration {duration:.6f}"]
    lines.append(f"file '{entries[-1][0]}'")
    path.write_text("\n".join(lines) + "\n")


def build(run_dir, work):
    frames, timeline = load(run_dir)
    origin = int(frames[0].stem)
    times = [seconds(int(f.stem), origin) for f in frames]
    end = times[-1]
    entries = [(f.resolve(), b - a) for f, a, b in zip(frames, times, times[1:])]
    concat_list(work / "frames.txt", entries)
    bands = []
    marks = states(timeline, origin, end) + [(end, None)]
    for index, ((start, state), (stop, _)) in enumerate(zip(marks, marks[1:])):
        svg = work / f"band-{index:03}.svg"
        svg.write_text(band_svg(state))
        png = svg.with_suffix(".png")
        run(["rsvg-convert", "-o", str(png), str(svg)], 30)
        bands.append((png, max(stop - start, 0.001)))
    concat_list(work / "bands.txt", bands)
    return end, timeline, origin


def composite(work, span):
    """The filter graph's input arguments and its stacked, trimmed output."""
    start, stop = span
    # Constant frame rate first, so a band shown since before `start` is kept.
    trim = f"fps={FPS},trim=start={start}:end={stop},setpts=PTS-STARTPTS"
    inputs = []
    for name in ("frames", "bands"):
        inputs += ["-f", "concat", "-safe", "0", "-i", str(work / f"{name}.txt")]
    graph = (
        f"[0:v]format=rgb24,{trim}[desk];[1:v]format=rgb24,{trim}[band];"
        "[desk][band]vstack=inputs=2[v]"
    )
    return inputs, graph


def videos(work, out, name, span):
    inputs, graph = composite(work, span)
    base = ["ffmpeg", "-y", "-v", "error", *inputs, "-filter_complex", graph, "-map", "[v]", "-an"]
    run(
        [*base, "-c:v", "libx264", "-crf", "23", "-preset", "slow", "-pix_fmt", "yuv420p",
         "-movflags", "+faststart", str(out / f"{name}.mp4")],
        600,
    )
    run(
        [*base, "-c:v", "libsvtav1", "-crf", "42", "-preset", "6", "-pix_fmt", "yuv420p",
         str(out / f"{name}.webm")],
        600,
    )


def gif(work, out, name, span):
    inputs, graph = composite(work, span)
    graph += (
        ";[v]fps=15,scale=960:-1:flags=lanczos,split[a][b];"
        "[a]palettegen=max_colors=128:stats_mode=diff[p];"
        "[b][p]paletteuse=dither=bayer:bayer_scale=4:diff_mode=rectangle[g]"
    )
    run(
        ["ffmpeg", "-y", "-v", "error", *inputs, "-filter_complex", graph, "-map", "[g]",
         "-loop", "0", str(out / f"{name}.gif")],
        600,
    )


def still(work, out, name, at):
    inputs, graph = composite(work, (at, at + 0.5))
    png = out / f"{name}.png"
    run(
        ["ffmpeg", "-y", "-v", "error", *inputs, "-filter_complex", graph, "-map", "[v]",
         "-frames:v", "1", str(png)],
        120,
    )
    run(["avifenc", "-q", "55", "-s", "4", str(png), str(out / f"{name}.avif")], 120)


def og(work, out, at):
    """A 1280x640 card: the desktop at `at` under the name and the one-line promise."""
    frame = work / "og-frame.png"
    inputs, graph = composite(work, (at, at + 0.5))
    run(
        ["ffmpeg", "-y", "-v", "error", *inputs, "-filter_complex", graph, "-map", "[v]",
         "-frames:v", "1", str(frame)],
        120,
    )
    svg = work / "og.svg"
    # The recording sits on the right, ringed in amber; the headline is the site's H1.
    shot_w, shot_h = 680, 680 * 836 // 1280
    shot_x, shot_y = 1280 - 32 - shot_w, (640 - shot_h) // 2
    headline = ["Your agent can", "drive your niri", "desktop."]
    lines = "".join(
        f'<text x="56" y="{230 + 54 * i}" fill="{INK}" font-size="42" font-weight="700">{line}</text>\n'
        for i, line in enumerate(headline)
    )
    svg.write_text(
        f'<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" '
        f'width="1280" height="640" font-family="{MONO}">\n'
        f'<rect width="1280" height="640" fill="{GROUND}"/>\n'
        f'<image x="{shot_x}" y="{shot_y}" width="{shot_w}" height="{shot_h}" '
        f'xlink:href="{frame.resolve()}"/>\n'
        f'<rect x="{shot_x - 4}" y="{shot_y - 4}" width="{shot_w + 8}" height="{shot_h + 8}" rx="8" '
        f'fill="none" stroke="{AMBER}" stroke-width="3"/>\n'
        f'<text x="56" y="150" fill="{MUTED}" font-size="22">niri-computer-use</text>\n'
        f"{lines}"
        f'<text x="56" y="420" fill="{AMBER}" font-size="28" font-weight="700">One key takes it back.</text>\n'
        f'<text x="56" y="500" fill="{MUTED}" font-size="20">An MCP server for niri 26.04</text>\n'
        "</svg>\n"
    )
    run(["rsvg-convert", "-o", str(out / "og.png"), str(svg)], 30)


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("run", type=Path)
    parser.add_argument("--name", required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--clip", nargs=2, type=float, metavar=("FROM", "TO"))
    parser.add_argument("--gif", nargs=2, type=float, metavar=("FROM", "TO"))
    parser.add_argument("--still", type=float, metavar="AT", action="append", default=[])
    parser.add_argument("--og", type=float, metavar="AT")
    options = parser.parse_args()
    for tool in ("ffmpeg", "rsvg-convert", "avifenc"):
        if shutil.which(tool) is None:
            sys.exit(f"{tool} is not on PATH")
    options.out.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory() as work:
        work = Path(work)
        end, timeline, origin = build(options.run, work)
        span = options.clip or (0, end)
        videos(work, options.out, options.name, span)
        record = transcript(timeline, origin, span)
        options.out.joinpath(f"{options.name}.json").write_text(json.dumps(record, indent=1) + "\n")
        if options.gif:
            gif(work, options.out, options.name, options.gif)
        for index, at in enumerate(options.still, 1):
            name = options.name if index == 1 else f"{options.name}-{index}"
            still(work, options.out, name, at)
        if options.og is not None:
            og(work, options.out, options.og)
    for path in sorted(options.out.glob(f"{options.name}.*")) + sorted(options.out.glob("og.png")):
        print(f"{path}  {path.stat().st_size // 1024} KiB")


if __name__ == "__main__":
    main()
