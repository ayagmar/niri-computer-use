"""GTK fixtures; only connect to the harness's private Wayland session.

`button` counts a button's real activations. `entry` reports a text entry's text after
every change, and each activation (Enter) with the text it submitted, then clears it.
"""

import json
import os
from pathlib import Path
import sys

root = Path(sys.argv[1]).resolve(strict=True)
mode = sys.argv[2]
if mode not in ("button", "entry"):
    raise RuntimeError(f"unknown fixture {mode}")
run = root / "run"
for name in ("NIRI_SOCKET", "WAYLAND_DISPLAY"):
    endpoint = Path(os.environ[name])
    if not endpoint.is_absolute():
        endpoint = run / endpoint
    if not endpoint.resolve(strict=True).is_relative_to(run):
        raise RuntimeError(f"{name} is not nested")
if Path(os.environ["XDG_RUNTIME_DIR"]) != run:
    raise RuntimeError("runtime is not nested")

os.environ["HOME"] = str(root)
os.environ["GDK_BACKEND"] = "wayland"
import gi

gi.require_version("Gtk", "4.0")
from gi.repository import Gio, GLib, Gtk

counter = root / "activations"
app_id = "org.ncu.Activation" if mode == "button" else "org.ncu.Entry"
app = Gtk.Application(application_id=app_id, flags=Gio.ApplicationFlags.NON_UNIQUE)
clicks = 0
submits = 0


def report(name, text):
    """Replaces the file whole, so a reader never sees half of it."""
    partial = root / f".{name}"
    partial.write_text(text, encoding="utf-8")
    partial.replace(root / name)


def clicked(button):
    global clicks
    clicks += 1
    button.set_label(f"Activations: {clicks}")
    counter.write_text(str(clicks), encoding="utf-8")


def changed(entry):
    report("entry-text", entry.get_text())


def submitted(entry):
    global submits
    submits += 1
    report("entry-submits", json.dumps({"count": submits, "text": entry.get_text()}))
    entry.set_text("")


def button_window(window):
    button = Gtk.Button(label="Activations: 0")
    button.connect("clicked", clicked)
    window.set_child(button)
    counter.write_text("0", encoding="utf-8")


def entry_window(window):
    entry = Gtk.Entry()
    entry.connect("changed", changed)
    entry.connect("activate", submitted)
    window.set_child(entry)
    report("entry-text", "")
    report("entry-submits", json.dumps({"count": 0, "text": ""}))
    entry.grab_focus()


def activate(application):
    window = Gtk.ApplicationWindow(application=application)
    window.set_default_size(320, 240)
    window.set_title(f"{mode} fixture")
    if mode == "button":
        button_window(window)
    else:
        entry_window(window)
    window.present()


app.connect("activate", activate)
GLib.timeout_add_seconds(45, app.quit)
app.run([])
