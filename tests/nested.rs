//! End-to-end tests on a nested niri. They open windows in the running
//! session, so they are ignored by default:
//! `cargo test -- --ignored --test-threads=1`

use std::path::PathBuf;
use std::process::{Command, Output};
use std::time::{Duration, Instant};

use serde_json::Value;

/// A nested niri with its own state dir; stopped on drop.
struct Nested {
    dir: PathBuf,
    pub info: Value,
}

impl Nested {
    fn start(name: &str, config: Option<&str>) -> Nested {
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut n = Nested {
            dir,
            info: Value::Null,
        };
        let mut args = vec!["nested", "start"];
        let cfg_path;
        if let Some(c) = config {
            cfg_path = n.dir.join("test.kdl");
            std::fs::write(&cfg_path, c).unwrap();
            args.extend(["--config", cfg_path.to_str().unwrap()]);
        }
        n.info = json_out(&n.ok(&args));
        n
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_deskctl"))
            .args(args)
            .env("DESKCTL_STATE_DIR", &self.dir)
            .output()
            .unwrap()
    }

    fn ok(&self, args: &[&str]) -> Output {
        let out = self.run(args);
        assert!(
            out.status.success(),
            "deskctl {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        out
    }

    fn pid(&self) -> u32 {
        self.info["pid"].as_u64().unwrap() as u32
    }
}

impl Drop for Nested {
    fn drop(&mut self) {
        let _ = self.run(&["nested", "stop"]);
    }
}

fn json_out(out: &Output) -> Value {
    serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("bad JSON ({e}): {}", String::from_utf8_lossy(&out.stdout)))
}

