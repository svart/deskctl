//! Accessibility tree (AT-SPI) of a target's apps: list apps, read and
//! search their trees, run actions. Spec: docs/SPEC-a11y.md.
//!
//! The AT-SPI bus is shared by every app of the user. An app belongs to
//! the niri whose WAYLAND_DISPLAY is in its environment.

use std::future::Future;
use std::path::Path;
use std::pin::Pin;

use anyhow::{Context, Result};
use atspi::AccessibilityConnection;
use atspi::proxy::accessible::AccessibleProxy;
use atspi::proxy::action::ActionProxy;
use atspi::proxy::application::ApplicationProxy;
use atspi::proxy::component::ComponentProxy;
use atspi::proxy::editable_text::EditableTextProxy;
use atspi::proxy::text::TextProxy;
use atspi::proxy::value::ValueProxy;
use atspi::zbus;
use atspi::zbus::proxy::CacheProperties;
use atspi::{CoordType, Interface, Role};
use serde_json::{Value, json};

use crate::Fail;
use crate::cli::Expect;
use crate::guard;
use crate::niri;
use crate::target::{Env, Target};

/// Value of `key` in a `/proc/<pid>/environ` blob (NUL-separated).
pub fn environ_value(environ: &[u8], key: &str) -> Option<String> {
    let prefix = format!("{key}=");
    environ
        .split(|&b| b == 0)
        .filter_map(|e| std::str::from_utf8(e).ok())
        .find_map(|e| e.strip_prefix(&prefix).map(str::to_string))
}

/// Do two WAYLAND_DISPLAY values name the same socket? A value may be a
/// name in XDG_RUNTIME_DIR ("wayland-1") or an absolute path.
pub fn same_display(a: &str, b: &str) -> bool {
    let base = |s: &str| {
        Path::new(s)
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
    };
    !a.is_empty() && base(a) == base(b)
}

fn display_of(pid: u32) -> Option<String> {
    let environ = std::fs::read(format!("/proc/{pid}/environ")).ok()?;
    environ_value(&environ, "WAYLAND_DISPLAY")
}

/// Does process `pid` run on the target's compositor?
pub fn on_target(pid: u32, env: &Env) -> bool {
    display_of(pid).is_some_and(|d| same_display(&d, &env.wayland_display))
}

/// A node id: its D-Bus bus name and object path, `:1.234/org/a11y/...`.
pub fn node_id(bus: &str, path: &str) -> String {
    format!("{bus}{path}")
}

/// Bus name and path from a node id.
pub fn parse_id(id: &str) -> Option<(&str, &str)> {
    let slash = id.find('/')?;
    let (bus, path) = id.split_at(slash);
    (!bus.is_empty() && path.len() > 1).then_some((bus, path))
}

/// An app on the AT-SPI bus.
#[derive(Debug, Clone)]
pub struct App {
    pub bus: String,
    pub path: String,
    pub pid: u32,
    pub name: String,
    pub toolkit: String,
}

pub struct A11y {
    conn: AccessibilityConnection,
}

impl A11y {
    pub fn connect() -> Result<A11y> {
        let conn = zbus::block_on(AccessibilityConnection::new())
            .context("connecting to the accessibility bus")?;
        Ok(A11y { conn })
    }

    fn zconn(&self) -> &zbus::Connection {
        self.conn.connection()
    }

