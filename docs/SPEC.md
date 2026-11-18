# Spec: deskctl

Status: approved 2026-11-09.

## Objective

`deskctl` lets a coding agent see and drive a niri Wayland desktop:
take screenshots, read windows and outputs, run niri actions, type,
press keys, move the pointer, click and scroll.

It works on two targets:

- **nested**: a niri started as a window inside the session. Deskctl sends
  input to the nested compositor. Programs run with the caller's normal
  permissions. Used to test shells and apps.
- **session**: the user's real niri session. Used when a task needs the
  real desktop. Input here is guarded (see Safety).

Users: Claude Code (through the skill in `skills/deskctl/`) and the
user from a terminal.

Success: an agent with only the skill can start a nested niri, run an
app in it, type into it, click a point in it, take a screenshot and read
it, and stop it, with no keyboard or mouse from the user.

Non-goals (v1): other compositors, X11, uinput (`ydotool`/`dotool`),
a second virtual output, recording video, OCR, accessibility (AT-SPI)
trees, an MCP server.

## Interface

The target is the first argument, so Claude Code permission rules can
match on a command prefix (`Bash(deskctl nested:*)`).

```
deskctl <nested|session> <command> [args]

Look
  shot [--output NAME | --region X,Y,WxH] [--scale F] FILE
                         screenshot (PNG); prints the mapping as JSON
  outputs | windows | workspaces | focused | layers
                         JSON from `niri msg -j ...`

Act on the compositor
  action <niri action> [args]     `niri msg action ...`
  focus <window-id>               focus-window --id

Input
  type  [--expect APP[/TITLE]] [--delay-ms 5] TEXT
  key   [--expect APP[/TITLE]] [--delay-ms 5] COMBO...   ctrl+shift+t  Return  alt+F4
  move  X Y
  click [--expect ...] [--at X,Y] [left|right|middle] [--double]
  scroll [--expect ...] [--at X,Y] DY [DX]

Nested only
  start [--config FILE]     start a nested niri, wait for its sockets
  stop                      stop it (by PID from the state file)
  status                    JSON: pid, WAYLAND_DISPLAY, NIRI_SOCKET
  run -- CMD [ARGS]         spawn CMD with the nested env, detached;
                            prints its PID
```

Coordinates: logical pixels in niri's global layout space (as in
`niri msg outputs`). `shot` prints how image pixels map to them:

```json
{"file":"/tmp/s.png","region":{"x":0,"y":0,"w":2240,"h":1400},
 "image":{"w":1120,"h":700},"scale":0.5}
```

logical = region.x + pixel_x / scale (same for y).

Accessibility trees (`deskctl <target> a11y …`): [SPEC-a11y.md](SPEC-a11y.md).

Exit codes: 0 done, 1 error, 2 usage error, 3 blocked by a guard.
When a command stops partway through, events already sent can take effect.

## Mechanisms

| Need | How |
|---|---|
| Screenshots | `grim` (`-o`, `-g`, `-s`) with `WAYLAND_DISPLAY` of the target |
| Windows, outputs, actions | `niri msg -j` with `NIRI_SOCKET` of the target, parsed as untyped JSON (no niri-ipc crate: no version lock to niri) |
| Typing | `zwp_virtual_keyboard_v1`; deskctl makes an XKB keymap that holds exactly the keysyms it needs, so any Unicode text types the same under any user layout (the method `wtype` uses) |
| Key combos | same device; modifiers by name (`ctrl`, `shift`, `alt`, `super`), keys by XKB keysym name |
| Pointer | `zwlr_virtual_pointer_v1` without an output; absolute motion relative to the bounding rectangle of all outputs (niri maps that with no transform) |
| User activity | `ext_idle_notifier_v1` v2 `get_input_idle_notification` |
| Nested niri | `niri -c CONFIG` (no `--session`) with `vblank_mode=0`; state in `$XDG_RUNTIME_DIR/deskctl/nested.json` |

Known limits:

- The nested niri runs on a workspace the user does not see. The host
  then sends it frame callbacks about once a second, and with vsync on
  each redraw blocks its event loop that long (a pointer move took 1 s,
  double clicks failed). `vblank_mode=0` (Mesa) turns the wait off; idle
  CPU stays below 1 %. Other GL drivers may ignore it.
