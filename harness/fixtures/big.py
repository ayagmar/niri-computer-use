"""GTK 3 and GTK 4 fixture with one big container; only connect to the harness's private
Wayland session.

Run as `big.py <TEST_DIR> <gtk3|gtk4> <buttons|rows|tree> <count>`. A 400x300 window:
a `Save` button and a `Search` entry above a scrolled container of `count` children,
then an `Apply` button, a `Close` button and a `Notes` entry below it. `buttons` holds
`Open 1` to `Open <count>` buttons in a box. `rows` (GTK 4 only) holds a list box of
`count` rows, each a `Record <n>` label and a `Delete` button, whose accessible name is
its record. `tree` (GTK 3 only) is a tree view of `count` rows of three text columns. Every name and
value is synthetic.
"""

import os
from pathlib import Path
import sys

root = Path(sys.argv[1]).resolve(strict=True)
toolkit, kind, count = sys.argv[2], sys.argv[3], int(sys.argv[4])
if toolkit not in ("gtk3", "gtk4") or kind not in ("buttons", "rows", "tree"):
    raise RuntimeError(f"unknown fixture {toolkit} {kind}")
if (kind, toolkit) in (("tree", "gtk4"), ("rows", "gtk3")):
    raise RuntimeError(f"no {kind} in {toolkit}")
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

gi.require_version("Gtk", "3.0" if toolkit == "gtk3" else "4.0")
from gi.repository import Gio, GLib, Gtk

GTK3 = toolkit == "gtk3"


def button(name):
    return Gtk.Button(label=name)


def labelled(widget, label):
    if GTK3:
        widget.get_accessible().set_name(label)
    else:
        widget.update_property([Gtk.AccessibleProperty.LABEL], [label])
    return widget


def box(orientation, children):
    widget = Gtk.Box(orientation=orientation, spacing=6)
    for child in children:
        if GTK3:
            widget.pack_start(child, False, False, 0)
        else:
            widget.append(child)
    return widget


def buttons():
    buttons = [button(f"Open {n}") for n in range(1, count + 1)]
    return box(Gtk.Orientation.VERTICAL, buttons)


def rows():
    listing = Gtk.ListBox()
    for n in range(1, count + 1):
        record = f"Record {n}"
        row = Gtk.ListBoxRow()
        content = box(
            Gtk.Orientation.HORIZONTAL, [Gtk.Label(label=record), button("Delete")]
        )
        row.set_child(content)
        listing.insert(labelled(row, record), -1)
    return listing


def tree():
    store = Gtk.ListStore(str, str, str)
    for n in range(1, count + 1):
        store.append([f"Name {n}", f"Size {n}", f"Kind {n % 7}"])
    view = Gtk.TreeView(model=store)
    for column, title in enumerate(("Name", "Size", "Kind")):
        view.append_column(Gtk.TreeViewColumn(title, Gtk.CellRendererText(), text=column))
    return view


def activate(application):
    window = Gtk.ApplicationWindow(application=application)
    window.set_default_size(400, 300)
    window.set_title(f"big {toolkit} {kind} fixture")
    scrolled = Gtk.ScrolledWindow(vexpand=True, hexpand=True)
    content = {"buttons": buttons, "rows": rows, "tree": tree}[kind]()
    top = box(Gtk.Orientation.HORIZONTAL, [button("Save"), labelled(Gtk.Entry(), "Search")])
    bottom = box(
        Gtk.Orientation.HORIZONTAL,
        [button("Apply"), button("Close"), labelled(Gtk.Entry(), "Notes")],
    )
    if GTK3:
        scrolled.add(content)
        whole = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)
        whole.pack_start(top, False, False, 0)
        whole.pack_start(scrolled, True, True, 0)
        whole.pack_start(bottom, False, False, 0)
        window.add(whole)
        window.show_all()
    else:
        scrolled.set_child(content)
        window.set_child(box(Gtk.Orientation.VERTICAL, [top, scrolled, bottom]))
        window.present()


app = Gtk.Application(
    application_id=f"org.ncu.Big.{toolkit}", flags=Gio.ApplicationFlags.NON_UNIQUE
)
app.connect("activate", activate)
GLib.timeout_add_seconds(240, app.quit)
app.run([])
