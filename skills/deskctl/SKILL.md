---
name: deskctl
description: See and drive the niri Wayland desktop with the deskctl CLI — screenshots, windows, outputs, layer-shell surfaces, niri actions, typing, key combos, pointer moves, clicks and scrolls, and apps' accessibility trees (find buttons and fields by role and name, press them, set text). Use to test a GUI app, shell, or bar in a nested niri without the user's hands, or, when the user asks for it, to act on their real desktop session. Not for X11, other compositors, or web pages (use a browser tool).
---

# deskctl: see and drive a niri desktop

`deskctl <nested|session> <command>`. The target comes first. Run
`deskctl <target> <command> --help` for flags instead of guessing. See
`docs/SPEC.md` in the deskctl checkout for details.

| Target | What | Input |
|---|---|---|
| `nested` | a niri started by deskctl as a window, on its own workspace, without focus | input goes to the nested compositor |
| `session` | the user's real desktop | guarded; only when the user asked for it |

Default to `nested` for any testing. Use `session` only when the task is
about the real desktop and the user asked for it.

## Nested: test an app or a shell

Use a separate workspace in the host niri session for each new nested
environment. This keeps test windows out of the user's working layout.
`nested start` already moves its window to an empty host workspace without
switching focus; keep it there while testing.

```sh
deskctl nested start [--config FILE]     # prints pid and sockets as JSON
deskctl nested run -- APP ARGS...        # prints {"pid", "log"}; app output goes to the log
deskctl nested windows | layers | outputs
deskctl nested shot --scale 0.5 /path/overview.png
deskctl nested click --at X,Y
deskctl nested type 'text'
deskctl nested key ctrl+s Return
deskctl nested stop                      # always stop it when done
```

- `run` keeps the current directory and environment and adds the
  nested `WAYLAND_DISPLAY` and `NIRI_SOCKET`. Apps run with the caller's
  usual permissions. Set any test-specific app environment before `deskctl`.
- The nested niri has one output (about the size of the screen) and
  no binds of its own. Give a config for layouts or rules:
  `--config` with a niri KDL file (write child nodes on their own lines;
  `a { b }` on one line does not parse).
- An app may need a second or two to map; poll `windows` / `layers`
  rather than sleeping blindly.
- For a popup drawn inside an existing surface, use a screenshot to see it.
- `outputs` reports the nested output as `transform: Flipped180`. niri
  sets that to fix its nested rendering; screenshots and clicks are not
  flipped.
- Keys reach toplevel windows. A popup on a layer surface without keyboard
  focus may need a click to close it.
- After `stop`, `status` exits 1 with "no nested niri is running".

## See: screenshots and coordinates

`shot` prints how image pixels map to logical coordinates:

```json
{"file": "...", "region": {"x": 0, "y": 0, "w": 2238, "h": 1362},
 "image": {"w": 1119, "h": 681}, "scale": 0.5}
```

logical = region.x + pixel_x / scale (same for y).

