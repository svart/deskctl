# deskctl

See and drive a niri Wayland desktop from the command line: screenshots,
windows, niri actions, typing, keys, pointer. Made for coding agents;
works for people too.

```sh
deskctl nested start                 # niri as a window, isolated
deskctl nested run -- foot           # or another installed Wayland app
deskctl nested windows
deskctl nested stop
```

`nested start` moves the new window to an empty workspace of the session
without moving the focus. So that it does not take focus when it opens,
the session's niri config needs:

```kdl
window-rule {
    match app-id=r#"^niri$"#
    open-focused false
    default-column-width { proportion 1.0; }
}
```

Accessibility trees (AT-SPI), like Android's UiAutomator: find buttons
and fields by role and name and run their actions, no coordinates
needed ([docs/SPEC-a11y.md](docs/SPEC-a11y.md)):

```sh
deskctl nested a11y find --role button --name OK
deskctl nested a11y act ':1.120/org/a11y/atspi/accessible/42'
```

Two targets: `nested` (a niri started by `deskctl nested start`) and
`session` (the real session; input there is guarded). What and why:
[docs/SPEC.md](docs/SPEC.md).

## Install

Deskctl runs in a niri session on Linux and links to `libxkbcommon`.
The tested compositor version is niri 26.04.

| Feature | Additional runtime requirement |
| --- | --- |
| Screenshots (`shot`) | `grim` |
| Accessibility (`a11y`) | An AT-SPI bus in your D-Bus session |

## Build and test

```sh
cargo build --release --locked
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo fmt --check
```

### Optional GUI integration tests

The full GUI suite opens nested windows in a running niri session and uses
`grim` and an AT-SPI bus. Its test apps use Qt 6 with Qt Quick Controls
(`/usr/lib/qt6/bin/qml`) and Python 3 with PyGObject and GTK 4.

```sh
cargo test --locked -- --ignored --test-threads=1
```

## Files

| Path | What |
|---|---|
| `$XDG_RUNTIME_DIR/deskctl/nested.json` | running nested niri: pid, sockets |
| `$XDG_RUNTIME_DIR/deskctl/nested.log` | its output |
| `$XDG_RUNTIME_DIR/deskctl/run-<pid>.log` | output of `nested run` commands |
| `$XDG_RUNTIME_DIR/deskctl/stop` | blocks session input and compositor actions |
| `$XDG_RUNTIME_DIR/deskctl/session.last` | Unix ms of the last session command (for a bar indicator) |
| `$XDG_STATE_HOME/deskctl/log.jsonl` | one line per command |

`nested run` sets `QT_FORCE_STDERR_LOGGING=1`: Qt apps otherwise log to
journald when stderr is not a terminal.

`DESKCTL_STATE_DIR` replaces `$XDG_RUNTIME_DIR/deskctl` (the tests use it).

## License

[MIT](LICENSE)