    pub(crate) async fn accessible(
        &self,
        bus: &str,
        path: &str,
    ) -> Result<AccessibleProxy<'static>> {
        Ok(AccessibleProxy::builder(self.zconn())
            .destination(bus.to_string())?
            .path(path.to_string())?
            .cache_properties(CacheProperties::No)
            .build()
            .await?)
    }

    /// Every app registered on the bus whose process runs on the target.
    pub fn apps(&self, env: &Env) -> Result<Vec<App>> {
        zbus::block_on(async {
            let root = self.conn.root_accessible_on_registry().await?;
            let dbus = zbus::fdo::DBusProxy::new(self.zconn()).await?;
            let mut apps = Vec::new();
            for child in root.get_children().await? {
                let (Some(bus), path) = (child.name_as_str(), child.path_as_str()) else {
                    continue;
                };
                let Ok(name) = zbus::names::BusName::try_from(bus) else {
                    continue;
                };
                let Ok(pid) = dbus.get_connection_unix_process_id(name).await else {
                    continue;
                };
                if !on_target(pid, env) {
                    continue;
                }
                let acc = self.accessible(bus, path).await?;
                let toolkit = match ApplicationProxy::builder(self.zconn())
                    .destination(bus.to_string())?
                    .path(path.to_string())?
                    .cache_properties(CacheProperties::No)
                    .build()
                    .await
                {
                    Ok(p) => p.toolkit_name().await.unwrap_or_default(),
                    Err(_) => String::new(),
                };
                apps.push(App {
                    bus: bus.to_string(),
                    path: path.to_string(),
                    pid,
                    name: acc.name().await.unwrap_or_default(),
                    toolkit,
                });
            }
            Ok(apps)
        })
    }
}

/// Limits for a walk.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub depth: usize,
    pub max_nodes: usize,
    pub all: bool,
    pub text_max: usize,
}

struct Walk<'a> {
    a: &'a A11y,
    limits: Limits,
    count: usize,
    truncated: bool,
}

type BoxFut<'f, T> = Pin<Box<dyn Future<Output = T> + 'f>>;

macro_rules! iface {
    ($proxy:ty, $conn:expr, $bus:expr, $path:expr) => {
        <$proxy>::builder($conn)
            .destination($bus.to_string())?
            .path($path.to_string())?
            .cache_properties(CacheProperties::No)
            .build()
            .await?
    };
}

impl Walk<'_> {
    /// One node as JSON, without children.
    async fn node(&self, bus: &str, path: &str) -> Result<Value> {
        let conn = self.a.zconn();
        let acc = self.a.accessible(bus, path).await?;
        // The numeric role in AT-SPI's own words: the same for every
        // toolkit (GetRoleName gives the toolkit's: Qt calls a password
        // field "text", GTK a "text box").
        let role = match acc.get_role().await {
            Ok(r) => r.name().to_string(),
            Err(_) => acc.get_role_name().await.unwrap_or_default(),
        };
        let name = acc.name().await.unwrap_or_default();
        let states: Vec<String> = acc
            .get_state()
            .await
            .map(|s| s.iter().map(|st| st.to_string()).collect())
            .unwrap_or_default();
        let ifaces = acc.get_interfaces().await.unwrap_or_default();
        let mut actions = Vec::new();
        if ifaces.contains(Interface::Action) {
            let p = iface!(ActionProxy, conn, bus, path);
            actions = p
                .get_actions()
                .await
                .map(|a| a.into_iter().map(|x| x.name).collect())
                .unwrap_or_default();
        }
        let mut text = Value::Null;
        if ifaces.contains(Interface::Text) {
            let p = iface!(TextProxy, conn, bus, path);
            let n = p.character_count().await.unwrap_or(0);
            let end = n.min(i32::try_from(self.limits.text_max).unwrap_or(i32::MAX));
            if let Ok(t) = p.get_text(0, end).await {
                text = json!(t);
            }
        }
        let mut value = Value::Null;
        if ifaces.contains(Interface::Value) {
            let p = iface!(ValueProxy, conn, bus, path);
            if let Ok(v) = p.current_value().await {
                value = json!(v);
            }
        }
        let mut bounds = Value::Null;
        if ifaces.contains(Interface::Component) {
            let p = iface!(ComponentProxy, conn, bus, path);
            if let Ok((x, y, w, h)) = p.get_extents(CoordType::Window).await {
                bounds = json!({"x": x, "y": y, "w": w, "h": h});
            }
        }
        Ok(
            json!({"id": node_id(bus, path), "role": role, "name": name, "states": states,
                  "actions": actions, "text": text, "value": value, "bounds": bounds}),
        )
    }

    /// The node and its children. `None` when it is left out (not
    /// showing, or a limit was reached).
    fn walk<'s>(
        &'s mut self,
        bus: String,
        path: String,
        depth: usize,
    ) -> BoxFut<'s, Result<Option<Value>>> {
        Box::pin(async move {
            if self.count >= self.limits.max_nodes {
                self.truncated = true;
                return Ok(None);
            }
            let mut node = match self.node(&bus, &path).await {
                Ok(n) => n,
                Err(_) => return Ok(None), // gone meanwhile
            };
            if depth > 0 && !self.limits.all && !has_state(&node, "showing") {
                return Ok(None);
            }
            self.count += 1;
            let mut children = Vec::new();
            if depth < self.limits.depth {
                let acc = self.a.accessible(&bus, &path).await?;
                for c in acc.get_children().await.unwrap_or_default() {
                    let (Some(b), p) = (c.name_as_str(), c.path_as_str()) else {
                        continue;
                    };
                    if let Some(child) = self.walk(b.to_string(), p.to_string(), depth + 1).await? {
                        children.push(child);
                    }
                }
            } else if self
                .a
                .accessible(&bus, &path)
                .await?
                .child_count()
                .await
                .unwrap_or(0)
                > 0
            {
                self.truncated = true;
            }
            node["children"] = Value::Array(children);
            Ok(Some(node))
        })
    }
}