fn wait_for<T>(what: &str, mut f: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(v) = f() {
            return v;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
#[ignore]
fn lifecycle() {
    let n = Nested::start("lifecycle", None);
    assert!(
        n.info["wayland_display"]
            .as_str()
            .unwrap()
            .starts_with("wayland-")
    );

    // A second start reports the running one.
    let again = json_out(&n.ok(&["nested", "start"]));
    assert_eq!(again["pid"], n.info["pid"]);
    assert_eq!(json_out(&n.ok(&["nested", "status"])), n.info);

    let outputs = json_out(&n.ok(&["nested", "outputs"]));
    assert_eq!(outputs.as_object().unwrap().len(), 1);

    let run = json_out(&n.ok(&["nested", "run", "/usr/lib/qt6/bin/qml", "-h"]));
    assert!(run["pid"].as_u64().is_some());
    assert!(PathBuf::from(run["log"].as_str().unwrap()).exists());

    // An app started through `action spawn` shows up as a window.
    n.ok(&[
        "nested",
        "action",
        "spawn",
        "--",
        "alacritty",
        "--title",
        "deskctl-e2e",
    ]);
    wait_for("the alacritty window", || {
        let w = json_out(&n.ok(&["nested", "windows"]));
        w.as_array()?
            .iter()
            .any(|w| w["title"] == "deskctl-e2e")
            .then_some(())
    });

    let pid = n.pid();
    n.ok(&["nested", "stop"]);
    assert!(
        !PathBuf::from(format!("/proc/{pid}")).exists(),
        "niri still runs"
    );
    let out = n.run(&["nested", "stop"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(n.run(&["nested", "windows"]).status.code(), Some(1));
}

#[test]
#[ignore]
fn shot_mapping_matches_image() {
    let n = Nested::start("shot", None);
    let outputs = json_out(&n.ok(&["nested", "outputs"]));
    let (name, out) = outputs.as_object().unwrap().iter().next().unwrap();
    let (w, h) = (
        out["logical"]["width"].as_f64().unwrap(),
        out["logical"]["height"].as_f64().unwrap(),
    );
    for (scale, args) in [
        (1.0, vec!["--output", name.as_str()]),
        (0.5, vec!["--scale", "0.5"]),
    ] {
        let file = n.dir.join(format!("shot-{scale}.png"));
        let mut cmd = vec!["nested", "shot"];
        cmd.extend(&args);
        cmd.push(file.to_str().unwrap());
        let m = json_out(&n.ok(&cmd));
        assert_eq!(m["scale"], scale);
        assert_eq!(m["region"]["w"], w);
        assert_eq!(m["image"]["w"].as_f64().unwrap(), (w * scale).round());
        assert_eq!(m["image"]["h"].as_f64().unwrap(), (h * scale).round());
        assert!(file.exists());
    }
    let m = json_out(&n.ok(&[
        "nested",
        "shot",
        "--region",
        "10,20,300x100",
        n.dir.join("r.png").to_str().unwrap(),
    ]));
    assert_eq!(m["image"]["w"], 300);
    assert_eq!(m["region"]["y"], 20.0);
}

/// The probe window (`tests/fixtures/probe.qml`) running in a nested niri.
struct Probe {
    log: PathBuf,
}

impl Probe {
    fn start(n: &Nested) -> Probe {
        let qml = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/probe.qml");
        let run = json_out(&n.ok(&["nested", "run", "/usr/lib/qt6/bin/qml", qml]));
        let p = Probe {
            log: PathBuf::from(run["log"].as_str().unwrap()),
        };
        p.wait("ready", |e| e["ev"] == "ready");
        p
    }

    fn events(&self) -> Vec<Value> {
        let text = std::fs::read_to_string(&self.log).unwrap_or_default();
        text.lines()
            .filter_map(|l| serde_json::from_str(&l[l.find('{')?..]).ok())
            .collect()
    }

    /// First event that matches.
    fn wait(&self, what: &str, f: impl Fn(&Value) -> bool) -> Value {
        wait_for(what, || self.events().into_iter().find(&f))
    }

    fn count(&self) -> usize {
        self.events().len()
    }

    /// First matching event after the first `skip` events.
    fn wait_after(&self, skip: usize, what: &str, f: impl Fn(&Value) -> bool) -> Value {
        wait_for(what, || self.events().into_iter().skip(skip).find(&f))
    }
}

fn near(e: &Value, x: f64, y: f64) -> bool {
    (e["x"].as_f64().unwrap() - x).abs() <= 1.0 && (e["y"].as_f64().unwrap() - y).abs() <= 1.0
}

#[test]
#[ignore]
fn pointer() {
    let n = Nested::start("pointer", None);
    let p = Probe::start(&n);

    let k = p.count();
    n.ok(&["nested", "move", "100", "200"]);
    let e = p.wait_after(k, "move", |e| e["ev"] == "move" && near(e, 100.0, 200.0));
    assert!(near(&e, 100.0, 200.0));

    let k = p.count();
    n.ok(&["nested", "click", "--at", "300,400"]);
    let e = p.wait_after(k, "left click", |e| e["ev"] == "click");
    assert!(near(&e, 300.0, 400.0), "{e}");
    assert_eq!(e["button"], 1);

    let k = p.count();
    n.ok(&["nested", "click", "--at", "500,600", "right"]);
    let e = p.wait_after(k, "right click", |e| e["ev"] == "click");
    assert!(near(&e, 500.0, 600.0), "{e}");
    assert_eq!(e["button"], 2);

    let k = p.count();
    n.ok(&["nested", "click", "--double", "--at", "700,300"]);
    let e = p.wait_after(k, "double click", |e| e["ev"] == "double");
    assert!(near(&e, 700.0, 300.0), "{e}");

    // Qt: positive angleDelta.y is up, positive angleDelta.x is left.
    let k = p.count();
    n.ok(&["nested", "scroll", "--at", "10,10", "2"]);
    let e = p.wait_after(k, "scroll down", |e| e["ev"] == "wheel" && e["dy"] != 0);
    assert_eq!(e["dy"], -240);

    let k = p.count();
    n.ok(&["nested", "scroll", "-1", "1"]);
    let e = p.wait_after(k, "scroll up and right", |e| {
        e["ev"] == "wheel" && e["dy"] != 0
    });
    assert_eq!(
        (e["dy"].as_i64(), e["dx"].as_i64()),
        (Some(120), Some(-120))
    );
}

const US_RU: &str = "input {\n    keyboard {\n        xkb {\n            layout \"us,ru\"\n        }\n    }\n}\nhotkey-overlay {\n    skip-at-startup\n}\n";

fn text_len(p: &Probe) -> u64 {
    p.events()
        .iter()
        .rev()
        .find(|e| e["ev"] == "text")
        .map(|e| e["len"].as_u64().unwrap())
        .unwrap_or(0)
}

#[test]
#[ignore]
fn typing_ignores_the_active_layout() {
    let n = Nested::start("typing", Some(US_RU));
    n.ok(&["nested", "action", "switch-layout", "next"]);
    let p = Probe::start(&n);
    let text = "Hello, мир! @#~ é€\tz";
    n.ok(&["nested", "type", text]);
    p.wait("typed text", |e| e["ev"] == "text" && e["text"] == text);
}

#[test]
#[ignore]
fn killed_while_typing_leaves_no_key_held() {
    let n = Nested::start("kill", None);
    let p = Probe::start(&n);
    let long = "x".repeat(5_000);
    let mut child = Command::new(env!("CARGO_BIN_EXE_deskctl"))
        .args(["nested", "type", &long])
        .env("DESKCTL_STATE_DIR", &n.dir)
        .spawn()
        .unwrap();
    wait_for("typing to start", || (text_len(&p) > 0).then_some(()));
    child.kill().unwrap();
    child.wait().unwrap();
    // Keys sent before the kill may still be in the probe's queue. A held
    // key would auto-repeat forever, so the text must stop growing.
    let mut last = text_len(&p);
    let len = wait_for("the text to stop growing", || {
        std::thread::sleep(Duration::from_millis(1000));
        let now = text_len(&p);
        let stable = now == last;
        last = now;
        stable.then_some(now)
    });
    assert!(len < long.len() as u64, "typing finished before the kill");
    // The keyboard still works.
    n.ok(&["nested", "type", "!"]);
    p.wait("text after kill", |e| {
        e["ev"] == "text" && e["text"].as_str().unwrap().ends_with('!')
    });
}

#[test]
#[ignore]
fn key_combos() {
    // Qt key codes and modifier flags as the probe reports them.
    const KEY_A: i64 = 0x41;
    const KEY_T: i64 = 0x54;
    const KEY_RETURN: i64 = 0x0100_0004;
    const KEY_F4: i64 = 0x0100_0033;
    const SHIFT: i64 = 0x0200_0000;
    const CTRL: i64 = 0x0400_0000;
    const ALT: i64 = 0x0800_0000;
    const META: i64 = 0x1000_0000;

    let n = Nested::start("keys", None);
    let p = Probe::start(&n);
    let k = p.count();
    n.ok(&[
        "nested",
        "key",
        "ctrl+a",
        "Return",
        "ctrl+shift+t",
        "alt+F4",
        "super+a",
    ]);
    let want = [
        (KEY_A, CTRL),
        (KEY_RETURN, 0),
        (KEY_T, CTRL | SHIFT),
        (KEY_F4, ALT),
        (KEY_A, META),
    ];
    let got = wait_for("five key events", || {
        let keys: Vec<(i64, i64)> = p
            .events()
            .into_iter()
            .skip(k)
            .filter(|e| e["ev"] == "key")
            .map(|e| (e["key"].as_i64().unwrap(), e["mods"].as_i64().unwrap()))
            .collect();
        (keys.len() >= want.len()).then_some(keys)
    });
    assert_eq!(got, want);

    assert_eq!(n.run(&["nested", "key", "hyper+a"]).status.code(), Some(2));
    assert_eq!(
        n.run(&["nested", "key", "NoSuchKey"]).status.code(),
        Some(2)
    );
}

/// Center of the red pixels within `r` pixels of (x, y) in a PNG.
fn red_center(file: &std::path::Path, x: f64, y: f64, r: f64) -> Option<(f64, f64)> {
    let f = std::io::BufReader::new(std::fs::File::open(file).unwrap());
    let mut reader = png::Decoder::new(f).read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).unwrap();
    assert_eq!(info.bit_depth, png::BitDepth::Eight);
    let channels = info.color_type.samples();
    let (mut sx, mut sy, mut n) = (0.0, 0.0, 0.0);
    for py in 0..info.height {
        for px in 0..info.width {
            if (f64::from(px) - x).abs() > r || (f64::from(py) - y).abs() > r {
                continue;
            }
            let i = (py * info.width + px) as usize * channels;
            let (red, g, b) = (buf[i], buf[i + 1], buf[i + 2]);
            if red > 200 && g < 80 && b < 80 {
                sx += f64::from(px);
                sy += f64::from(py);
                n += 1.0;
            }
        }
    }
    (n > 0.0).then(|| (sx / n, sy / n))
}

#[test]
#[ignore]
fn shot_mapping_finds_clicked_points() {
    let n = Nested::start("mapping", None);
    let p = Probe::start(&n);
    let points = [(300.0, 400.0), (1500.0, 900.0)];
    for (x, y) in points {
        let k = p.count();
        n.ok(&["nested", "click", "--at", &format!("{x},{y}")]);
        p.wait_after(k, "click", |e| e["ev"] == "click");
    }
    std::thread::sleep(Duration::from_millis(300)); // let the markers render

    let shots: [&[&str]; 3] = [&[], &["--scale", "0.5"], &["--region", "1400,800,300x200"]];
    for (i, extra) in shots.iter().enumerate() {
        let file = n.dir.join(format!("map-{i}.png"));
        let mut cmd = vec!["nested", "shot"];
        cmd.extend(extra.iter());
        cmd.push(file.to_str().unwrap());
        let m = json_out(&n.ok(&cmd));
        let scale = m["scale"].as_f64().unwrap();
        let (rx, ry) = (
            m["region"]["x"].as_f64().unwrap(),
            m["region"]["y"].as_f64().unwrap(),
        );
        let (rw, rh) = (
            m["region"]["w"].as_f64().unwrap(),
            m["region"]["h"].as_f64().unwrap(),
        );
        for (x, y) in points {
            if x < rx || y < ry || x >= rx + rw || y >= ry + rh {
                continue;
            }
            // logical = region + pixel / scale  ⇔  pixel = (logical - region) * scale
            let (px, py) = ((x - rx) * scale, (y - ry) * scale);
            let (cx, cy) = red_center(&file, px, py, 8.0)
                .unwrap_or_else(|| panic!("no marker near {px},{py} in {}", file.display()));
            assert!(
                (cx - px).abs() <= 1.0 && (cy - py).abs() <= 1.0,
                "{}: marker for {x},{y} at {cx},{cy}, expected {px},{py}",
                file.display()
            );
        }
    }
}

#[test]
#[ignore]
fn idle_check_waits_for_pointer_activity_to_stop() {
    let n = Nested::start("idle", None);
    let timed = |args: &[&str]| {
        let t = Instant::now();
        n.ok(args);
        t.elapsed()
    };
    // Idle: about --idle-ms (niri starts the timer when asked).
    let quiet = timed(&["nested", "move", "--idle-ms", "300", "10", "10"]);
    assert!(
        quiet >= Duration::from_millis(300) && quiet < Duration::from_millis(1500),
        "{quiet:?}"
    );

    // Busy for ~2 s: virtual pointer motion counts as activity (virtual
    // keyboard input does not, in niri 26.04).
    let dir = n.dir.clone();
    let busy = std::thread::spawn(move || {
        let end = Instant::now() + Duration::from_secs(2);
        let mut x = 0;
        while Instant::now() < end {
            x = (x + 7) % 500;
            Command::new(env!("CARGO_BIN_EXE_deskctl"))
                .args(["nested", "move", &x.to_string(), "50"])
                .env("DESKCTL_STATE_DIR", &dir)
                .output()
                .unwrap();
            std::thread::sleep(Duration::from_millis(50));
        }
    });
    std::thread::sleep(Duration::from_millis(200));
    let waited = timed(&["nested", "move", "--idle-ms", "300", "20", "20"]);
    busy.join().unwrap();
    assert!(
        waited >= Duration::from_millis(1800),
        "returned while busy: {waited:?}"
    );
}

/// The GTK 4 probe (`tests/fixtures/probe_gtk.py`) in a nested niri.
fn start_gtk_probe(n: &Nested) -> Probe {
    let py = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/probe_gtk.py");
    let run = json_out(&n.ok(&["nested", "run", "python3", "-I", py]));
    let p = Probe {
        log: PathBuf::from(run["log"].as_str().unwrap()),
    };
    p.wait("gtk ready", |e| e["ev"] == "ready");
    p
}

/// `a11y apps` until it lists `count` apps that export a tree.
fn a11y_apps(n: &Nested, count: usize) -> Vec<Value> {
    wait_for("apps on the accessibility bus", || {
        let apps = json_out(&n.ok(&["nested", "a11y", "apps"]));
        let exporting: Vec<Value> = apps
            .as_array()?
            .iter()
            .filter(|a| a["exports"] == true)
            .cloned()
            .collect();
        (exporting.len() >= count).then_some(exporting)
    })
}

#[test]
#[ignore]
fn a11y_apps_lists_only_this_target() {
    let n = Nested::start("a11y-apps", None);
    Probe::start(&n);
    start_gtk_probe(&n);
    let apps = a11y_apps(&n, 2);
    let toolkits: Vec<String> = apps
        .iter()
        .map(|a| a["toolkit"].as_str().unwrap().to_lowercase())
        .collect();
    assert_eq!(apps.len(), 2, "{apps:?}");
    assert!(
        toolkits.contains(&"qt".into()) && toolkits.contains(&"gtk".into()),
        "{toolkits:?}"
    );
    for a in &apps {
        assert_eq!(a["windows"].as_array().unwrap().len(), 1, "{a}");
    }
}

#[test]
#[ignore]
fn a11y_tree_and_find_in_both_toolkits() {
    let n = Nested::start("a11y-tree", None);
    Probe::start(&n);
    start_gtk_probe(&n);
    let apps = a11y_apps(&n, 2);

    for app in &apps {
        let win = app["windows"][0].as_u64().unwrap().to_string();
        let t = Instant::now();
        let tree = json_out(&n.ok(&["nested", "a11y", "tree", "--window", &win]));
        assert!(
            t.elapsed() < Duration::from_secs(2),
            "slow tree: {:?}",
            t.elapsed()
        );
        assert_eq!(tree["truncated"], false);
        assert_eq!(
            tree["apps"].as_array().unwrap().len(),
            1,
            "scoped to one app"
        );
        let text = tree.to_string();
        assert!(text.contains("\"Increment\""), "{}", app["toolkit"]);

        let found = json_out(&n.ok(&[
            "nested",
            "a11y",
            "find",
            "--window",
            &win,
            "--role",
            "button",
            "--name",
            "Increment",
        ]));
        let found = found.as_array().unwrap();
        assert_eq!(found.len(), 1, "{} {found:?}", app["toolkit"]);
        assert!(!found[0]["actions"].as_array().unwrap().is_empty());
        assert!(found[0]["bounds"]["w"].as_i64().unwrap() > 0);

        let pw = json_out(&n.ok(&[
            "nested",
            "a11y",
            "find",
            "--window",
            &win,
            "--role",
            "password text",
        ]));
        assert_eq!(
            pw.as_array().unwrap().len(),
            1,
            "{} password field",
            app["toolkit"]
        );
    }

    let cut = json_out(&n.ok(&["nested", "a11y", "tree", "--depth", "1"]));
    assert_eq!(cut["truncated"], true);
    let cut = json_out(&n.ok(&["nested", "a11y", "tree", "--max-nodes", "5"]));
    assert_eq!(cut["truncated"], true);
    assert_eq!(
        n.run(&["nested", "a11y", "find", "--name-re", "("])
            .status
            .code(),
        Some(2)
    );
    assert_eq!(
        n.run(&["nested", "a11y", "tree", "--window", "999999"])
            .status
            .code(),
        Some(1)
    );
}

fn a11y_id(n: &Nested, app: &Value, role: &str, name: Option<&str>) -> String {
    let pid = app["pid"].to_string();
    let mut args = vec!["nested", "a11y", "find", "--app", &pid, "--role", role];
    if let Some(name) = name {
        args.extend(["--name", name]);
    }
    let found = json_out(&n.ok(&args));
    found[0]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("no {role} {name:?}: {found}"))
        .to_string()
}

#[test]
#[ignore]
fn a11y_actions_in_both_toolkits() {
    let n = Nested::start("a11y-act", None);
    let qt = Probe::start(&n);
    let gtk = start_gtk_probe(&n);
    let apps = a11y_apps(&n, 2);
    for app in &apps {
        let is_qt = app["toolkit"].as_str().unwrap().eq_ignore_ascii_case("qt");
        let p = if is_qt { &qt } else { &gtk };

        let k = p.count();
        n.ok(&[
            "nested",
            "a11y",
            "act",
            &a11y_id(&n, app, "button", Some("Increment")),
        ]);
        p.wait_after(k, "button event", |e| e["ev"] == "button");

        let field = a11y_id(&n, app, "text", Some("Name"));
        let k = p.count();
        n.ok(&["nested", "a11y", "set-text", &field, "Hello, мир"]);
        p.wait_after(k, "field text", |e| {
            e["ev"] == "field" && e["text"] == "Hello, мир"
        });

        let focus = n.run(&["nested", "a11y", "focus", &field]);
        if is_qt {
            assert!(focus.status.success());
            p.wait("focus event", |e| {
                e["ev"] == "focus" && e["focused"] == true
            });
        } else {
            assert_eq!(
                focus.status.code(),
                Some(1),
                "GTK 4 has no focus over AT-SPI"
            );
        }

        let pw = a11y_id(&n, app, "password text", None);
        let k = p.count();
        assert_eq!(
            n.run(&["nested", "a11y", "set-text", &pw, "secret"])
                .status
                .code(),
            Some(3)
        );
        let button = a11y_id(&n, app, "button", Some("Increment"));
        assert_eq!(
            n.run(&["nested", "a11y", "act", &button, "NoSuchAction"])
                .status
                .code(),
            Some(2)
        );
        assert_eq!(
            n.run(&["nested", "a11y", "act", "--expect", "wrong.app", &button])
                .status
                .code(),
            Some(3)
        );
        std::thread::sleep(Duration::from_millis(300));
        assert!(
            !p.events()
                .iter()
                .skip(k)
                .any(|e| e["ev"] == "button" || e["name"] == "Password"),
            "a refused command reached the app"
        );
    }
    let stale = n.run(&[
        "nested",
        "a11y",
        "act",
        ":1.99999/org/a11y/atspi/accessible/1",
    ]);
    assert_eq!(stale.status.code(), Some(1));
    assert_eq!(
        n.run(&["nested", "a11y", "act", "garbage"]).status.code(),
        Some(2)
    );
}
