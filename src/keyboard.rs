//! Virtual keyboard (zwp_virtual_keyboard_v1). deskctl uploads its own
//! XKB keymap that holds exactly the keysyms it needs, one per keycode,
//! so text types the same under any layout the user has.
//!
//! Keys are paced (`--delay-ms`): sent at full speed they pile up in a
//! slow client's socket, and the command returns long before the app has
//! the text.
//!
//! A key's press and release (or a whole combo) go out in one socket
//! write: if deskctl is killed, the compositor got all of it or none, so
//! no key stays held.

use std::fmt::Write as _;
use std::io::Write as _;
use std::os::fd::AsFd;

use anyhow::{Context, Result, bail};
use rustix::fs::{MemfdFlags, memfd_create};
use wayland_client::protocol::wl_seat::WlSeat;
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::{
    zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1,
    zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1,
};

use crate::guard;
use crate::target::{Env, Target};
use crate::wl::{Wl, now_ms};

const KEYMAP_FORMAT_XKB_V1: u32 = 1;
const PRESSED: u32 = 1;
const RELEASED: u32 = 0;
/// Keysyms per keymap; longer texts upload a new keymap per chunk.
const MAX_KEYS: usize = 200;

pub const XK_RETURN: u32 = 0xff0d;
pub const XK_TAB: u32 = 0xff09;

/// Keysym of a character: Latin-1 directly, other Unicode as
/// 0x01000000 + code point (the XKB rule).
pub fn keysym_for(c: char) -> Result<u32> {
    let cp = c as u32;
    Ok(match c {
        '\n' => XK_RETURN,
        '\t' => XK_TAB,
        _ if c.is_control() => bail!("cannot type control character U+{cp:04X}"),
        _ if (0x20..=0x7e).contains(&cp) || (0xa0..=0xff).contains(&cp) => cp,
        _ => 0x0100_0000 + cp,
    })
}

/// Modifier keys at the end of every keymap, so `modifiers` masks map to
/// the usual names (Control, Mod1 = Alt, Mod4 = Super) in clients.
const MODIFIER_KEYS: [(&str, &str); 4] = [
    ("Shift_L", "Shift"),
    ("Control_L", "Control"),
    ("Alt_L", "Mod1"),
    ("Super_L", "Mod4"),
];

/// XKB keymap with keysym i on keycode 9 + i (evdev code 1 + i), then
/// the modifier keys.
pub fn keymap_text(keysyms: &[u32]) -> String {
    let n = keysyms.len();
    let mut s = String::from("xkb_keymap {\nxkb_keycodes \"deskctl\" {\nminimum = 8;\n");
    let _ = writeln!(s, "maximum = {};", 8 + n + MODIFIER_KEYS.len());
    for i in 0..n + MODIFIER_KEYS.len() {
        let _ = writeln!(s, "<K{i}> = {};", 9 + i);
    }
    s.push_str("};\nxkb_types \"deskctl\" { include \"complete\" };\n");
    s.push_str("xkb_compatibility \"deskctl\" { include \"complete\" };\n");
    s.push_str("xkb_symbols \"deskctl\" {\n");
    for (i, sym) in keysyms.iter().enumerate() {
        let _ = writeln!(s, "key <K{i}> {{ [ 0x{sym:x} ] }};");
    }
    for (j, (name, real)) in MODIFIER_KEYS.iter().enumerate() {
        let i = n + j;
        let _ = writeln!(s, "key <K{i}> {{ [ {name} ] }};");
        let _ = writeln!(s, "modifier_map {real} {{ <K{i}> }};");
    }
    s.push_str("};\n};\n");
    s
}

pub const MOD_SHIFT: u32 = 1 << 0;
pub const MOD_CTRL: u32 = 1 << 2;
pub const MOD_ALT: u32 = 1 << 3;
pub const MOD_SUPER: u32 = 1 << 6;

/// One key with modifiers held, e.g. `ctrl+shift+t`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Combo {
    pub mods: u32,
    pub keysym: u32,
}

