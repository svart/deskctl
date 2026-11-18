# Implementation Plan: deskctl v1

## Overview

Build the `deskctl` CLI from `docs/SPEC.md` in vertical slices. Each slice
works end to end on a nested niri before the next starts. The riskiest
parts (virtual pointer mapping, virtual keyboard keymaps) come right
after the foundation, so a dead end shows early. The real session is
touched only after everything works nested, and input there only with
the user's go-ahead.

## Architecture decisions

- **Target first in argv** (`deskctl nested …`): Claude Code permission
  rules match on a prefix.
- **Shell out to `niri msg -j` and `grim`**, parse JSON untyped: no
  version lock to niri, little code.
- **Own Wayland clients only for input** (virtual keyboard, virtual
  pointer v2, idle notifier): no packaged tool covers the pointer, and
  one binary keeps the safety checks in one place.
- **Pure logic split from I/O** (keymap text, combo parsing, point →
  output, shot mapping, expect matching): unit-tested without a display.
- **End-to-end tests** start their own nested niri and a QML probe
  (`/usr/lib/qt6/bin/qml`) that prints the input it receives; marked
  `#[ignore]`, run serially.

## Dependency graph

```
T1 skeleton (CLI, target env, log)
 ├── T2 nested start/stop/status/run + niri read/action commands
 │    ├── T3 window rule in ~/work/dots (nested opens unfocused)
 │    ├── T4 shot + mapping
 │    └── T5 probe + virtual pointer ──┐
 │         └── T6 virtual keyboard: type ── T7 key combos
 │              T8 shot mapping check with probe (T4 + T5)
 └── T9 session guards: expect, stop file ── T10 idle check
                                             └── T11 session input trial (user)
T12 skill + install + permission rules (after T8; session parts after T11)
```

## Phases

1. **Foundation**: T1–T4. Checkpoint: nested niri can be started,
   read, screenshotted and stopped from deskctl.
2. **Input on nested**: T5–T8. Checkpoint: spec criteria 3–6 and 9 pass
   in `cargo test -- --ignored`. Review with the user.
3. **Session**: T9–T11. Checkpoint: criteria 7–8.
4. **Skill**: T12. Checkpoint: criterion 10.

Tasks with acceptance criteria: `tasks/todo.md`.

## Risks and mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| niri maps `create_virtual_pointer_with_output` absolute motion differently than expected | High | T5 checks clicks within 1 px on nested first; fall back to whole-layout extents if needed |
| A virtual-keyboard keymap disturbs the real layout after `type` | High | T6 tests with `us,ru` in nested; session check (T11) needs the user |
| Keys stay pressed if deskctl dies | Med | Release on every exit path and signal; destroying the device on disconnect; criterion 9 test |
| e2e tests open windows in the user session and take focus | Med | T3 window rule before the input tasks; tests serial |
| Multi-output mapping cannot be tested without a second monitor | Low | Unit-test the math; user check later |
| Own input resets the idle timer: about 1.5 s per session step | Low | Accepted for now; revisit on real use |

## Follow-ups outside this repo (not in v1)

- `~/work/dots`: stop-file keybind (after T9).

# Plan: a11y (docs/SPEC-a11y.md)

Same approach as v1: slices that work end to end on a nested niri,
riskiest first (talking AT-SPI from Rust and scoping apps to a target),
then reading, then acting with guards, then docs.

```
A1 connection, apps, scoping, fixtures (Qt probe controls, GTK probe)
 └── A2 tree, find
      └── A3 act, set-text, focus, guards
           └── A4 skill, README, permission rules
```

| Risk | Mitigation |
|---|---|
| atspi/zbus API differs from what the spec assumes | A1 first: list apps and one node before anything else |
| An app's tree is slow over D-Bus | `--max-nodes`; measure in A2 (criterion 7); Cache interface later |
| Qt reports role names differently from GTK | find matches role case-insensitively; tests cover both toolkits |
