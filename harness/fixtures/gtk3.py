"""GTK 3 fixture for the accessibility checks; only connect to the harness's private
Wayland session.

A 400x300 window with a label, a button that counts its activations in
`TEST_DIR/gtk3-count`, and an entry. The program name sets the Wayland app_id,
`org.ncu.Gtk3`.
"""

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

gi.require_version("Gtk", "3.0")
from gi.repository import Gio, GLib, Gtk

GLib.set_prgname("org.ncu.Gtk3")
clicks = 0


def report(text):
    """Replaces the file whole, so a reader never sees half of it."""
    partial = root / ".gtk3-count"
    partial.write_text(text, encoding="utf-8")
    partial.replace(root / "gtk3-count")


def clicked(button):
    global clicks
    clicks += 1
    button.set_label(f"Gtk3: {clicks}")
    report(str(clicks))


def activate(application):
    window = Gtk.ApplicationWindow(application=application)
    window.set_default_size(400, 300)
    window.set_title("gtk3 fixture")
    box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=12, margin=20)
    box.pack_start(Gtk.Label(label="GTK 3 fixture"), False, False, 0)
    button = Gtk.Button(label="Gtk3: 0")
    button.connect("clicked", clicked)
    box.pack_start(button, False, False, 0)
    box.pack_start(Gtk.Entry(), False, False, 0)
    window.add(box)
    report("0")
    window.show_all()


app = Gtk.Application(application_id="org.ncu.Gtk3", flags=Gio.ApplicationFlags.NON_UNIQUE)
app.connect("activate", activate)
GLib.timeout_add_seconds(240, app.quit)
app.run([])
