//! `niri msg` with the target's NIRI_SOCKET. JSON stays untyped so a
//! niri update does not break deskctl.

use std::process::Command;

use anyhow::{Context, Result, bail};
use serde_json::Value;

use crate::target::Env;

fn niri(env: &Env, args: &[&str]) -> Result<Vec<u8>> {
    let out = Command::new("niri")
        .arg("msg")
        .args(args)
        .envs(env.vars())
        .output()
        .context("running niri msg")?;
    if !out.status.success() {
        bail!(
            "niri msg {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(out.stdout)
}

/// `niri msg -j <what>`, e.g. `outputs`, `windows`, `focused-window`.
pub fn query(env: &Env, what: &str) -> Result<Value> {
    let out = niri(env, &["-j", what])?;
    serde_json::from_slice(&out).with_context(|| format!("parsing niri msg -j {what}"))
}

/// `niri msg action <args>`.
pub fn action(env: &Env, args: &[String]) -> Result<()> {
    let mut all = vec!["action"];
    all.extend(args.iter().map(String::as_str));
    niri(env, &all).map(drop)
}