- When you view an image, the viewer may shrink it and say so ("displayed
  at W×H, multiply by F"). Multiply what you read by F first to get image
  pixels, then apply the mapping.
- Start with `--scale 0.5` for an overview, then `--region X,Y,WxH` at
  scale 1 to read small text and aim precisely.
- niri gives no screen position for tiled windows, and there is no
  window screenshot: find things on screen in a screenshot.

## Act: pointer and keyboard

- `click --at X,Y [left|right|middle] [--double]`, `move X Y`,
  `scroll [--at X,Y] DY [DX]` (positive DY is down, in wheel steps).
  Coordinates are logical, as in `outputs`.
- `type TEXT` types any Unicode text under any layout. Keys are paced
  (`--delay-ms 5`); `\n` is Return, `\t` is Tab.
- `key COMBO...`: `ctrl+shift+t`, `alt+F4`, `super+1`, `Return`, `Escape`,
  `Page_Down`, any XKB keysym name. Keys go to the focused client only:
  niri binds do not fire. Use `action <niri action>` for those
  (`action focus-workspace 2`, `action spawn -- foot`).
- After acting, check the effect (screenshot, `windows`, the app's log)
  before the next step.

## Accessibility tree: find elements by role and name

Like Android's UiAutomator: read an app's tree (AT-SPI), find a node,
run its action. No coordinates, exact names and states. Prefer it to
pixels for apps that export a tree; use screenshots for the look and for
apps that export nothing.

```sh
deskctl nested a11y apps                      # pid, toolkit, windows, root id; or exports:false + hint
deskctl nested a11y tree --window ID          # JSON tree of showing nodes (--all, --depth, --max-nodes)
deskctl nested a11y find --window ID --role button --name Increment
deskctl nested a11y act ID [ACTION]           # default: Press/Click/Activate/Toggle, else the first
deskctl nested a11y set-text ID 'text'        # replaces the field's text
deskctl nested a11y focus ID                  # Qt yes; GTK 4 cannot (exit 1): click instead
```

- A node: `id` (`:1.120/org/a11y/...`, valid while the app and node
  live), `role`, `name`, `states`, `actions`, `text`, `value`, `bounds`.
- Roles are the same for every toolkit: `button`, `text`,
  `password text`, `check box`, `label`, `frame`... `--role` ignores case;
  `--name` is exact, `--name-re` a regular expression, `--state` repeats.
- `bounds` are relative to the node's window, not the screen. To click a
  node with the pointer you still need the window's place from a
  screenshot; prefer `act`.
- Which apps export a tree: GTK always; Qt and Firefox in a nested niri
  (`nested run` switches it on). On the session only GTK apps (Qt and
  Firefox stay off there by the user's choice); Chromium and Electron
  need `--force-renderer-accessibility`. Custom-drawn UIs may lack an
  accessibility tree.
- `set-text` on a password field is refused (exit 3). A node of an app
  on another desktop than the target is refused (exit 3).
- On the session, `act`, `set-text` and `focus` are input: `--expect`
  (matched against the node's app windows, not the focused one), the
  idle wait and the stop file apply. Reading (`apps`, `tree`, `find`)
  shows on-screen text, like a screenshot.

## Session: the real desktop

Rules, every time:

1. Act on the session only when the user asked for it. Reading
   (`windows`, `focused`, `layers`, `outputs`) is fine. Screenshots
   show private content: take a region, not the whole screen, unless the
   task needs it.
2. `type`, `key`, `click` and `scroll` require `--expect APP[/TITLE]`.
   Use the exact app_id of the focused window and optionally part of its
   title. Deskctl refuses a mismatch with exit 3. Get both from
   `deskctl session focused`.
3. A click lands on the window under the pointer, not necessarily the
   focused one: `focus ID` first, take a screenshot, then click inside it.
4. Pointer, keyboard, and accessibility input wait for 1.5 s of user idle
   time by default (`--idle-ms`). If the wait times out, tell the user the
   session is busy. Compositor commands `action` and `focus` use the stop
   file guard and run without an idle wait.
5. Never type passwords or secrets.
6. `$XDG_RUNTIME_DIR/deskctl/stop` blocks session input, accessibility
   mutations, `action`, and `focus` while it exists. Deskctl checks again
   before sending input, including between keystrokes and clicks. If the
   file appears during a command, the events already sent can take effect.

## Exit codes and logs

| Exit | Meaning |
|---|---|
| 0 | done |
| 1 | error (message on stderr) |
| 2 | usage error (bad flag, unknown key name, missing `--expect`) |
| 3 | a guard blocked the command or stopped further input |

- Every command: one JSON line in `~/.local/state/deskctl/log.jsonl`.
- `nested run` app output: the `log` path it printed.
- The nested niri's own output: `$XDG_RUNTIME_DIR/deskctl/nested.log`.

## Limits

- No second monitor, lid or window screenshots; ask the user for checks
  that need those.