- A virtual pointer bound to an output gets that output's transform
  applied to absolute motion (as in wlroots). niri v26.04 forces a fake
  `Flipped180` on a nested (winit) output to fix its rendering
  (`src/niri.rs`, FIXME in `src/input/backend_ext.rs`), so a bound pointer
  lands mirrored there. deskctl binds no output: niri then maps onto the
  global bounding rectangle with no transform, right for every output
  and transform.
- Keys are paced, 5 ms each by default: unpaced, 100k keys left in
  0.4 s while a Qt text field took ~1 ms per key, so the command
  returned long before the app had the text, and a big backlog can make
  the compositor drop the client.
- A popup that grabs focus on a layer surface with keyboard
  interactivity `None` got no virtual-keyboard keys
  in a nested niri (v26.04): Escape did not reach it. Not checked on the
  session.
- Virtual keyboard keys go to the focused client; niri key binds do not
  fire (checked: `super+t` did not run a `Super+T` bind). Use `action`.
- niri IPC has no screen position for tiled windows, so there is no
  window screenshot in v1 (see Decisions).
- Nested niri has one output; multi-output mapping is unit-tested and
  checked on the session only with a second monitor (user check).

## Safety (session target)

1. **Expect check.** `type`, `key`, `click`, `scroll` on the session
   need `--expect APP[/TITLE]`. deskctl reads the focused window; if its
   app_id is not APP (or its title does not contain TITLE) it exits 3.
   Missing `--expect` on the session is a usage error. A click lands on
   the window under the pointer, which may not be the focused one: focus
   the target first (`focus ID`).
2. **Idle check.** Before input, wait until the seat's input has been
   idle for 1500 ms (`--idle-ms`), at most 10 s, else exit 3. niri
   starts the timer when deskctl asks, so every guarded command takes at
   least that long. In niri 26.04 deskctl's own pointer motion counts as
   activity and its keyboard input does not. On nested the check runs
   only when `--idle-ms` is given (used by the tests).
3. **Stop file.** While `$XDG_RUNTIME_DIR/deskctl/stop` exists, session
   input, accessibility mutations, `action`, and `focus` exit 3. The file
   is checked again after any idle wait and before sending input, including
   between keystrokes and clicks. A pointer button already pressed is
   released. `action` and `focus` use the stop file without an idle wait
   or `--expect` check. Read commands remain available.
4. **No stuck input.** Every pressed key and button is released before
   exit, also on error and SIGINT/SIGTERM.
5. **Activity mark.** Every session command (read-only ones too) writes
   the time in Unix ms to `$XDG_RUNTIME_DIR/deskctl/session.last` when it
   starts and when it ends. A bar can use a fresh mark to indicate activity.
6. **Log.** Every command appends one JSON line to
   `$XDG_STATE_HOME/deskctl/log.jsonl`: time, target, command,
   arguments, result. Typed text is logged as its length only.

The nested target skips checks 1, 3 and 5, and check 2 unless asked.

## Tech stack

Rust (stable 1.99), edition 2024. Crates: `clap` (derive), `anyhow`,
`serde_json`, `wayland-client`, `wayland-protocols`,
`wayland-protocols-wlr`, `wayland-protocols-misc`, `xkbcommon` (keysym
names; uses the system libxkbcommon), `rustix` (keymap memfd, signals,
clock). Tests only: `png` (find markers in screenshots).
Runtime tools: `niri`, `grim`.

## Commands

```
cargo build --release
cargo test                              # unit tests, no display
cargo test -- --ignored --test-threads=1   # end-to-end on a nested niri
cargo clippy --all-targets -- -D warnings
cargo fmt --check
cargo install --locked --path .
mkdir -p ~/.claude/skills
ln -s "$PWD/skills/deskctl" ~/.claude/skills/deskctl
```

## Project structure