/// The trees of the target's apps, as `{"truncated", "apps": [...]}`.
pub fn tree(env: &Env, scope: &crate::cli::A11yScope, limits: Limits) -> Result<Value> {
    let a = A11y::connect()?;
    let apps = scoped_apps(&a, env, scope)?;
    zbus::block_on(async {
        let mut w = Walk {
            a: &a,
            limits,
            count: 0,
            truncated: false,
        };
        let mut out = Vec::new();
        for app in &apps {
            if let Some(mut node) = w.walk(app.bus.clone(), app.path.clone(), 0).await? {
                node["pid"] = json!(app.pid);
                node["toolkit"] = json!(app.toolkit);
                out.push(node);
            }
        }
        Ok(json!({"truncated": w.truncated, "apps": out}))
    })
}

fn scoped_apps(a: &A11y, env: &Env, scope: &crate::cli::A11yScope) -> Result<Vec<App>> {
    let mut apps = a.apps(env)?;
    let pid = match (scope.app, scope.window) {
        (Some(pid), _) => Some(pid),
        (None, Some(win)) => {
            let windows = niri::query(env, "windows")?;
            let w = windows
                .as_array()
                .and_then(|ws| ws.iter().find(|w| w["id"].as_u64() == Some(win)))
                .with_context(|| format!("no window {win} on this target"))?;
            Some(
                w["pid"]
                    .as_u64()
                    .and_then(|p| u32::try_from(p).ok())
                    .context("window has no pid")?,
            )
        }
        (None, None) => None,
    };
    if let Some(pid) = pid {
        apps.retain(|a| a.pid == pid);
        anyhow::ensure!(
            !apps.is_empty(),
            "process {pid} exports no accessibility tree on this target"
        );
    }
    Ok(apps)
}

pub fn has_state(node: &Value, state: &str) -> bool {
    node["states"]
        .as_array()
        .is_some_and(|s| s.iter().any(|x| x == state))
}

/// What `find` looks for.
#[derive(Debug, Default)]
pub struct Selector {
    pub role: Option<String>,
    pub name: Option<String>,
    pub name_re: Option<regex::Regex>,
    pub states: Vec<String>,
}

impl Selector {
    pub fn matches(&self, node: &Value) -> bool {
        let role = node["role"].as_str().unwrap_or("");
        let name = node["name"].as_str().unwrap_or("");
        self.role
            .as_ref()
            .is_none_or(|r| r.eq_ignore_ascii_case(role))
            && self.name.as_ref().is_none_or(|n| n == name)
            && self.name_re.as_ref().is_none_or(|re| re.is_match(name))
            && self.states.iter().all(|s| has_state(node, s))
    }
}

