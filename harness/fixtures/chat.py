"""A stand-in chat window for the demo recordings; only connects to the harness's private
Wayland session.

A libadwaita window, app_id `org.ncu.Chat`, showing the `#release` channel with two
synthetic messages, a `Message` entry and a `Send` button. Sending, by the button or by
Enter in the entry, adds the entry's text as the user's message, clears the entry and
appends the text to `chat-sent`, one message per line.
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

gi.require_version("Gtk", "4.0")
gi.require_version("Adw", "1")
from gi.repository import Adw, Gio, Gtk

MESSAGES = [
    ("Maya", "14:02", "Is the release review still at 2?"),
    ("Jon", "14:03", "I'm on a call until 2:30, so I'd miss the start."),
]

CSS = """
.chat-author { font-weight: 700; }
.chat-time { opacity: 0.55; font-size: 0.85em; }
.chat-bubble {
    background: alpha(currentColor, 0.07);
    border-radius: 12px;
    padding: 8px 12px;
}
.chat-mine .chat-bubble { background: alpha(@accent_bg_color, 0.35); }
.chat-list { background: none; }
.chat-list row { background: none; padding: 6px 4px; }
"""

app = Adw.Application(application_id="org.ncu.Chat", flags=Gio.ApplicationFlags.NON_UNIQUE)


def message_row(author, time, text, mine=False):
    box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=4)
    box.set_halign(Gtk.Align.END if mine else Gtk.Align.START)
    if mine:
        box.add_css_class("chat-mine")
    heading = Gtk.Box(spacing=8)
    heading.set_halign(Gtk.Align.END if mine else Gtk.Align.START)
    author_label = Gtk.Label(label=author)
    author_label.add_css_class("chat-author")
    time_label = Gtk.Label(label=time)
    time_label.add_css_class("chat-time")
    heading.append(author_label)
    heading.append(time_label)
    bubble = Gtk.Label(label=text, wrap=True, xalign=0)
    bubble.set_max_width_chars(34)
    bubble.add_css_class("chat-bubble")
    box.append(heading)
    box.append(bubble)
    row = Gtk.ListBoxRow(activatable=False, selectable=False)
    row.set_child(box)
    return row


def activate(app):
    Adw.StyleManager.get_default().set_color_scheme(Adw.ColorScheme.FORCE_DARK)
    provider = Gtk.CssProvider()
    provider.load_from_string(CSS)
    Gtk.StyleContext.add_provider_for_display(
        Gtk.Widget.get_display(Gtk.Label()), provider, Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION
    )

    messages = Gtk.ListBox(selection_mode=Gtk.SelectionMode.NONE)
    messages.add_css_class("chat-list")
    for author, time, text in MESSAGES:
        messages.append(message_row(author, time, text))
    scrolled = Gtk.ScrolledWindow(vexpand=True, child=messages)
    scrolled.set_margin_start(12)
    scrolled.set_margin_end(12)

    entry = Gtk.Entry(hexpand=True, placeholder_text="Message #release")
    entry.update_property([Gtk.AccessibleProperty.LABEL], ["Message"])
    send = Gtk.Button(label="Send")
    send.add_css_class("suggested-action")
    composer = Gtk.Box(spacing=8)
    for side in ("top", "bottom", "start", "end"):
        getattr(composer, f"set_margin_{side}")(12)
    composer.append(entry)
    composer.append(send)

    def sent(_widget):
        text = entry.get_text()
        if not text:
            return
        messages.append(message_row("You", "14:04", text, mine=True))
        entry.set_text("")
        with (root / "chat-sent").open("a", encoding="utf-8") as log:
            log.write(text + "\n")

    entry.connect("activate", sent)
    send.connect("clicked", sent)

    header = Adw.HeaderBar()
    header.set_title_widget(Adw.WindowTitle(title="#release", subtitle="Maya, Jon and you"))
    view = Adw.ToolbarView(content=scrolled)
    view.add_top_bar(header)
    view.add_bottom_bar(composer)
    window = Adw.ApplicationWindow(application=app, title="#release", content=view)
    window.present()
    entry.grab_focus()


app.connect("activate", activate)
app.run([sys.argv[0]])
