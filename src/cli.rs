//! Command line: the target comes first, so permission rules can match
//! on a prefix (`deskctl nested ...`).

use std::path::PathBuf;
use std::str::FromStr;

use clap::{Args, Parser, Subcommand, ValueEnum};
use serde_json::{Value, json};

#[derive(Parser, Debug)]
#[command(name = "deskctl", version, about = "See and drive a niri desktop")]
pub struct Cli {
    #[command(subcommand)]
    pub target: TargetCmd,
}

#[derive(Subcommand, Debug)]
pub enum TargetCmd {
    /// A niri started as a window inside the session
    Nested {
        #[command(subcommand)]
        cmd: NestedCmd,
    },
    /// The real session (input is guarded)
    Session {
        #[command(subcommand)]
        cmd: Cmd,
    },
}

#[derive(Subcommand, Debug)]
pub enum NestedCmd {
    /// Start a nested niri and wait for its sockets
    Start {
        /// niri config file (default: a minimal built-in one)
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// Stop the nested niri
    Stop,
    /// Print pid, WAYLAND_DISPLAY and NIRI_SOCKET as JSON
    Status,
    /// Spawn CMD inside the nested niri, detached
    ///
    /// Keeps the current directory and environment, adds the nested
    /// WAYLAND_DISPLAY and NIRI_SOCKET. Prints {"pid", "log"}; the log holds
    /// the app's stdout and stderr (Qt apps included).
    Run {
        #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
        cmd: Vec<String>,
    },
    #[command(flatten)]
    Common(Cmd),
}

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// Screenshot to a PNG file; prints how image pixels map to logical ones
    Shot {
        /// Only this output
        #[arg(long, conflicts_with = "region")]
        output: Option<String>,
        /// Only this region: X,Y,WxH in logical pixels
        #[arg(long)]
        region: Option<Region>,
        /// Image scale (grim -s)
        #[arg(long)]
        scale: Option<f64>,
        file: PathBuf,
    },
    /// Outputs as JSON
    Outputs,
    /// Windows as JSON
    Windows,
    /// Workspaces as JSON
    Workspaces,
    /// Focused window as JSON
    Focused,
    /// Layer-shell surfaces (bars, panels, popups) as JSON
    Layers,
    /// Run a niri action (`niri msg action ...`)
    Action {
        #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Focus a window by id
    Focus { id: u64 },
    /// Type text
    Type {
        #[command(flatten)]
        input: InputOpts,
        /// Pause after each key, so slow apps keep up
        #[arg(long, default_value_t = 5)]
        delay_ms: u64,
        text: String,
    },
    /// Press key combos in order: ctrl+shift+t Return alt+F4
    Key {
        #[command(flatten)]
        input: InputOpts,
        /// Pause after each combo
        #[arg(long, default_value_t = 5)]
        delay_ms: u64,
        #[arg(required = true)]
        combos: Vec<String>,
    },
    /// Move the pointer to X Y (logical pixels)
    Move {
        #[command(flatten)]
        input: InputOpts,
        x: f64,
        y: f64,
    },
    /// Click a button, optionally after moving to --at
    Click {
        #[command(flatten)]
        input: InputOpts,
        #[arg(long)]
        at: Option<Point>,
        #[arg(long)]
        double: bool,
        #[arg(value_enum, default_value_t = Button::Left)]
        button: Button,
    },
    /// Scroll DY (positive: down) and DX (positive: right), in wheel steps
    #[command(allow_negative_numbers = true)]
    Scroll {
        #[command(flatten)]
        input: InputOpts,
        #[arg(long)]
        at: Option<Point>,
        dy: f64,
        dx: Option<f64>,
    },
    /// Accessibility tree (AT-SPI): find UI elements by role and name, act on them
    A11y {
        #[command(subcommand)]
        cmd: A11yCmd,
    },
}

#[derive(Subcommand, Debug)]
pub enum A11yCmd {
    /// Apps of this target that export a tree (and windows whose app does not)
    Apps,
    /// The tree as JSON
    Tree {
        #[command(flatten)]
        scope: A11yScope,
        /// Levels below the app (or window) to include
        #[arg(long, default_value_t = 40)]
        depth: usize,
        /// Stop after this many nodes
        #[arg(long, default_value_t = 2000)]
        max_nodes: usize,
        /// Also nodes that are not showing
        #[arg(long)]
        all: bool,
        /// Characters of text to include per text node
        #[arg(long, default_value_t = 200)]
        text_max: usize,
    },
    /// Nodes that match, as a JSON list
    Find {
        #[command(flatten)]
        scope: A11yScope,
        /// Role, case-insensitive: "button", "text", "password text", "check box"
        #[arg(long)]
        role: Option<String>,
        /// Exact name
        #[arg(long, conflicts_with = "name_re")]
        name: Option<String>,
        /// Name as a regular expression
        #[arg(long)]
        name_re: Option<String>,
        /// A state the node must have ("focused", "checked"); repeatable
        #[arg(long)]
        state: Vec<String>,
        /// Also nodes that are not showing
        #[arg(long)]
        all: bool,
    },
    /// Run a node's action (default: press, click, activate or toggle, else its first)
    Act {
        #[command(flatten)]
        input: InputOpts,
        id: String,
        action: Option<String>,
    },
    /// Replace the text of an editable node
    SetText {
        #[command(flatten)]
        input: InputOpts,
        id: String,
        text: String,
    },
    /// Give a node keyboard focus
    Focus {
        #[command(flatten)]
        input: InputOpts,
        id: String,
    },
}

#[derive(Args, Debug, Clone, Default)]
pub struct A11yScope {
    /// Only this app (process id)
    #[arg(long, conflicts_with = "window")]
    pub app: Option<u32>,
    /// Only the app of this niri window
    #[arg(long)]
    pub window: Option<u64>,
}

#[derive(Args, Debug, Clone)]
pub struct InputOpts {
    /// Focused window must have this app_id (and a title containing TITLE)
    #[arg(long, value_name = "APP[/TITLE]")]
    pub expect: Option<Expect>,
    /// Wait until user input has been idle this long; session default
    /// 1500, nested only when given
    #[arg(long)]
    pub idle_ms: Option<u32>,
}

#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Left,
    Right,
    Middle,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Region {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expect {
    pub app_id: String,
    pub title: Option<String>,
}

fn num(s: &str) -> Result<f64, String> {
    s.trim()
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite())
        .ok_or_else(|| format!("not a number: {s:?}"))
}

impl FromStr for Point {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        let (x, y) = s.split_once(',').ok_or("expected X,Y")?;
        Ok(Point {
            x: num(x)?,
            y: num(y)?,
        })
    }
}