```
Cargo.toml
src/main.rs        dispatch, exit codes, logging
src/cli.rs         clap definitions, argument parsing
src/target.rs      nested/session env, state file
src/nested.rs      nested start/stop/status/run
src/niri.rs        niri msg wrapper
src/shot.rs        grim wrapper, mapping JSON
src/wl.rs          Wayland connection, registry, roundtrips
src/keyboard.rs    virtual keyboard; keymap and combo parsing (pure, tested)
src/pointer.rs     virtual pointer; point → output mapping (pure, tested)
src/guard.rs       expect, idle, stop file
src/log.rs         action log
tests/nested.rs    end-to-end tests (#[ignore]), each with its own nested niri
tests/fixtures/    probe.qml: a window that prints keys, clicks, scrolls
skills/deskctl/SKILL.md   agent skill (linked into ~/.claude/skills)
docs/SPEC.md       this file
README.md
```

## Code style

`cargo fmt` defaults. Errors through `anyhow::Result` with context. Pure
logic split from Wayland I/O so it can be unit-tested. No `unwrap` on
I/O paths.

```rust
/// Output that holds the point, and the point relative to it.
pub fn locate(outputs: &[Output], x: f64, y: f64) -> Option<(&Output, f64, f64)> {
    outputs
        .iter()
        .find(|o| o.contains(x, y))
        .map(|o| (o, x - o.x, y - o.y))
}
```

## Testing strategy

- **Unit** (`cargo test`): combo parsing, keymap text for a set of
  keysyms, text → keysym sequence (Unicode, newline, tab), point →
  output, shot mapping, `--expect` matching, argument validation.
- **End-to-end** (`--ignored`): start a nested niri, run
  `tests/fixtures/probe.qml` with `/usr/lib/qt6/bin/qml`, drive it with
  deskctl, compare what the probe printed. Covers type, key, move, click,
  scroll, shot, start/stop.
- **Session**: read-only commands by the agent; input on the session only
  with the user's go-ahead, into a scratch window.

## Boundaries

- Always: target first; release input before exit; guards on the
  session; log; `make`-free plain cargo commands; tests pass before a
  commit.
- Ask first: new crates beyond the list; changes in `~/work/dots`
  (niri window rule, a keybind for the stop file); installing packages;
  any input test on the real session.
- Never: type passwords or secrets; uinput; session input without
  `--expect`; `pkill -f` patterns (stop by PID).

## Success criteria

1. `cargo build`, `cargo test`, `cargo clippy -D warnings`,
   `cargo fmt --check` pass.
2. Nested: `start` returns once both sockets exist; `status` shows them;
   `stop` leaves no niri process from it.
3. Nested: `type 'Hello, мир! @#~'` into the probe gives the same text
   while the nested niri layout is `us,ru` with `ru` active.
4. Nested: `key ctrl+a`, `key Return` reach the probe with the right
   modifiers.
5. Nested: `click --at X,Y` is seen by the probe within 1 logical px;
   `scroll` gives the right direction; `--double` gives a double click.
6. Nested: in a `shot`, a probe square drawn at a known logical point
   is found at the pixel the printed mapping gives (within 1 px at
   scale 1 and 0.5).
7. Session: `shot`, `windows`, `focused` work; `type --expect wrong-app`
   exits 3 and the focused window receives nothing.
8. Session (user check): the idle check waits while the user types; the
   stop file blocks input; the real `us,ru` layout still works after a
   `type`.
9. Killing deskctl during a long `type` leaves no key held in the nested
   probe.
10. A fresh Claude Code session with only the skill runs a GUI app in a
    nested niri, clicks a control, and reads the screenshot.

## Decisions

1. **Window screenshots:** none in v1; use output and region shots.
   (`niri msg action screenshot-window` overwrites the clipboard and
   saves into `~/Pictures/Screenshots`.)
2. **Nested window placement:** a window rule in `~/work/dots` opens the
   nested niri on its own workspace without focus.
3. **Idle check:** 1500 ms idle, 10 s timeout; revisit on real cases.
   Each guarded session command costs at least 1.5 s.
4. **Stop-file keybind:** later, after input on the session lands
   (tracked in `tasks/todo.md`).
5. **Screenshots:** grim; own screencopy only if grim gets in the way.
