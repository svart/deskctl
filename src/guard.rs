//! Checks before input on the real session. Order: arguments, stop file,
//! user idle, then the focused window. Input handlers recheck before sending.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde_json::Value;
use wayland_client::Proxy;
use wayland_client::protocol::wl_seat::WlSeat;
use wayland_protocols::ext::idle_notify::v1::client::ext_idle_notifier_v1::ExtIdleNotifierV1;

use crate::Fail;
use crate::cli::{A11yCmd, Cmd, Expect, InputOpts};
use crate::niri;
use crate::target::{self, Env, Target};
use crate::wl::Wl;

/// Session default for `--idle-ms`.
pub const SESSION_IDLE_MS: u32 = 1500;
/// How long to wait for the user to stop.
const IDLE_WAIT: Duration = Duration::from_secs(10);

/// Wait until the seat's input has been idle for `idle_ms`. deskctl's own
/// input counts too. The compositor starts the timer when asked, so this
/// takes at least `idle_ms`.
pub fn wait_idle(env: &Env, idle_ms: u32) -> Result<()> {
    let mut wl = Wl::connect(env)?;
    let notifier = wl.bind::<ExtIdleNotifierV1>(1..=2)?;
    let seat = wl.bind::<WlSeat>(1..=7)?;
    let n = if notifier.version() >= 2 {
        notifier.get_input_idle_notification(idle_ms, &seat, &wl.qh, ())
    } else {
        notifier.get_idle_notification(idle_ms, &seat, &wl.qh, ())
    };
    let deadline = Instant::now() + IDLE_WAIT + Duration::from_millis(idle_ms.into());
    let result = loop {
        if wl.state.idled {
            break Ok(());
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break Err(Fail::Refused(format!(
                "user input did not pause for {idle_ms} ms within {} s",
                IDLE_WAIT.as_secs()
            ))
            .into());
        }
        wl.dispatch_timeout(left)?;
    };
    n.destroy();
    let _ = wl.sync();
    result
}

/// While this file exists, all input on the session is refused.
pub fn check_stop(target: Target) -> Result<()> {
    if target == Target::Nested {
        return Ok(());
    }
    let path = target::state_dir()?.join("stop");
    match std::fs::symlink_metadata(&path) {
        Ok(_) => Err(Fail::Refused(format!("{} exists", path.display())).into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).with_context(|| format!("checking stop file {}", path.display())),
    }
}

/// Holds the time (Unix ms) of the last command on the session, so a bar
/// can show that automation is driving the desktop.
pub fn session_mark_file() -> Result<PathBuf> {
    Ok(target::state_dir()?.join("session.last"))
}

/// Write the session mark. A failure is reported but does not stop the
/// command.
pub fn mark_session_activity() {
    let res = (|| -> Result<()> {
        let path = session_mark_file()?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_millis();
        std::fs::write(&path, format!("{ms}\n"))?;
        Ok(())
    })();
    if let Err(e) = res {
        eprintln!("deskctl: cannot write the session mark: {e:#}");
    }
}

/// Input options of an input command, and whether `--expect` is required.
pub fn input_of(cmd: &Cmd) -> Option<(&InputOpts, bool)> {
    match cmd {
        Cmd::Type { input, .. }
        | Cmd::Key { input, .. }
        | Cmd::Click { input, .. }
        | Cmd::Scroll { input, .. } => Some((input, true)),
        Cmd::Move { input, .. } => Some((input, false)),
        Cmd::A11y {
            cmd:
                A11yCmd::Act { input, .. }
                | A11yCmd::SetText { input, .. }
                | A11yCmd::Focus { input, .. },
        } => Some((input, true)),
        _ => None,
    }
}

/// Accessibility actions check `--expect` against the window of the
/// node's app (they need no focus), in `a11y`, after the checks here.
fn expect_is_focused_window(cmd: &Cmd) -> bool {
    !matches!(cmd, Cmd::A11y { .. })
}

/// Does one of these windows (niri `windows` entries) match?
pub fn check_expect_any(expect: &Expect, windows: &[Value]) -> Result<(), String> {
    if windows.is_empty() {
        return Err(format!(
            "the app has no window (expected {})",
            expect.app_id
        ));
    }
    let mut last = String::new();
    for w in windows {
        match check_expect(expect, w) {
            Ok(()) => return Ok(()),
            Err(e) => last = e.replace("focused window", "the app's window"),
        }
    }
    Err(last)
}

