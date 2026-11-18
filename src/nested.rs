//! Start, stop and use a nested niri (a niri running as a window).

use std::fs::File;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use rustix::process::{Pid, Signal, kill_process};
use serde_json::{Value, json};

use crate::niri;
use crate::target::{self, Env, Nested};

const DEFAULT_CONFIG: &str = "hotkey-overlay {\n    skip-at-startup\n}\n";

/// `niri.<display>.<pid>.sock` → `<display>`.
pub fn display_from_socket_name(name: &str, pid: u32) -> Option<String> {
    let rest = name.strip_prefix("niri.")?;
    let display = rest.strip_suffix(&format!(".{pid}.sock"))?;
    (!display.is_empty()).then(|| display.to_string())
}

fn find_sockets(runtime: &Path, pid: u32) -> Option<Env> {
    for entry in std::fs::read_dir(runtime).ok()?.flatten() {
        let name = entry.file_name();
        let Some(display) = display_from_socket_name(&name.to_string_lossy(), pid) else {
            continue;
        };
        if runtime.join(&display).exists() {
            return Some(Env {
                wayland_display: display,
                niri_socket: entry.path(),
            });
        }
    }
    None
}

fn log_tail(path: &Path) -> String {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(5)..].join("\n")
}

/// Start a nested niri unless one runs; print its status.
pub fn start(config: Option<&Path>) -> Result<()> {
    if let Some(n) = target::read_nested()? {
        eprintln!("deskctl: a nested niri is already running");
        println!("{}", n.to_json());
        return Ok(());
    }
    let dir = target::state_dir()?;
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let config = match config {
        Some(c) => c.to_path_buf(),
        None => {
            let c = dir.join("nested.kdl");
            std::fs::write(&c, DEFAULT_CONFIG)?;
            c
        }
    };
    let log_path = dir.join("nested.log");
    let log = File::create(&log_path)?;
    let mut child = Command::new("niri")
        .arg("-c")
        .arg(&config)
        .env_remove("NIRI_SOCKET")
        .env("vblank_mode", "0")
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log)
        .process_group(0)
        .spawn()
        .context("starting niri")?;
    let pid = child.id();
    let runtime = target::runtime_dir()?;
    let deadline = Instant::now() + Duration::from_secs(10);
    let env = loop {
        if let Some(env) = find_sockets(&runtime, pid) {
            break env;
        }
        // try_wait, not /proc: an exited child stays a zombie named niri.
        if child.try_wait()?.is_some() {
            bail!(
                "nested niri exited; log {}:\n{}",
                log_path.display(),
                log_tail(&log_path)
            );
        }
        if Instant::now() > deadline {
            let _ = kill_process(Pid::from_raw(pid as i32).context("bad pid")?, Signal::TERM);
            bail!(
                "nested niri made no sockets in 10 s; log {}",
                log_path.display()
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let n = Nested { pid, env };
    std::fs::write(target::nested_state_file()?, n.to_json().to_string())?;
    if let Err(e) = park(pid) {
        eprintln!("deskctl: nested window left where it opened: {e:#}");
    }
    println!("{}", n.to_json());
    Ok(())
}

/// Window id of the nested niri in the host, and the index of the last
/// workspace on its output (niri keeps that one empty).
pub fn park_target(windows: &Value, workspaces: &Value, pid: u32) -> Option<(u64, u64)> {
    let win = windows
        .as_array()?
        .iter()
        .find(|w| w["pid"].as_u64() == Some(pid.into()))?;
    let workspaces = workspaces.as_array()?;
    let current = workspaces
        .iter()
        .find(|ws| ws["id"] == win["workspace_id"])?;
    let last = workspaces
        .iter()
        .filter(|ws| ws["output"] == current["output"])
        .max_by_key(|ws| ws["idx"].as_u64())?;
    if last["id"] == current["id"] {
        return None;
    }
    Some((win["id"].as_u64()?, last["idx"].as_u64()?))
}

/// Move the nested window to a new workspace in the host session without
/// moving the focus there. The host's window rule keeps it from taking
/// focus when it opens.
fn park(pid: u32) -> Result<()> {
    let host = target::env_for(target::Target::Session)?;
    let deadline = Instant::now() + Duration::from_secs(3);
    let (id, idx) = loop {
        let windows = niri::query(&host, "windows")?;
        let workspaces = niri::query(&host, "workspaces")?;
        if let Some(t) = park_target(&windows, &workspaces, pid) {
            break t;
        }
        if Instant::now() > deadline {
            bail!("no window of pid {pid} in the host session");
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let args = [
        "move-window-to-workspace",
        "--window-id",
        &id.to_string(),
        "--focus",
        "false",
        &idx.to_string(),
    ];
    niri::action(&host, &args.map(String::from))
}

pub fn stop() -> Result<()> {
    let state = target::nested_state_file()?;
    let Some(n) = target::read_nested()? else {
        let _ = std::fs::remove_file(&state);
        bail!("no nested niri is running");
    };
    let pid = Pid::from_raw(n.pid as i32).context("bad pid")?;
    kill_process(pid, Signal::TERM).context("stopping nested niri")?;
    let deadline = Instant::now() + Duration::from_secs(5);
    while target::niri_alive(n.pid) {
        if Instant::now() > deadline {
            bail!("nested niri (pid {}) did not exit in 5 s", n.pid);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    std::fs::remove_file(&state)?;
    Ok(())
}

pub fn status() -> Result<()> {
    match target::read_nested()? {
        Some(n) => {
            println!("{}", n.to_json());
            Ok(())
        }
        None => bail!("no nested niri is running"),
    }
}

/// Spawn a command inside the nested niri, detached. Its output goes to
/// a log file next to the state (Qt apps included).
pub fn run(cmd: &[String]) -> Result<()> {
    let env = target::env_for(target::Target::Nested)?;
    let dir = target::state_dir()?;
    let (prog, args) = cmd.split_first().context("no command")?;
    let tmp_log = dir.join(format!("run-{}.log", std::process::id()));
    let log = File::create(&tmp_log)?;
    let child = Command::new(prog)
        .args(args)
        .envs(env.vars())
        // Qt logs to journald when stderr is not a terminal.
        .env("QT_FORCE_STDERR_LOGGING", "1")
        // Export an accessibility tree (deskctl a11y): Qt, Firefox.
        .env("QT_LINUX_ACCESSIBILITY_ALWAYS_ON", "1")
        .env("GNOME_ACCESSIBILITY", "1")
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log)
        .process_group(0)
        .spawn()
        .with_context(|| format!("starting {prog}"))?;
    let log_path: PathBuf = dir.join(format!("run-{}.log", child.id()));
    std::fs::rename(&tmp_log, &log_path)?;
    let out: Value = json!({"pid": child.id(), "log": log_path});
    println!("{out}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn park_on_last_workspace_of_the_same_output() {
        let windows = json!([
            {"id": 7, "pid": 100, "workspace_id": 1},
            {"id": 9, "pid": 555, "workspace_id": 1},
        ]);
        let workspaces = json!([
            {"id": 1, "idx": 1, "output": "eDP-1"},
            {"id": 5, "idx": 2, "output": "eDP-1"},
            {"id": 6, "idx": 3, "output": "eDP-1"},
            {"id": 8, "idx": 9, "output": "HDMI-1"},
        ]);
        assert_eq!(park_target(&windows, &workspaces, 555), Some((9, 3)));
        assert_eq!(park_target(&windows, &workspaces, 1), None);
        let on_last = json!([{"id": 9, "pid": 555, "workspace_id": 6}]);
        assert_eq!(park_target(&on_last, &workspaces, 555), None);
    }

    #[test]
    fn socket_name() {
        assert_eq!(
            display_from_socket_name("niri.wayland-2.1962064.sock", 1962064).as_deref(),
            Some("wayland-2")
        );
        assert_eq!(display_from_socket_name("niri.wayland-2.1.sock", 2), None);
        assert_eq!(display_from_socket_name("wayland-2.lock", 2), None);
        assert_eq!(display_from_socket_name("niri..5.sock", 5), None);
    }
}
