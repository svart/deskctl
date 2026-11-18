//! One JSON line per command in `$XDG_STATE_HOME/deskctl/log.jsonl`.

use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

pub fn path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))?;
    Some(base.join("deskctl/log.jsonl"))
}

pub fn line(target: &str, command: &str, args: Value, result: &str) -> String {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    json!({"ts_ms": ms, "target": target, "command": command, "args": args, "result": result})
        .to_string()
}

/// Append a line. A log failure is reported but does not fail the command.
pub fn append(line: &str) {
    let Some(path) = path() else { return };
    let res = (|| {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        writeln!(f, "{line}")
    })();
    if let Err(e) = res {
        eprintln!("deskctl: cannot write log {}: {e}", path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_format() {
        let l = line("nested", "click", json!({"at": [1.0, 2.0]}), "ok");
        let v: Value = serde_json::from_str(&l).unwrap();
        assert_eq!(v["target"], "nested");
        assert_eq!(v["command"], "click");
        assert_eq!(v["args"]["at"][1], 2.0);
        assert_eq!(v["result"], "ok");
        assert!(v["ts_ms"].as_u64().unwrap() > 0);
        assert!(!l.contains('\n'));
    }
}