/// Does the focused window (`niri msg -j focused-window`) match?
pub fn check_expect(expect: &Expect, focused: &Value) -> Result<(), String> {
    if focused.is_null() {
        return Err(format!("no window has focus (expected {})", expect.app_id));
    }
    let app = focused["app_id"].as_str().unwrap_or("");
    let title = focused["title"].as_str().unwrap_or("");
    if app != expect.app_id {
        return Err(format!(
            "focused window is {app:?} ({title:?}), expected {:?}",
            expect.app_id
        ));
    }
    if let Some(t) = &expect.title
        && !title.contains(t.as_str())
    {
        return Err(format!(
            "focused window title is {title:?}, expected it to contain {t:?}"
        ));
    }
    Ok(())
}

pub fn check(target: Target, cmd: &Cmd, env: &Env) -> Result<()> {
    let Some((input, needs_expect)) = input_of(cmd) else {
        return match cmd {
            Cmd::Action { .. } | Cmd::Focus { .. } => check_stop(target),
            _ => Ok(()),
        };
    };
    if target == Target::Nested {
        // Only on request (and for tests): nobody types there.
        if let Some(ms) = input.idle_ms {
            wait_idle(env, ms)?;
        }
        return Ok(());
    }
    if needs_expect && input.expect.is_none() {
        return Err(Fail::Usage("input on the session needs --expect APP[/TITLE]".into()).into());
    }
    check_stop(target)?;
    wait_idle(env, input.idle_ms.unwrap_or(SESSION_IDLE_MS))?;
    check_stop(target)?;
    if let Some(expect) = input
        .expect
        .as_ref()
        .filter(|_| expect_is_focused_window(cmd))
    {
        let focused = niri::query(env, "focused-window")?;
        check_expect(expect, &focused).map_err(Fail::Refused)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn exp(s: &str) -> Expect {
        s.parse().unwrap()
    }

    #[test]
    fn expect_matching() {
        let w = json!({"app_id": "Alacritty", "title": "svart@laptop: ~/work"});
        assert!(check_expect(&exp("Alacritty"), &w).is_ok());
        assert!(check_expect(&exp("Alacritty/~/work"), &w).is_ok());
        assert!(check_expect(&exp("alacritty"), &w).is_err());
        assert!(check_expect(&exp("Alacritty/firefox"), &w).is_err());
        assert!(check_expect(&exp("Alacritty"), &Value::Null).is_err());
    }

    #[test]
    fn which_commands_need_expect() {
        use crate::cli::Cli;
        use clap::Parser;
        let cmd = |args: &[&str]| {
            let cli = Cli::try_parse_from(["deskctl", "session"].iter().chain(args)).unwrap();
            match cli.target {
                crate::cli::TargetCmd::Session { cmd } => cmd,
                _ => unreachable!(),
            }
        };
        assert_eq!(input_of(&cmd(&["type", "x"])).map(|i| i.1), Some(true));
        assert_eq!(input_of(&cmd(&["key", "Return"])).map(|i| i.1), Some(true));
        assert_eq!(input_of(&cmd(&["click"])).map(|i| i.1), Some(true));
        assert_eq!(input_of(&cmd(&["scroll", "1"])).map(|i| i.1), Some(true));
        assert_eq!(
            input_of(&cmd(&["move", "1", "2"])).map(|i| i.1),
            Some(false)
        );
        assert!(input_of(&cmd(&["windows"])).is_none());
        // Accessibility actions go through the same guards; reading does not.
        assert_eq!(
            input_of(&cmd(&["a11y", "act", ":1.2/x"])).map(|i| i.1),
            Some(true)
        );
        assert_eq!(
            input_of(&cmd(&["a11y", "set-text", ":1.2/x", "t"])).map(|i| i.1),
            Some(true)
        );
        assert_eq!(
            input_of(&cmd(&["a11y", "focus", ":1.2/x"])).map(|i| i.1),
            Some(true)
        );
        assert!(input_of(&cmd(&["a11y", "tree"])).is_none());
        assert!(input_of(&cmd(&["a11y", "find", "--role", "button"])).is_none());
    }
}