/// Keysym of a key name: one character, an alias, or an XKB keysym name
/// (`Return`, `F4`, `Page_Down`, `XF86AudioMute`).
pub fn key_by_name(name: &str) -> Option<u32> {
    let mut chars = name.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        return keysym_for(c)
            .ok()
            .filter(|_| !c.is_whitespace() || c == ' ');
    }
    let name = match name.to_ascii_lowercase().as_str() {
        "enter" => "Return",
        "esc" => "Escape",
        "space" => "space",
        "del" => "Delete",
        "pgup" => "Page_Up",
        "pgdn" => "Page_Down",
        _ => name,
    };
    if name.contains('\0') {
        return None;
    }
    use xkbcommon::xkb;
    [xkb::KEYSYM_NO_FLAGS, xkb::KEYSYM_CASE_INSENSITIVE]
        .into_iter()
        .map(|f| xkb::keysym_from_name(name, f).raw())
        .find(|&k| k != 0)
}

pub fn parse_combo(s: &str) -> Result<Combo, String> {
    // `+` alone and `ctrl++` mean the plus key.
    let (mods_part, key) = if s == "+" {
        ("", "+")
    } else if let Some(m) = s.strip_suffix("++") {
        (m, "+")
    } else {
        s.rsplit_once('+').unwrap_or(("", s))
    };
    let mut mods = 0;
    for m in mods_part.split('+').filter(|m| !m.is_empty()) {
        mods |= match m.to_ascii_lowercase().as_str() {
            "shift" => MOD_SHIFT,
            "ctrl" | "control" => MOD_CTRL,
            "alt" => MOD_ALT,
            "super" | "logo" | "mod4" | "win" => MOD_SUPER,
            _ => return Err(format!("unknown modifier {m:?} in {s:?}")),
        };
    }
    let keysym = key_by_name(key).ok_or_else(|| format!("unknown key {key:?} in {s:?}"))?;
    Ok(Combo { mods, keysym })
}

/// Split a keysym sequence into runs with at most `max` distinct keysyms.
pub fn chunks(seq: &[u32], max: usize) -> Vec<(Vec<u32>, Vec<u32>)> {
    let mut out = Vec::new();
    let mut distinct: Vec<u32> = Vec::new();
    let mut run: Vec<u32> = Vec::new();
    for &s in seq {
        if !distinct.contains(&s) {
            if distinct.len() == max {
                out.push((std::mem::take(&mut distinct), std::mem::take(&mut run)));
            }
            distinct.push(s);
        }
        run.push(s);
    }
    if !run.is_empty() {
        out.push((distinct, run));
    }
    out
}

pub struct Keyboard {
    wl: Wl,
    kb: ZwpVirtualKeyboardV1,
    target: Target,
}

impl Keyboard {
    pub fn new(env: &Env, target: Target) -> Result<Keyboard> {
        let wl = Wl::connect(env)?;
        let manager = wl.bind::<ZwpVirtualKeyboardManagerV1>(1..=1)?;
        let seat = wl.bind::<WlSeat>(1..=7)?;
        let kb = manager.create_virtual_keyboard(&seat, &wl.qh, ());
        Ok(Keyboard { wl, kb, target })
    }

    /// Upload a keymap; keysym i is then evdev code 1 + i.
    fn upload(&mut self, keysyms: &[u32]) -> Result<()> {
        let text = keymap_text(keysyms);
        let fd = memfd_create(c"deskctl-keymap", MemfdFlags::CLOEXEC).context("memfd")?;
        let mut file = std::fs::File::from(fd);
        file.write_all(text.as_bytes())?;
        file.write_all(&[0])?;
        guard::check_stop(self.target)?;
        self.kb
            .keymap(KEYMAP_FORMAT_XKB_V1, file.as_fd(), text.len() as u32 + 1);
        self.kb.modifiers(0, 0, 0, 0);
        self.wl.sync()
    }

    /// Press and release each key, one socket write per key.
    fn tap(&mut self, code: u32) -> Result<()> {
        guard::check_stop(self.target)?;
        let t = now_ms();
        self.kb.key(t, code, PRESSED);
        self.kb.key(t, code, RELEASED);
        self.wl.flush()
    }

    pub fn type_text(&mut self, text: &str, delay: std::time::Duration) -> Result<()> {
        let syms = text.chars().map(keysym_for).collect::<Result<Vec<_>>>()?;
        for (distinct, run) in chunks(&syms, MAX_KEYS) {
            self.upload(&distinct)?;
            for s in run {
                let i = distinct.iter().position(|&d| d == s).expect("in keymap");
                self.tap(i as u32 + 1)?;
                std::thread::sleep(delay);
            }
            self.wl.sync()?;
        }
        Ok(())
    }
}

