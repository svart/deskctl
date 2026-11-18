use std::fs;
use std::path::PathBuf;
use std::process::Command;

#[test]
fn stop_file_blocks_session_mutations_before_desktop_access() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("session-guard-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let stop = dir.join("stop");
    fs::write(&stop, "").unwrap();

    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_deskctl"))
            .arg("session")
            .args(args)
            .env("DESKCTL_STATE_DIR", &dir)
            .env("XDG_RUNTIME_DIR", &dir)
            .env("XDG_STATE_HOME", &dir)
            .env("NIRI_SOCKET", dir.join("missing-niri.sock"))
            .env("WAYLAND_DISPLAY", "missing-wayland")
            .env("PATH", &dir)
            .output()
            .unwrap()
    };

    for args in [
        &["action", "focus-workspace", "1"][..],
        &["focus", "1"],
        &["type", "--expect", "fixture", "text"],
        &["key", "--expect", "fixture", "Return"],
        &["move", "1", "2"],
        &["click", "--expect", "fixture"],
        &["scroll", "--expect", "fixture", "1"],
        &["a11y", "act", "--expect", "fixture", ":1.2/node"],
        &[
            "a11y",
            "set-text",
            "--expect",
            "fixture",
            ":1.2/node",
            "text",
        ],
        &["a11y", "focus", "--expect", "fixture", ":1.2/node"],
    ] {
        let out = run(args);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(3), "{args:?}: {stderr}");
        assert!(stderr.contains("stop exists"), "{args:?}: {stderr}");
    }

    // Read commands still reach the desktop lookup (niri is absent from PATH).
    let out = run(&["windows"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("running niri msg"));

    fs::remove_file(stop).unwrap();
    let out = run(&["focus", "1"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("running niri msg"));
    fs::remove_dir_all(&dir).unwrap();
}