impl FromStr for Region {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        let err = || "expected X,Y,WxH".to_string();
        let mut parts = s.splitn(3, ',');
        let (x, y, wh) = match (parts.next(), parts.next(), parts.next()) {
            (Some(x), Some(y), Some(wh)) => (x, y, wh),
            _ => return Err(err()),
        };
        let (w, h) = wh.split_once('x').ok_or_else(err)?;
        let r = Region {
            x: num(x)?,
            y: num(y)?,
            w: num(w)?,
            h: num(h)?,
        };
        if r.w <= 0.0 || r.h <= 0.0 {
            return Err("width and height must be positive".into());
        }
        Ok(r)
    }
}

impl FromStr for Expect {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        let (app_id, title) = match s.split_once('/') {
            Some((a, t)) => (a, Some(t.to_string()).filter(|t| !t.is_empty())),
            None => (s, None),
        };
        if app_id.is_empty() {
            return Err("APP must not be empty".into());
        }
        Ok(Expect {
            app_id: app_id.to_string(),
            title,
        })
    }
}

impl Cmd {
    /// Name and arguments for the action log. Typed text is logged as its
    /// length only.
    pub fn describe(&self) -> (&'static str, Value) {
        let expect = |i: &InputOpts| {
            i.expect
                .as_ref()
                .map(|e| json!({"app_id": e.app_id, "title": e.title}))
        };
        let pt = |p: &Option<Point>| p.map(|p| json!([p.x, p.y]));
        match self {
            Cmd::Shot {
                output,
                region,
                scale,
                file,
            } => (
                "shot",
                json!({"output": output, "region": region.map(|r| json!([r.x, r.y, r.w, r.h])),
                       "scale": scale, "file": file}),
            ),
            Cmd::Outputs => ("outputs", json!({})),
            Cmd::Windows => ("windows", json!({})),
            Cmd::Workspaces => ("workspaces", json!({})),
            Cmd::Focused => ("focused", json!({})),
            Cmd::Layers => ("layers", json!({})),
            Cmd::Action { args } => ("action", json!({"args": args})),
            Cmd::Focus { id } => ("focus", json!({"id": id})),
            Cmd::Type { input, text, .. } => (
                "type",
                json!({"expect": expect(input), "chars": text.chars().count()}),
            ),
            Cmd::Key { input, combos, .. } => {
                ("key", json!({"expect": expect(input), "combos": combos}))
            }
            Cmd::Move { input, x, y } => ("move", json!({"expect": expect(input), "at": [x, y]})),
            Cmd::Click {
                input,
                at,
                double,
                button,
            } => (
                "click",
                json!({"expect": expect(input), "at": pt(at), "double": double,
                       "button": format!("{button:?}").to_lowercase()}),
            ),
            Cmd::Scroll { input, at, dy, dx } => (
                "scroll",
                json!({"expect": expect(input), "at": pt(at), "dy": dy, "dx": dx}),
            ),
            Cmd::A11y { cmd } => {
                let scope = |s: &A11yScope| json!({"app": s.app, "window": s.window});
                match cmd {
                    A11yCmd::Apps => ("a11y apps", json!({})),
                    A11yCmd::Tree {
                        scope: s,
                        depth,
                        max_nodes,
                        all,
                        ..
                    } => (
                        "a11y tree",
                        json!({"scope": scope(s), "depth": depth, "max_nodes": max_nodes, "all": all}),
                    ),
                    A11yCmd::Find {
                        scope: s,
                        role,
                        name,
                        name_re,
                        state,
                        ..
                    } => (
                        "a11y find",
                        json!({"scope": scope(s), "role": role, "name": name, "name_re": name_re, "state": state}),
                    ),
                    A11yCmd::Act { input, id, action } => (
                        "a11y act",
                        json!({"expect": expect(input), "id": id, "action": action}),
                    ),
                    A11yCmd::SetText { input, id, text } => (
                        "a11y set-text",
                        json!({"expect": expect(input), "id": id, "chars": text.chars().count()}),
                    ),
                    A11yCmd::Focus { input, id } => {
                        ("a11y focus", json!({"expect": expect(input), "id": id}))
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(std::iter::once("deskctl").chain(args.iter().copied()))
    }

    #[test]
    fn cli_is_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn point_and_region() {
        assert_eq!("10,20.5".parse::<Point>(), Ok(Point { x: 10.0, y: 20.5 }));
        assert!("10".parse::<Point>().is_err());
        assert!("a,1".parse::<Point>().is_err());
        assert_eq!(
            "0,10,200x100".parse::<Region>(),
            Ok(Region {
                x: 0.0,
                y: 10.0,
                w: 200.0,
                h: 100.0
            })
        );
        assert!("0,10,200".parse::<Region>().is_err());
        assert!("0,10,0x100".parse::<Region>().is_err());
    }

    #[test]
    fn expect() {
        assert_eq!(
            "Alacritty".parse::<Expect>(),
            Ok(Expect {
                app_id: "Alacritty".into(),
                title: None
            })
        );
        assert_eq!(
            "firefox/Mozilla Firefox".parse::<Expect>(),
            Ok(Expect {
                app_id: "firefox".into(),
                title: Some("Mozilla Firefox".into())
            })
        );
        assert!("/title".parse::<Expect>().is_err());
    }

    #[test]
    fn nested_has_common_and_own_commands() {
        assert!(matches!(
            parse(&["nested", "windows"]).unwrap().target,
            TargetCmd::Nested {
                cmd: NestedCmd::Common(Cmd::Windows)
            }
        ));
        assert!(matches!(
            parse(&["nested", "run", "qs", "-p", "."]).unwrap().target,
            TargetCmd::Nested {
                cmd: NestedCmd::Run { .. }
            }
        ));
        // start/stop/run are nested only
        assert!(parse(&["session", "start"]).is_err());
    }

    #[test]
    fn negative_scroll_and_hyphen_action_args() {
        assert!(parse(&["session", "scroll", "-3"]).is_ok());
        let cli = parse(&["nested", "action", "focus-workspace", "--", "2"]).unwrap();
        let TargetCmd::Nested {
            cmd: NestedCmd::Common(Cmd::Action { args }),
        } = cli.target
        else {
            panic!()
        };
        assert_eq!(args, ["focus-workspace", "--", "2"]);
    }

    #[test]
    fn usage_errors() {
        let err = parse(&[
            "session", "shot", "--output", "a", "--region", "0,0,1x1", "f.png",
        ])
        .unwrap_err();
        assert_eq!(err.exit_code(), 2);
        assert!(parse(&["session", "click", "sideways"]).is_err());
        assert!(parse(&["session", "key"]).is_err());
    }

    #[test]
    fn log_hides_typed_text() {
        let cli = parse(&["session", "type", "--expect", "foot", "secret"]).unwrap();
        let TargetCmd::Session { cmd } = cli.target else {
            panic!()
        };
        let (name, v) = cmd.describe();
        assert_eq!(name, "type");
        assert_eq!(v["chars"], 6);
        assert!(!v.to_string().contains("secret"));
    }
}
