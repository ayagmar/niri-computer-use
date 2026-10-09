"""GTK activation fixture; only connects to the harness's private Wayland session."""

import os
from pathlib import Path
import sys

root = Path(sys.argv[1]).resolve(strict=True)
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
app = Gtk.Application(application_id="org.ncu.Activation", flags=Gio.ApplicationFlags.NON_UNIQUE)
clicks = 0


def clicked(button):
    global clicks
    clicks += 1
    button.set_label(f"Activations: {clicks}")
    counter.write_text(str(clicks), encoding="utf-8")


def activate(application):
    window = Gtk.ApplicationWindow(application=application)
    window.set_default_size(320, 240)
    window.set_title("Activation fixture")
    button = Gtk.Button(label="Activations: 0")
    button.connect("clicked", clicked)
    window.set_child(button)
    counter.write_text("0", encoding="utf-8")
    window.present()


app.connect("activate", activate)
GLib.timeout_add_seconds(45, app.quit)
app.run([])
