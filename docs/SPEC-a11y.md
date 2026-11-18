# Spec: deskctl a11y (accessibility tree)

Status: approved 2026-11-09. Extends [SPEC.md](SPEC.md) (v1).

## Objective

Let an agent find and use UI elements by role and name, like Android's
UiAutomator, instead of reading pixels: read an app's accessibility tree
(AT-SPI), find nodes, and run their actions (press, set text, focus).
Screenshots stay for what the tree does not show (look, layout, custom
drawing).

Why: an action needs no coordinates. That matters on niri, where tiled
windows have no screen position, and on Wayland, where AT-SPI positions
are relative to the window. Names and states are exact where OCR from a
screenshot guesses.

Checked on 2026-11-09 (nested niri, GNOME Calculator, GTK 4): 112 nodes,
34 buttons with names and a `click` action; clicking `7 + 2 =` through
AT-SPI alone gave `9`. Custom Qt Quick items may need `Accessible`
properties to appear in the tree.

Non-goals: screen-reader features (speech, events, focus tracking),
watching for changes, X11 apps without an AT-SPI bridge, OCR.

## Interface

A new group, so permission rules can tell reading from acting:

```
deskctl <nested|session> a11y <command>

  apps                          apps that export a tree on this target:
                                pid, name, toolkit, their niri window ids
  tree [--app PID | --window ID] [--depth N] [--max-nodes N] [--all]
                                the tree as JSON (default: nodes that
                                are showing; --all: every node)
  find [--app PID | --window ID] [--role ROLE] [--name NAME | --name-re RE]
       [--state STATE]...       matching nodes as a JSON list
  act  [--expect APP[/TITLE]] ID [ACTION]
                                run an action (default: press, click,
                                activate or toggle; else the first one;
                                Qt lists "Set Focus" first for some roles)
  set-text [--expect ...] ID TEXT
                                replace an editable text's contents
  focus [--expect ...] ID       give the node keyboard focus
```

A node (JSON):

```json
{"id": ":1.234/org/a11y/atspi/accessible/42", "role": "button",
 "name": "7", "states": ["focusable", "showing"], "actions": ["click"],
 "text": null, "value": null,
 "bounds": {"x": 221, "y": 1106, "w": 132, "h": 44},
 "children": [...]}
```

- `id` is the node's D-Bus bus name and object path. It stays valid while
  the app runs and the node exists; a stale id is an error (exit 1).
- `bounds` are relative to the node's window (Wayland gives apps no
  screen position). `null` when the node has no position.
- `text` holds the first 200 characters of a text node (`--text-max`
  changes it); `value` holds a slider's or spin box's current value.
- `role` is the name of the numeric AT-SPI role as the `atspi` crate
  writes it (`button`, `text`, `password text`), the same for every
  toolkit; toolkits' own
  role strings differ (Qt calls a password field `text`, GTK a `text
  box`). `--role` matches it case-insensitively; `--name` matches
  exactly; `--name-re` is a regular expression.
- Defaults: `--depth 40`, `--max-nodes 2000`; when a limit cuts the tree
  the JSON says so (`"truncated": true`).

## Which apps belong to a target

The AT-SPI bus is shared by every app of the user, in the session and
in nested niris. An app belongs to the niri whose `WAYLAND_DISPLAY` is
in its environment (`/proc/<pid>/environ`). An app with no
`WAYLAND_DISPLAY` (or another one) belongs to neither and is not shown.

`--window ID` maps a niri window to its app through the window's `pid`
(`niri msg -j windows`).

## Making apps export a tree

| Toolkit | Exports when |
|---|---|
| GTK 4 | always (checked) |
| GTK 3 | always, through the ATK bridge |
| Qt 5/6 | the a11y bus says `IsEnabled`, or `QT_LINUX_ACCESSIBILITY_ALWAYS_ON=1` |
| Firefox, Thunderbird | `IsEnabled`, or `GNOME_ACCESSIBILITY=1` |
| Chromium, Electron | `--force-renderer-accessibility` |

- `nested run` adds `QT_LINUX_ACCESSIBILITY_ALWAYS_ON=1` and
  `GNOME_ACCESSIBILITY=1` to the environment.
- The session stays as it is (`IsEnabled` is false now): only GTK apps
  show there unless the user decides otherwise (Open question 1).
- `apps` lists an app that has a niri window but exports nothing with
  `"exports": false` and the reason to try.

## Safety

- `apps`, `tree`, `find` only read. On the session they can return the
  text on screen (messages, documents), like a screenshot. They write the
  session activity mark (the bar shows automation as active).
- `act`, `set-text`, `focus` change an app. On the session they get the
  same guards as input (SPEC.md, Safety), with one change: `--expect`
  must match the window of the node's app (its app_id and title), not the
  focused window, because an action needs no focus. Missing `--expect`
  on the session is a usage error.
- Never `set-text` a password or secret field (role `password text`):
  refused (exit 3) on both targets.

## Tech

- `atspi` 0.30 (`connection`, `proxies`; zbus with its default async-io
  executor, run with `zbus::block_on`: no tokio).
- Accessible, Component (bounds, focus), Action, Text, EditableText,
  Value interfaces. Walk with `GetChildren`; the app's Cache interface is
  a later speed-up if big trees are slow.
- New module `src/a11y.rs`; pure parts (selectors, id parsing, JSON
  shaping, environ matching) unit-tested.

## Testing

- Unit: id parse/format, role/name/state matching, `--depth` and
  `--max-nodes` cuts, environ → target, `--expect` against a window.
- End to end (nested, `#[ignore]`), with test apps that keep no user
  state:
  - `tests/fixtures/probe.qml` gains a Qt Quick Controls `Button`
    ("Increment") and a `TextField` ("Name"); it already prints clicks
    and text.
  - `tests/fixtures/probe_gtk.py` (new): a GTK 4 window with the same
    button and entry, printing JSON lines; run with `python3 -I`.
- Session: read-only commands by the agent; `act` only with the user's
  go-ahead, into a test window.

## Success criteria

1. `nested a11y apps` lists the Qt and GTK probes with pid and window id,
   and no session app.
2. `tree --window ID` returns JSON with the probe's button and text field
   (roles, names, actions, bounds); `--depth 1` and `--max-nodes 5` cut
   it and say so.
3. `find --role button --name Increment` returns one node per probe
   (Qt and GTK).
4. `act ID` on that node: the probe prints a click. `set-text ID 'Hello,
   мир'` on the field: the probe prints that text. `focus ID`: the Qt
   field reports `focused`; GTK 4 does not support focus over AT-SPI
   (`NotSupported`), so deskctl says to click instead (exit 1).
5. A stale id exits 1 with a clear message; an unknown action exits 2.
6. Session: `a11y act` without `--expect` exits 2; with a wrong one, or
   with the stop file, exits 3 and the app gets nothing; read-only
   commands write the activity mark.
7. A tree of up to 2000 nodes comes back in under 2 s.
8. The skill explains when to use the tree and when screenshots, and
   how to make apps export.

## Decisions

1. **Session export:** `IsEnabled` stays off; only GTK apps are readable
   on the session. A switch comes when a real task needs a Qt or Firefox
   app there.
2. **Id format:** the exact `busname/path`; no state file.
