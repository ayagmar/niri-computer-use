"""GTK 4 apps for the skill evals; only connect to the harness's private Wayland session.

`signup` is a `Sign up` form: `Name` and `Email` entries, a `Send me the newsletter`
check box, a `Plan` drop-down (Free, Team or Enterprise) and a `Sign up` button, which
appends the fields' values to `signups.jsonl` and says who signed up. `draft` is a
`Message` entry and a `Send` button; Enter in the entry sends too, and each send appends
the text to `sent.jsonl` and clears the entry. The first time the entry gets text, a
modal `Autosave failed` window opens over it and the form takes no input until its `OK`
button, Escape or a close request dismisses it; `draft-events.jsonl` records when it was
shown and dismissed.
"""

import json
import os
from pathlib import Path
import sys

root = Path(sys.argv[1]).resolve(strict=True)
mode = sys.argv[2]
seconds = int(sys.argv[3])
if mode not in ("signup", "draft"):
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
from gi.repository import Gdk, Gio, GLib, Gtk

PLANS = ["Free", "Team", "Enterprise"]


def append(name, record):
    with open(root / name, "a", encoding="utf-8") as file:
        file.write(json.dumps(record) + "\n")


def labelled(widget, label):
    widget.update_property([Gtk.AccessibleProperty.LABEL], [label])
    return widget


def form_grid():
    grid = Gtk.Grid(row_spacing=8, column_spacing=12)
    for side in ("top", "bottom", "start", "end"):
        getattr(grid, f"set_margin_{side}")(16)
    return grid


def signup_window(window):
    grid = form_grid()
    name = labelled(Gtk.Entry(hexpand=True), "Name")
    email = labelled(Gtk.Entry(hexpand=True, input_purpose=Gtk.InputPurpose.EMAIL), "Email")
    newsletter = Gtk.CheckButton(label="Send me the newsletter")
    plan = labelled(Gtk.DropDown.new_from_strings(PLANS), "Plan")
    for row, (label, widget) in enumerate(
        (("Name", name), ("Email", email), ("Plan", plan))
    ):
        grid.attach(Gtk.Label(label=label, xalign=1), 0, row, 1, 1)
        grid.attach(widget, 1, row, 1, 1)
    grid.attach(newsletter, 1, 3, 1, 1)
    status = Gtk.Label(label="", xalign=0)

    def signed_up(_button):
        append(
            "signups.jsonl",
            {
                "name": name.get_text(),
                "email": email.get_text(),
                "newsletter": newsletter.get_active(),
                "plan": PLANS[plan.get_selected()],
            },
        )
        status.set_label(f"Signed up {email.get_text()}")

    submit = Gtk.Button(label="Sign up")
    submit.connect("clicked", signed_up)
    grid.attach(submit, 1, 4, 1, 1)
    grid.attach(status, 1, 5, 1, 1)
    window.set_child(grid)
    window.set_title("Sign up")


def autosave_failed(parent, form):
    dialog = Gtk.Window(transient_for=parent, modal=True, title="Autosave failed")
    box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=12)
    for side in ("top", "bottom", "start", "end"):
        getattr(box, f"set_margin_{side}")(16)
    box.append(
        Gtk.Label(label="Autosave failed: the drafts folder is read-only.\nYour text is still here.")
    )
    ok = Gtk.Button(label="OK")
    box.append(ok)
    dialog.set_child(box)
    dialog.set_default_widget(ok)

    def dismissed(how):
        append("draft-events.jsonl", {"event": "dismissed", "how": how})
        form.set_sensitive(True)
        dialog.destroy()

    ok.connect("clicked", lambda _button: dismissed("ok"))
    dialog.connect("close-request", lambda _dialog: dismissed("close") or True)
    escape = Gtk.ShortcutController()
    escape.add_shortcut(
        Gtk.Shortcut.new(
            Gtk.ShortcutTrigger.parse_string("Escape"),
            Gtk.CallbackAction.new(lambda *_: dismissed("escape") or True),
        )
    )
    dialog.add_controller(escape)
    form.set_sensitive(False)
    append("draft-events.jsonl", {"event": "shown"})
    dialog.present()


def draft_window(window):
    grid = form_grid()
    message = labelled(Gtk.Entry(hexpand=True), "Message")
    grid.attach(Gtk.Label(label="Message", xalign=1), 0, 0, 1, 1)
    grid.attach(message, 1, 0, 1, 1)
    send = Gtk.Button(label="Send")
    grid.attach(send, 1, 1, 1, 1)
    shown = False

    def changed(entry):
        nonlocal shown
        if entry.get_text() and not shown:
            shown = True
            GLib.idle_add(autosave_failed, window, grid)

    def sent(_widget):
        append("sent.jsonl", {"text": message.get_text()})
        message.set_text("")

    message.connect("changed", changed)
    message.connect("activate", sent)
    send.connect("clicked", sent)
    window.set_child(grid)
    window.set_title("Draft")
    message.grab_focus()


def activate(application):
    window = Gtk.ApplicationWindow(application=application)
    window.set_default_size(480, 300)
    {"signup": signup_window, "draft": draft_window}[mode](window)
    window.present()


app = Gtk.Application(
    application_id=f"org.ncu.{mode.capitalize()}",
    flags=Gio.ApplicationFlags.NON_UNIQUE,
)
app.connect("activate", activate)
GLib.timeout_add_seconds(seconds, app.quit)
app.run([])
