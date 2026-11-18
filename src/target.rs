//! Which desktop a command talks to, and the environment that selects it.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Nested,
    Session,
}

impl Target {
    pub fn name(self) -> &'static str {
        match self {
            Target::Nested => "nested",
            Target::Session => "session",
        }
    }
}

/// Sockets of one niri instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Env {
    pub wayland_display: String,
    pub niri_socket: PathBuf,
}

impl Env {
    /// Environment for child processes (niri msg, grim, apps).
    pub fn vars(&self) -> [(&'static str, &std::ffi::OsStr); 2] {
        [
            ("WAYLAND_DISPLAY", self.wayland_display.as_ref()),
            ("NIRI_SOCKET", self.niri_socket.as_os_str()),
        ]
    }
}

/// State of a nested niri started by deskctl.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Nested {
    pub pid: u32,
    pub env: Env,
}

impl Nested {
    pub fn to_json(&self) -> Value {
        json!({
            "pid": self.pid,
            "wayland_display": self.env.wayland_display,
            "niri_socket": self.env.niri_socket,
        })
    }

    pub fn from_json(v: &Value) -> Option<Nested> {
        Some(Nested {
            pid: u32::try_from(v["pid"].as_u64()?).ok()?,
            env: Env {
                wayland_display: v["wayland_display"].as_str()?.to_string(),
                niri_socket: PathBuf::from(v["niri_socket"].as_str()?),
            },
        })
    }
}

pub fn runtime_dir() -> Result<PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .context("XDG_RUNTIME_DIR is not set")
}

/// `$DESKCTL_STATE_DIR`, else `$XDG_RUNTIME_DIR/deskctl`. Tests set the
/// variable so they do not touch a nested niri the user started.
pub fn state_dir() -> Result<PathBuf> {
    match std::env::var_os("DESKCTL_STATE_DIR") {
        Some(d) => Ok(PathBuf::from(d)),
        None => Ok(runtime_dir()?.join("deskctl")),
    }
}

pub fn nested_state_file() -> Result<PathBuf> {
    Ok(state_dir()?.join("nested.json"))
}

/// The process lives and is a niri (guards against PID reuse).
pub fn niri_alive(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/comm")).is_ok_and(|c| c.trim() == "niri")
}

/// The running nested niri, if its state file exists and its process lives.
pub fn read_nested() -> Result<Option<Nested>> {
    let path = nested_state_file()?;
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    let v: Value =
        serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    let n = Nested::from_json(&v).with_context(|| format!("bad state in {}", path.display()))?;
    Ok(niri_alive(n.pid).then_some(n))
}

pub fn env_for(target: Target) -> Result<Env> {
    match target {
        Target::Session => Ok(Env {
            wayland_display: std::env::var("WAYLAND_DISPLAY")
                .context("WAYLAND_DISPLAY is not set: not in a Wayland session")?,
            niri_socket: std::env::var_os("NIRI_SOCKET")
                .map(PathBuf::from)
                .context("NIRI_SOCKET is not set: not in a niri session")?,
        }),
        Target::Nested => match read_nested()? {
            Some(n) => Ok(n.env),
            None => bail!("no nested niri is running (start one: deskctl nested start)"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_state_round_trip() {
        let n = Nested {
            pid: 42,
            env: Env {
                wayland_display: "wayland-2".into(),
                niri_socket: "/run/user/1000/niri.wayland-2.42.sock".into(),
            },
        };
        assert_eq!(Nested::from_json(&n.to_json()), Some(n));
        assert_eq!(Nested::from_json(&json!({"pid": 1})), None);
    }
}
