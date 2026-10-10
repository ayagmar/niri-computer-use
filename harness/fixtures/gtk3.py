"""GTK 3 fixture for the accessibility checks; only connect to the harness's private
Wayland session.

A 400x300 window with a label, a button that counts its activations in
`TEST_DIR/gtk3-count`, the `Plain entry`, which reports its text in `gtk3-entry-text`,
the `Password entry`, which hides its text and counts its changes in
`gtk3-password-changes` without the text, and a `Dismiss` button that closes the window.
The program name sets the Wayland app_id, `org.ncu.Gtk3`.
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
password_changes = 0


def report(name, text):
    """Replaces the file whole, so a reader never sees half of it."""
    partial = root / f".{name}"
    partial.write_text(text, encoding="utf-8")
    partial.replace(root / name)


def clicked(button):
    global clicks
    clicks += 1
    button.set_label(f"Gtk3: {clicks}")
    report("gtk3-count", str(clicks))


def password_changed(entry):
    global password_changes
    password_changes += 1
    report("gtk3-password-changes", str(password_changes))


def labelled(widget, label):
    widget.get_accessible().set_name(label)
    return widget


def activate(application):
    window = Gtk.ApplicationWindow(application=application)
    window.set_default_size(400, 300)
    window.set_title("gtk3 fixture")
    grid = Gtk.Grid(row_spacing=8, column_spacing=8, column_homogeneous=True, margin=20)
    grid.attach(Gtk.Label(label="GTK 3 fixture"), 0, 0, 2, 1)
    button = Gtk.Button(label="Gtk3: 0")
    button.connect("clicked", clicked)
    grid.attach(button, 0, 1, 1, 1)
    dismiss = Gtk.Button(label="Dismiss")
    dismiss.connect("clicked", lambda button: window.close())
    grid.attach(dismiss, 1, 1, 1, 1)
    entry = labelled(Gtk.Entry(), "Plain entry")
    entry.connect("changed", lambda entry: report("gtk3-entry-text", entry.get_text()))
    grid.attach(entry, 0, 2, 1, 1)
    password = labelled(Gtk.Entry(visibility=False), "Password entry")
    password.connect("changed", password_changed)
    grid.attach(password, 1, 2, 1, 1)
    window.add(grid)
    report("gtk3-count", "0")
    report("gtk3-entry-text", "")
    report("gtk3-password-changes", "0")
    window.show_all()


app = Gtk.Application(application_id="org.ncu.Gtk3", flags=Gio.ApplicationFlags.NON_UNIQUE)
app.connect("activate", activate)
GLib.timeout_add_seconds(240, app.quit)
app.run([])
