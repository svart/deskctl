mod a11y;
mod cli;
mod guard;
mod keyboard;
mod log;
mod nested;
mod niri;
mod pointer;
mod shot;
mod target;
mod wl;

use std::fmt;
use std::process::ExitCode;

use anyhow::Result;
use clap::Parser;
use serde_json::json;

use cli::{Cli, Cmd, NestedCmd, TargetCmd};
use target::Target;

/// Errors with their own exit code. Anything else exits 1.
#[derive(Debug)]
pub enum Fail {
    /// Bad arguments found after parsing: exit 2.
    Usage(String),
    /// A guard refused: nothing was sent. Exit 3.
    Refused(String),
}

impl fmt::Display for Fail {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Fail::Usage(m) => write!(f, "usage: {m}"),
            Fail::Refused(m) => write!(f, "refused: {m}"),
        }
    }
}

impl std::error::Error for Fail {}

fn exit_code(err: &anyhow::Error) -> u8 {
    match err.downcast_ref::<Fail>() {
        Some(Fail::Usage(_)) => 2,
        Some(Fail::Refused(_)) => 3,
        None => 1,
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let (target, name, args, result) = match &cli.target {
        TargetCmd::Session { cmd } => {
            let (name, args) = cmd.describe();
            guard::mark_session_activity();
            let result = run_common(Target::Session, cmd);
            guard::mark_session_activity();
            (Target::Session, name, args, result)
        }
        TargetCmd::Nested { cmd } => match cmd {
            NestedCmd::Common(c) => {
                let (name, args) = c.describe();
                (Target::Nested, name, args, run_common(Target::Nested, c))
            }
            NestedCmd::Start { config } => (
                Target::Nested,
                "start",
                json!({"config": config}),
                nested::start(config.as_deref()),
            ),
            NestedCmd::Stop => (Target::Nested, "stop", json!({}), nested::stop()),
            NestedCmd::Status => (Target::Nested, "status", json!({}), nested::status()),
            NestedCmd::Run { cmd } => {
                (Target::Nested, "run", json!({"cmd": cmd}), nested::run(cmd))
            }
        },
    };
    let outcome = match &result {
        Ok(()) => "ok".to_string(),
        Err(e) => format!("{e:#}"),
    };
    log::append(&log::line(target.name(), name, args, &outcome));
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("deskctl: {e:#}");
            ExitCode::from(exit_code(&e))
        }
    }
}

fn print_json(v: &serde_json::Value) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(v)?);
    Ok(())
}

fn run_common(target: Target, cmd: &Cmd) -> Result<()> {
    let env = target::env_for(target)?;
    guard::check(target, cmd, &env)?;
    match cmd {
        Cmd::Shot {
            output,
            region,
            scale,
            file,
        } => print_json(&shot::shot(&env, output.as_deref(), *region, *scale, file)?),
        Cmd::Outputs => print_json(&niri::query(&env, "outputs")?),
        Cmd::Windows => print_json(&niri::query(&env, "windows")?),
        Cmd::Workspaces => print_json(&niri::query(&env, "workspaces")?),
        Cmd::Focused => print_json(&niri::query(&env, "focused-window")?),
        Cmd::Layers => print_json(&niri::query(&env, "layers")?),
        Cmd::Action { args } => niri::action(&env, args),
        Cmd::Focus { id } => niri::action(
            &env,
            &["focus-window".into(), "--id".into(), id.to_string()],
        ),
        Cmd::Move { x, y, .. } => {
            pointer::Pointer::new(&env, target)?.move_to(cli::Point { x: *x, y: *y })
        }
        Cmd::Click {
            at, double, button, ..
        } => {
            let mut p = pointer::Pointer::new(&env, target)?;
            if let Some(at) = at {
                p.move_to(*at)?;
            }
            p.click(*button, *double)
        }
        Cmd::Scroll { at, dy, dx, .. } => {
            let mut p = pointer::Pointer::new(&env, target)?;
            if let Some(at) = at {
                p.move_to(*at)?;
            }
            p.scroll(*dy, dx.unwrap_or(0.0))
        }
        Cmd::A11y { cmd } => match cmd {
            cli::A11yCmd::Apps => print_json(&a11y::apps(&env)?),
            cli::A11yCmd::Tree {
                scope,
                depth,
                max_nodes,
                all,
                text_max,
            } => {
                let limits = a11y::Limits {
                    depth: *depth,
                    max_nodes: *max_nodes,
                    all: *all,
                    text_max: *text_max,
                };
                print_json(&a11y::tree(&env, scope, limits)?)
            }
            cli::A11yCmd::Find {
                scope,
                role,
                name,
                name_re,
                state,
                all,
            } => {
                let name_re = name_re
                    .as_deref()
                    .map(regex::Regex::new)
                    .transpose()
                    .map_err(|e| Fail::Usage(format!("--name-re: {e}")))?;
                let sel = a11y::Selector {
                    role: role.clone(),
                    name: name.clone(),
                    name_re,
                    states: state.clone(),
                };
                let limits = a11y::Limits {
                    depth: 40,
                    max_nodes: 5000,
                    all: *all,
                    text_max: 200,
                };
                let tree = a11y::tree(&env, scope, limits)?;
                print_json(&serde_json::Value::Array(a11y::find_in(&tree, &sel)))
            }
            cli::A11yCmd::Act { input, id, action } => print_json(&a11y::act(
                &env,
                target,
                id,
                input.expect.as_ref(),
                a11y::Act::Action(action.as_deref()),
            )?),
            cli::A11yCmd::SetText { input, id, text } => print_json(&a11y::act(
                &env,
                target,
                id,
                input.expect.as_ref(),
                a11y::Act::SetText(text),
            )?),
            cli::A11yCmd::Focus { input, id } => print_json(&a11y::act(
                &env,
                target,
                id,
                input.expect.as_ref(),
                a11y::Act::Focus,
            )?),
        },
        Cmd::Type { text, delay_ms, .. } => keyboard::Keyboard::new(&env, target)?
            .type_text(text, std::time::Duration::from_millis(*delay_ms)),
        Cmd::Key {
            combos, delay_ms, ..
        } => {
            let combos = combos
                .iter()
                .map(|c| keyboard::parse_combo(c).map_err(Fail::Usage))
                .collect::<Result<Vec<_>, _>>()?;
            keyboard::Keyboard::new(&env, target)?
                .press_combos(&combos, std::time::Duration::from_millis(*delay_ms))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_codes() {
        assert_eq!(exit_code(&anyhow::anyhow!("x")), 1);
        assert_eq!(exit_code(&Fail::Usage("x".into()).into()), 2);
        let refused = anyhow::Error::from(Fail::Refused("x".into())).context("while typing");
        assert_eq!(exit_code(&refused), 3);
    }
}