/// Matching nodes of a tree (`tree()` output), without their children;
/// each gets the pid of its app.
pub fn find_in(tree: &Value, sel: &Selector) -> Vec<Value> {
    fn visit(node: &Value, pid: &Value, sel: &Selector, out: &mut Vec<Value>) {
        if sel.matches(node) {
            let mut n = node.clone();
            if let Some(o) = n.as_object_mut() {
                o.remove("children");
                o.insert("pid".into(), pid.clone());
            }
            out.push(n);
        }
        for c in node["children"].as_array().into_iter().flatten() {
            visit(c, pid, sel, out);
        }
    }
    let mut out = Vec::new();
    for app in tree["apps"].as_array().into_iter().flatten() {
        visit(app, &app["pid"], sel, &mut out);
    }
    out
}

/// What to do with a node.
#[derive(Debug, Clone, Copy)]
pub enum Act<'a> {
    /// Run an action by name (case-insensitive), or the first one.
    Action(Option<&'a str>),
    SetText(&'a str),
    Focus,
}

/// Actions that do what a click does, in order of preference. Qt lists
/// "Set Focus" first for some roles (page tab, combo box).
const PRIMARY_ACTIONS: [&str; 4] = ["press", "click", "activate", "toggle"];

/// Index of the action to run: by name; without a name the primary one
/// (press, click, activate, toggle), else the first. Err lists the
/// node's actions.
pub fn action_index(actions: &[String], wanted: Option<&str>) -> Result<i32, String> {
    let find = |w: &str| actions.iter().position(|a| a.eq_ignore_ascii_case(w));
    let i = match wanted {
        None => PRIMARY_ACTIONS
            .iter()
            .find_map(|w| find(w))
            .or((!actions.is_empty()).then_some(0)),
        Some(w) => find(w),
    };
    i.map(|i| i32::try_from(i).unwrap_or(0))
        .ok_or_else(|| match wanted {
            Some(w) => format!("no action {w:?}; the node has {actions:?}"),
            None => "the node has no actions".to_string(),
        })
}

/// Run `what` on node `id`, after checking that its app runs on this
/// target and, when given, that `expect` matches one of its windows.
pub fn act(
    env: &Env,
    target: Target,
    id: &str,
    expect: Option<&Expect>,
    what: Act,
) -> Result<Value> {
    let (bus, path) = parse_id(id).ok_or_else(|| Fail::Usage(format!("not a node id: {id:?}")))?;
    let a = A11y::connect()?;
    let conn = a.zconn();
    zbus::block_on(async {
        let dbus = zbus::fdo::DBusProxy::new(conn).await?;
        let name = zbus::names::BusName::try_from(bus)
            .map_err(|e| Fail::Usage(format!("bad bus name in id: {e}")))?;
        let pid = dbus
            .get_connection_unix_process_id(name)
            .await
            .with_context(|| format!("no app owns {bus} any more (node {id})"))?;
        if !on_target(pid, env) {
            return Err(Fail::Refused(format!(
                "node {id} belongs to an app (pid {pid}) on another desktop than the {} target",
                target.name()
            ))
            .into());
        }
        if let Some(expect) = expect {
            let windows = niri::query(env, "windows")?;
            let mine: Vec<Value> = windows
                .as_array()
                .into_iter()
                .flatten()
                .filter(|w| w["pid"].as_u64() == Some(pid.into()))
                .cloned()
                .collect();
            guard::check_expect_any(expect, &mine).map_err(Fail::Refused)?;
        }
        let acc = a.accessible(bus, path).await?;
        let role = acc
            .get_role()
            .await
            .with_context(|| format!("node {id} no longer exists"))?;
        let ifaces = acc.get_interfaces().await.unwrap_or_default();
        let done = match what {
            Act::Action(wanted) => {
                anyhow::ensure!(
                    ifaces.contains(Interface::Action),
                    "node {id} has no actions"
                );
                let p = iface!(ActionProxy, conn, bus, path);
                let names: Vec<String> =
                    p.get_actions().await?.into_iter().map(|x| x.name).collect();
                let i = action_index(&names, wanted).map_err(Fail::Usage)?;
                guard::check_stop(target)?;
                anyhow::ensure!(
                    p.do_action(i).await?,
                    "the app did not run {:?}",
                    names[i as usize]
                );
                json!({"id": id, "action": names[i as usize]})
            }
            Act::SetText(text) => {
                if role == Role::PasswordText {
                    return Err(Fail::Refused(format!("node {id} is a password field")).into());
                }
                anyhow::ensure!(
                    ifaces.contains(Interface::EditableText),
                    "node {id} is not editable text"
                );
                let p = iface!(EditableTextProxy, conn, bus, path);
                guard::check_stop(target)?;
                anyhow::ensure!(
                    p.set_text_contents(text).await?,
                    "the app did not take the text"
                );
                json!({"id": id, "set_text": text.chars().count()})
            }
            Act::Focus => {
                anyhow::ensure!(
                    ifaces.contains(Interface::Component),
                    "node {id} cannot take focus"
                );
                let p = iface!(ComponentProxy, conn, bus, path);
                guard::check_stop(target)?;
                let moved = match p.grab_focus().await {
                    Ok(m) => m,
                    Err(zbus::Error::MethodError(name, _, _))
                        if name.as_str().ends_with("NotSupported") =>
                    {
                        anyhow::bail!(
                            "this app cannot move focus over AT-SPI (GTK 4 cannot); click the node instead"
                        )
                    }
                    Err(e) => return Err(e.into()),
                };
                anyhow::ensure!(moved, "the app did not move the focus");
                json!({"id": id, "focus": true})
            }
        };
        Ok(done)
    })
}

/// Windows (niri `windows` JSON) of a process: their ids.
pub fn window_ids(windows: &Value, pid: u32) -> Vec<u64> {
    windows
        .as_array()
        .map(|ws| {
            ws.iter()
                .filter(|w| w["pid"].as_u64() == Some(pid.into()))
                .filter_map(|w| w["id"].as_u64())
                .collect()
        })
        .unwrap_or_default()
}

const EXPORT_HINT: &str = "no tree: Qt needs QT_LINUX_ACCESSIBILITY_ALWAYS_ON=1, Firefox \
GNOME_ACCESSIBILITY=1 (nested run sets both), Chromium and Electron \
--force-renderer-accessibility";

/// `a11y apps`: the target's apps, then windows whose app exports nothing.
pub fn apps_json(apps: &[App], windows: &Value) -> Value {
    let mut out: Vec<Value> = apps
        .iter()
        .map(|a| {
            json!({"pid": a.pid, "name": a.name, "toolkit": a.toolkit, "exports": true,
                   "id": node_id(&a.bus, &a.path), "windows": window_ids(windows, a.pid)})
        })
        .collect();
    for w in windows.as_array().into_iter().flatten() {
        let Some(pid) = w["pid"].as_u64() else {
            continue;
        };
        if apps.iter().any(|a| u64::from(a.pid) == pid) {
            continue;
        }
        out.push(
            json!({"pid": pid, "app_id": w["app_id"], "title": w["title"],
                        "windows": [w["id"]], "exports": false, "hint": EXPORT_HINT}),
        );
    }
    Value::Array(out)
}

pub fn apps(env: &Env) -> Result<Value> {
    let a = A11y::connect()?;
    let apps = a.apps(env)?;
    let windows = niri::query(env, "windows")?;
    Ok(apps_json(&apps, &windows))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn environ() {
        let blob = b"HOME=/home/u\0WAYLAND_DISPLAY=wayland-2\0XDG_X=1\0";
        assert_eq!(
            environ_value(blob, "WAYLAND_DISPLAY").as_deref(),
            Some("wayland-2")
        );
        assert_eq!(environ_value(blob, "WAYLAND"), None);
        assert_eq!(environ_value(b"", "HOME"), None);
    }

    #[test]
    fn ids() {
        let id = node_id(":1.234", "/org/a11y/atspi/accessible/42");
        assert_eq!(id, ":1.234/org/a11y/atspi/accessible/42");
        assert_eq!(
            parse_id(&id),
            Some((":1.234", "/org/a11y/atspi/accessible/42"))
        );
        assert_eq!(parse_id("/org/a11y"), None);
        assert_eq!(parse_id(":1.2"), None);
        assert_eq!(parse_id(":1.2/"), None);
    }

    #[test]
    fn choose_action() {
        let acts = vec!["Press".to_string(), "Set Focus".to_string()];
        assert_eq!(action_index(&acts, None), Ok(0));
        let qt_tab = vec!["Set Focus".to_string(), "Press".to_string()];
        assert_eq!(
            action_index(&qt_tab, None),
            Ok(1),
            "primary action, not Set Focus"
        );
        let other = vec!["Set Focus".to_string(), "Jump".to_string()];
        assert_eq!(action_index(&other, None), Ok(0), "else the first");
        assert_eq!(action_index(&acts, Some("set focus")), Ok(1));
        assert!(
            action_index(&acts, Some("Toggle"))
                .unwrap_err()
                .contains("Press")
        );
        assert!(action_index(&[], None).is_err());
    }

    #[test]
    fn displays() {
        assert!(same_display("wayland-1", "wayland-1"));
        assert!(same_display("/run/user/1000/wayland-2", "wayland-2"));
        assert!(!same_display("wayland-1", "wayland-2"));
        assert!(!same_display("", ""));
    }

    fn tree_fixture() -> Value {
        json!({"truncated": false, "apps": [{
            "id": ":1.5/root", "role": "application", "name": "probe", "states": [], "pid": 10,
            "children": [{
                "id": ":1.5/1", "role": "frame", "name": "deskctl-probe", "states": ["showing"],
                "children": [
                    {"id": ":1.5/2", "role": "push button", "name": "Increment",
                     "states": ["focusable", "showing"], "children": []},
                    {"id": ":1.5/3", "role": "text", "name": "Name",
                     "states": ["editable", "focused", "showing"], "children": []},
                ]
            }]
        }]})
    }

    #[test]
    fn find_by_role_name_state() {
        let t = tree_fixture();
        let sel = Selector {
            role: Some("Push Button".into()),
            name: Some("Increment".into()),
            ..Default::default()
        };
        let found = find_in(&t, &sel);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0]["id"], ":1.5/2");
        assert_eq!(found[0]["pid"], 10);
        assert!(found[0].get("children").is_none());

        let sel = Selector {
            states: vec!["focused".into()],
            ..Default::default()
        };
        assert_eq!(find_in(&t, &sel)[0]["name"], "Name");

        let sel = Selector {
            name_re: Some(regex::Regex::new("^(Incr|Na)").unwrap()),
            ..Default::default()
        };
        assert_eq!(find_in(&t, &sel).len(), 2);

        let sel = Selector {
            role: Some("button".into()),
            ..Default::default()
        };
        assert!(
            find_in(&t, &sel).is_empty(),
            "role matches whole names only"
        );
        assert_eq!(
            find_in(&t, &Selector::default()).len(),
            4,
            "empty selector matches all"
        );
    }

    #[test]
    fn apps_and_windows() {
        let apps = vec![App {
            bus: ":1.5".into(),
            path: "/org/a11y/atspi/accessible/root".into(),
            pid: 10,
            name: "probe".into(),
            toolkit: "GTK".into(),
        }];
        let windows = json!([
            {"id": 3, "pid": 10, "app_id": "org.deskctl.ProbeGtk", "title": "p"},
            {"id": 4, "pid": 20, "app_id": "org.qt-project.qml", "title": "q"},
        ]);
        let out = apps_json(&apps, &windows);
        assert_eq!(out[0]["windows"], json!([3]));
        assert_eq!(out[0]["exports"], true);
        assert_eq!(out[1]["pid"], 20);
        assert_eq!(out[1]["exports"], false);
        assert_eq!(out.as_array().unwrap().len(), 2);
    }
}
