"""M4's supervised real-session accuracy run (docs/results/m4.md). Run it from a shell in
the niri session, with the person at the machine and their hands off the mouse:

    python3 -I scripts/real-pointer-check.py target/debug/niri-computer-use /tmp/ncu-m4-real-<n>

It opens wev on the focused workspace, floats it at 400x300 with niri IPC, and moves the
real pointer with the server's pointer_move to five points on wev's surface, from a
screenshot of DP-1 at its own scale and one lowered to 1280 pixels wide. wev's log says
where each motion landed. Pointer motion only: no clicks, keys or scrolls."""
import json, math, os, subprocess, sys, time

SERVER = sys.argv[1]
OUT = sys.argv[2]
os.makedirs(OUT, exist_ok=True)
log_path = os.path.join(OUT, "wev.log")
report = open(os.path.join(OUT, "run.log"), "w")

def say(line):
    print(line); report.write(line + "\n"); report.flush()

def niri(*args):
    return json.loads(subprocess.run(["niri", "msg", "--json", *args], check=True, capture_output=True, text=True, timeout=5).stdout)

def action(*args):
    subprocess.run(["niri", "msg", "action", *args], check=True, capture_output=True, timeout=5)

wev = subprocess.Popen(["timeout", "120", "stdbuf", "-oL", "wev"], stdout=open(log_path, "w"), stderr=subprocess.STDOUT)
server = None
try:
    win = None
    for _ in range(50):
        win = next((w for w in niri("windows") if w["app_id"] == "wev" and w["pid"] is not None), None)
        if win: break
        time.sleep(0.1)
    assert win, "wev didn't map"
    wid = win["id"]
    if not win["is_floating"]:
        action("toggle-window-floating", "--id", str(wid))
    action("set-window-width", "--id", str(wid), "400")
    action("set-window-height", "--id", str(wid), "300")
    action("move-floating-window", "--id", str(wid), "-x", "200", "-y", "200")
    action("focus-window", "--id", str(wid))
    time.sleep(0.5)
    win = next(w for w in niri("windows") if w["id"] == wid)
    lay = win["layout"]
    out = niri("outputs")["DP-1"]["logical"]
    sx = out["x"] + lay["tile_pos_in_workspace_view"][0] + lay["window_offset_in_tile"][0]
    sy = out["y"] + lay["tile_pos_in_workspace_view"][1] + lay["window_offset_in_tile"][1]
    w, h = lay["window_size"]
    say(f"wev window {wid}: surface at ({sx}, {sy}), size {w}x{h}")

    server = subprocess.Popen([SERVER, "serve"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=open(os.path.join(OUT, "server.err"), "w"), text=True)
    ids = iter(range(1, 1000))
    def call(method, params):
        i = next(ids)
        server.stdin.write(json.dumps({"jsonrpc": "2.0", "id": i, "method": method, "params": params}) + "\n"); server.stdin.flush()
        while True:
            msg = json.loads(server.stdout.readline())
            if msg.get("id") == i: return msg["result"]
    call("initialize", {"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "m4-real-run", "version": "1"}})
    server.stdin.write(json.dumps({"jsonrpc": "2.0", "method": "notifications/initialized"}) + "\n"); server.stdin.flush()
    tool = lambda name, args: call("tools/call", {"name": name, "arguments": args})
    acq = tool("acquire_desktop", {})
    assert not acq.get("isError"), acq
    worst = 0.0
    for max_width in (4000, 1280):
        meta = tool("screenshot", {"target": "output:DP-1", "max_width": max_width})["structuredContent"]
        ref, scale = meta["screenshot_ref"], meta["scale"]
        cx, cy = meta["captured"]["x"], meta["captured"]["y"]
        say(f"{ref}: capture scale {scale}")
        for px_s, py_s in [(10, 10), (w - 10, 10), (10, h - 10), (w - 10, h - 10), (w / 2, h / 2)]:
            lx, ly = sx + px_s, sy + py_s
            px, py = math.floor((lx - cx) * scale), math.floor((ly - cy) * scale)
            ex, ey = cx + (px + 0.5) / scale - sx, cy + (py + 0.5) / scale - sy
            offset = os.path.getsize(log_path)
            res = tool("pointer_move", {"screenshot_ref": ref, "x": px, "y": py})
            assert res["structuredContent"].get("observed") == "sent", res
            seen = None
            for _ in range(50):
                with open(log_path) as f:
                    f.seek(offset); text = f.read()
                for line in text.splitlines():
                    if "wl_pointer]" in line and ("motion:" in line or "enter:" in line) and "x, y: " in line:
                        x, y = map(float, line.split("x, y: ")[1].split(", "))
                        seen = (x, y); break
                if seen: break
                time.sleep(0.1)
            assert seen, "no motion in wev"
            off = max(abs(seen[0] - ex), abs(seen[1] - ey))
            worst = max(worst, off)
            say(f"pixel ({px}, {py}) -> ({ex:.4f}, {ey:.4f}); wev ({seen[0]:.6f}, {seen[1]:.6f}), off by {off:.4f}: {'pass' if off <= 0.05 else 'fail'}")
    tool("release_desktop", {})
    say(f"worst {worst:.4f}: {'pass' if worst <= 0.05 else 'fail'}")
finally:
    if server:
        server.stdin.close(); server.wait(timeout=5)
    wev.terminate(); wev.wait(timeout=5)
