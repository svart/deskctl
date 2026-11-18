"""GTK 4 test probe for deskctl's accessibility tests: a window with a
button, a name entry and a password entry that prints what happens to
them as JSON lines on stdout. Keeps no state. Run: python3 -I probe_gtk.py
"""
import json
import sys

import gi

gi.require_version("Gtk", "4.0")
from gi.repository import Gio, Gtk  # noqa: E402


def emit(**ev):
    print(json.dumps(ev), flush=True)


def activate(app):
    win = Gtk.ApplicationWindow(application=app, title="deskctl-probe-gtk")
    box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=8)
    for side in ("top", "bottom", "start", "end"):
        getattr(box, f"set_margin_{side}")(20)

    button = Gtk.Button(label="Increment")
    button.connect("clicked", lambda b: emit(ev="button", name="Increment"))

    name = Gtk.Entry(placeholder_text="Name")
    name.update_property([Gtk.AccessibleProperty.LABEL], ["Name"])
    name.connect("changed", lambda e: emit(ev="field", name="Name", text=e.get_text()))
    focus = Gtk.EventControllerFocus()
    focus.connect("enter", lambda c: emit(ev="focus", name="Name", focused=True))
    name.add_controller(focus)

    password = Gtk.PasswordEntry()
    password.update_property([Gtk.AccessibleProperty.LABEL], ["Password"])
    password.connect("changed", lambda e: emit(ev="field", name="Password", len=len(e.get_text())))

    for w in (button, name, password):
        box.append(w)
    win.set_child(box)
    win.present()
    emit(ev="ready")


# NON_UNIQUE: each run is its own process, even while another runs.
app = Gtk.Application(application_id="org.deskctl.ProbeGtk", flags=Gio.ApplicationFlags.NON_UNIQUE)
app.connect("activate", activate)
sys.exit(app.run([]))
