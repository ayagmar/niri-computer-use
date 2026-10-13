"""Turns a real-desktop demo run into one `scripts/demo-encode.py` takes:

    python3 -I scripts/demo-timeline.py <run> <claude-code-transcript.jsonl> \
        [--gap S] [--tail S] [--exclude-session LABEL] [--prompt TEXT]

`<run>/frames/` comes from `scripts/demo-record.sh`. The agent's calls come from its Claude
Code transcript, which has each call's arguments and full result, and are timed by the
server's audit log, whose `ts` is when a call started and `duration_ms` how long it took.
`<run>/user.jsonl`, when present, holds what the user did meanwhile, one JSON object a line:
`{"user": [program, [args...]], "start_ns": …, "end_ns": …}` with an optional `narration`.

Writes `<run>/cut/`: `timeline.jsonl` and `frames/` as links to the frames kept. Where no call
is in flight for longer than `--gap` seconds, the agent is thinking: that stretch is cut down
to `--gap`, and the end is held for `--tail`. Calls play in real time.
"""

import argparse
import datetime
import json
import os
from pathlib import Path

PREFIX = "mcp__niri-computer-use__"


def ns(stamp):
    moment = datetime.datetime.fromisoformat(stamp.replace("Z", "+00:00"))
    return int(moment.timestamp() * 1e9)


def audit_path():
    state = os.environ.get("XDG_STATE_HOME") or Path.home() / ".local/state"
    return Path(state) / "niri-computer-use/audit.jsonl"


def prompt_of(entry):
    content = (entry.get("message") or {}).get("content")
    if isinstance(content, str):
        return content
    texts = [b["text"] for b in content or [] if b.get("type") == "text"]
    return texts[0] if texts else None


def result_of(block):
    text = block.get("content")
    if isinstance(text, list):
        text = next((b.get("text") for b in text if b.get("type") == "text"), "")
    try:
        result = json.loads(text or "{}")
    except json.JSONDecodeError:
        return {"error": (text or "")[:200]}
    return result if isinstance(result, dict) else {}


def transcript_calls(path):
    """The first prompt and the server's calls, in order, timed by the transcript."""
    uses, results, prompt = [], {}, None
    for line in Path(path).open():
        entry = json.loads(line)
        if entry.get("type") == "user" and prompt is None:
            prompt = prompt_of(entry)
        content = (entry.get("message") or {}).get("content")
        for block in content if isinstance(content, list) else []:
            if block.get("type") == "tool_use" and block["name"].startswith(PREFIX):
                uses.append((block, ns(entry["timestamp"])))
            if block.get("type") == "tool_result":
                results[block["tool_use_id"]] = (block, ns(entry["timestamp"]))
    calls = []
    for use, start in uses:
        block, end = results.get(use["id"], ({}, start))
        calls.append({
            "tool": use["name"].removeprefix(PREFIX),
            "arguments": use.get("input") or {},
            "result": result_of(block),
            "is_error": bool(block.get("is_error")),
            "start_ns": start,
            "end_ns": end,
        })
    return prompt or "", calls


def timed_by_audit(calls, first, last, exclude):
    """Each call's start and end from the audit log, matched in order by tool name."""
    audit = []
    for line in audit_path().open():
        record = json.loads(line)
        start = ns(record["ts"])
        if first <= start <= last and record["session"] != exclude:
            audit.append((record["tool"], start, start + record["duration_ms"] * 1_000_000))
    index = 0
    for call in calls:
        for j in range(index, len(audit)):
            if audit[j][0] == call["tool"]:
                call["start_ns"], call["end_ns"] = audit[j][1], audit[j][2]
                index = j + 1
                break
    return calls


def cuts_for(records, first, last, gap, tail):
    """The stretches to remove: each idle one longer than `gap` down to `gap`, the end to `tail`."""
    busy = []
    for r in records:
        if busy and r["start_ns"] <= busy[-1][1]:
            busy[-1][1] = max(busy[-1][1], r["end_ns"])
        else:
            busy.append([r["start_ns"], r["end_ns"]])
    edges = [first] + [t for span in busy for t in span] + [last]
    idle = list(zip(edges[0::2], edges[1::2]))
    cuts = []
    for a, b in idle[:-1]:
        if b - a > gap:
            cuts.append((a + gap // 2, b - gap // 2))
    a, b = idle[-1]
    if b - a > tail:
        cuts.append((a + tail, b))
    return cuts


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("run", type=Path)
    parser.add_argument("transcript", type=Path)
    parser.add_argument("--gap", type=float, default=1.2)
    parser.add_argument("--tail", type=float, default=2.5)
    parser.add_argument("--exclude-session", default="", metavar="LABEL",
                        help="an audit session to leave out, such as the one that set the scene up")
    parser.add_argument("--prompt", help="the band's prompt instead of the transcript's first")
    options = parser.parse_args()
    frames = sorted(options.run.joinpath("frames").glob("*.jpg"), key=lambda p: int(p.stem))
    first, last = int(frames[0].stem), int(frames[-1].stem)
    prompt, calls = transcript_calls(options.transcript)
    calls = timed_by_audit(calls, first, last, options.exclude_session)
    records = [c for c in calls if first <= c["start_ns"] <= last]
    user = options.run / "user.jsonl"
    if user.exists():
        records += [json.loads(line) for line in user.open()]
    records.sort(key=lambda r: r["start_ns"])
    cuts = cuts_for(records, first, last, int(options.gap * 1e9), int(options.tail * 1e9))

    def shifted(t):
        return t - sum(min(t, to) - start for start, to in cuts if t > start)

    out = options.run / "cut"
    (out / "frames").mkdir(parents=True, exist_ok=True)
    for old in (out / "frames").iterdir():
        old.unlink()
    for frame in frames:
        t = int(frame.stem)
        if not any(start < t <= to for start, to in cuts):
            os.symlink(frame.resolve(), out / "frames" / f"{shifted(t)}.jpg")
    with (out / "timeline.jsonl").open("w") as timeline:
        timeline.write(json.dumps({"note": options.prompt or prompt}) + "\n")
        for r in records:
            moved = {**r, "start_ns": shifted(r["start_ns"]), "end_ns": shifted(r["end_ns"])}
            timeline.write(json.dumps(moved) + "\n")
    removed = sum(to - start for start, to in cuts) / 1e9
    print(f"{len(records)} records, {len(cuts)} cuts, {removed:.1f} s removed, "
          f"{(last - first) / 1e9 - removed:.1f} s left")


if __name__ == "__main__":
    main()