impl Keyboard {
    /// Each combo in one write: modifiers on, key down, key up, modifiers off.
    pub fn press_combos(&mut self, combos: &[Combo], delay: std::time::Duration) -> Result<()> {
        let mut distinct: Vec<u32> = Vec::new();
        for c in combos {
            if !distinct.contains(&c.keysym) {
                distinct.push(c.keysym);
            }
        }
        self.upload(&distinct)?;
        for c in combos {
            let code = distinct
                .iter()
                .position(|&d| d == c.keysym)
                .expect("in keymap") as u32
                + 1;
            guard::check_stop(self.target)?;
            let t = now_ms();
            self.kb.modifiers(c.mods, 0, 0, 0);
            self.kb.key(t, code, PRESSED);
            self.kb.key(t, code, RELEASED);
            self.kb.modifiers(0, 0, 0, 0);
            self.wl.flush()?;
            std::thread::sleep(delay);
        }
        self.wl.sync()
    }
}

impl Drop for Keyboard {
    fn drop(&mut self) {
        self.kb.destroy();
        let _ = self.wl.sync();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keysyms() {
        assert_eq!(keysym_for('a').unwrap(), 0x61);
        assert_eq!(keysym_for('A').unwrap(), 0x41);
        assert_eq!(keysym_for('é').unwrap(), 0xe9);
        assert_eq!(keysym_for('м').unwrap(), 0x0100_043c);
        assert_eq!(keysym_for('€').unwrap(), 0x0100_20ac);
        assert_eq!(keysym_for('\n').unwrap(), XK_RETURN);
        assert_eq!(keysym_for('\t').unwrap(), XK_TAB);
        assert!(keysym_for('\u{7}').is_err());
    }

    #[test]
    fn keymap() {
        let k = keymap_text(&[0x61, 0x0100_043c]);
        assert!(k.contains("maximum = 14;"));
        assert!(k.contains("<K1> = 10;"));
        assert!(k.contains("key <K1> { [ 0x100043c ] };"));
    }

    #[test]
    fn modifier_keys_in_keymap() {
        let k = keymap_text(&[0x61]);
        assert!(k.contains("maximum = 13;"));
        assert!(k.contains("key <K3> { [ Alt_L ] };"));
        assert!(k.contains("modifier_map Mod1 { <K3> };"));
    }

    #[test]
    fn combos() {
        let c = |s: &str| parse_combo(s);
        assert_eq!(
            c("Return"),
            Ok(Combo {
                mods: 0,
                keysym: XK_RETURN
            })
        );
        assert_eq!(
            c("enter"),
            Ok(Combo {
                mods: 0,
                keysym: XK_RETURN
            })
        );
        assert_eq!(
            c("ctrl+a"),
            Ok(Combo {
                mods: MOD_CTRL,
                keysym: 0x61
            })
        );
        assert_eq!(
            c("Ctrl+Shift+t"),
            Ok(Combo {
                mods: MOD_CTRL | MOD_SHIFT,
                keysym: 0x74
            })
        );
        assert_eq!(
            c("alt+F4"),
            Ok(Combo {
                mods: MOD_ALT,
                keysym: 0xffc1
            })
        );
        assert_eq!(
            c("super+1"),
            Ok(Combo {
                mods: MOD_SUPER,
                keysym: 0x31
            })
        );
        assert_eq!(
            c("ctrl++"),
            Ok(Combo {
                mods: MOD_CTRL,
                keysym: 0x2b
            })
        );
        assert_eq!(
            c("+"),
            Ok(Combo {
                mods: 0,
                keysym: 0x2b
            })
        );
        assert_eq!(c("page_down").map(|c| c.keysym), Ok(0xff56));
        assert_eq!(c("XF86AudioMute").map(|c| c.keysym), Ok(0x1008ff12));
        assert!(c("hyper+a").is_err());
        assert!(c("NoSuchKey").is_err());
        assert!(c("ctrl+").is_err());
    }

    #[test]
    fn chunking() {
        let c = chunks(&[1, 2, 1, 3, 2, 4], 2);
        assert_eq!(
            c,
            vec![
                (vec![1, 2], vec![1, 2, 1]),
                (vec![3, 2], vec![3, 2]),
                (vec![4], vec![4]),
            ]
        );
        assert!(chunks(&[], 2).is_empty());
    }
}
