//! Virtual pointer (zwlr_virtual_pointer_v1 v2): move, click, scroll.

use std::time::Duration;

use anyhow::{Context, Result};
use wayland_client::protocol::{wl_pointer, wl_seat::WlSeat};
use wayland_protocols_wlr::virtual_pointer::v1::client::{
    zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1,
    zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1,
};

use crate::cli::{Button, Point, Region};
use crate::target::{Env, Target};
use crate::wl::{Wl, now_ms};
use crate::{guard, niri, shot};

const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;
const BTN_MIDDLE: u32 = 0x112;
/// Scroll distance of one wheel step, as libinput reports it.
const WHEEL_STEP: f64 = 15.0;

/// Is the point on some output? (Gaps between outputs are not.)
pub fn on_output(rects: &[Region], x: f64, y: f64) -> bool {
    rects
        .iter()
        .any(|r| x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h)
}

/// Logical point → `motion_absolute` arguments, relative to the bounding
/// rectangle of all outputs. A virtual pointer without an output maps
/// there with no transform (niri `src/input/mod.rs`, absolute motion
/// fallback). Bound to an output, niri applies the output's transform,
/// and a nested niri's winit output carries a fake `Flipped180`.
pub fn device_coords(x: f64, y: f64, bbox: &Region) -> (u32, u32, u32, u32) {
    let (w, h) = (bbox.w as u32, bbox.h as u32);
    let dx = ((x - bbox.x).round().max(0.0) as u32).min(w - 1);
    let dy = ((y - bbox.y).round().max(0.0) as u32).min(h - 1);
    (dx, dy, w, h)
}

fn code(b: Button) -> u32 {
    match b {
        Button::Left => BTN_LEFT,
        Button::Right => BTN_RIGHT,
        Button::Middle => BTN_MIDDLE,
    }
}

pub struct Pointer {
    wl: Wl,
    manager: ZwlrVirtualPointerManagerV1,
    seat: WlSeat,
    rects: Vec<Region>,
    bbox: Region,
    device: Option<ZwlrVirtualPointerV1>,
    target: Target,
}

impl Pointer {
    pub fn new(env: &Env, target: Target) -> Result<Pointer> {
        let wl = Wl::connect(env)?;
        let manager = wl.bind::<ZwlrVirtualPointerManagerV1>(2..=2)?;
        let seat = wl.bind::<WlSeat>(1..=7)?;
        let rects: Vec<Region> = shot::output_rects(&niri::query(env, "outputs")?)
            .into_iter()
            .map(|(_, r)| r)
            .collect();
        let bbox = shot::bounding(&rects).context("niri reports no outputs")?;
        Ok(Pointer {
            wl,
            manager,
            seat,
            rects,
            bbox,
            device: None,
            target,
        })
    }

    fn device(&mut self) -> &ZwlrVirtualPointerV1 {
        if self.device.is_none() {
            let d = self
                .manager
                .create_virtual_pointer(Some(&self.seat), &self.wl.qh, ());
            self.device = Some(d);
        }
        self.device.as_ref().expect("just set")
    }

    /// Absolute move to a point in logical layout coordinates.
    pub fn move_to(&mut self, p: Point) -> Result<()> {
        if !on_output(&self.rects, p.x, p.y) {
            anyhow::bail!("{},{} is on no output", p.x, p.y);
        }
        let (x, y, w, h) = device_coords(p.x, p.y, &self.bbox);
        guard::check_stop(self.target)?;
        let d = self.device();
        d.motion_absolute(now_ms(), x, y, w, h);
        d.frame();
        self.wl.sync()
    }

    pub fn click(&mut self, button: Button, double: bool) -> Result<()> {
        let code = code(button);
        for i in 0..if double { 2 } else { 1 } {
            if i > 0 {
                std::thread::sleep(Duration::from_millis(40));
            }
            guard::check_stop(self.target)?;
            let d = self.device();
            d.button(now_ms(), code, wl_pointer::ButtonState::Pressed);
            d.frame();
            self.wl.sync()?;
            std::thread::sleep(Duration::from_millis(20));
            let d = self.device();
            d.button(now_ms(), code, wl_pointer::ButtonState::Released);
            d.frame();
            self.wl.sync()?;
        }
        Ok(())
    }

    /// Scroll in wheel steps; positive dy is down, positive dx is right.
    pub fn scroll(&mut self, dy: f64, dx: f64) -> Result<()> {
        guard::check_stop(self.target)?;
        let d = self.device().clone();
        d.axis_source(wl_pointer::AxisSource::Wheel);
        for (axis, steps) in [
            (wl_pointer::Axis::VerticalScroll, dy),
            (wl_pointer::Axis::HorizontalScroll, dx),
        ] {
            if steps == 0.0 {
                continue;
            }
            let t = now_ms();
            if steps.fract() == 0.0 {
                d.axis_discrete(t, axis, steps * WHEEL_STEP, steps as i32);
            } else {
                d.axis(t, axis, steps * WHEEL_STEP);
            }
        }
        d.frame();
        self.wl.sync()
    }
}

impl Drop for Pointer {
    fn drop(&mut self) {
        if let Some(d) = self.device.take() {
            d.destroy();
        }
        let _ = self.wl.sync();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(x: f64, y: f64, w: f64, h: f64) -> Region {
        Region { x, y, w, h }
    }

    #[test]
    fn coords_relative_to_all_outputs() {
        let one = r(0.0, 0.0, 2238.0, 1362.0);
        assert_eq!(device_coords(100.0, 200.0, &one), (100, 200, 2238, 1362));
        assert_eq!(device_coords(9999.0, -5.0, &one), (2237, 0, 2238, 1362));
        // two outputs: bounding box from (0,-200), 4160x1600
        let rects = [
            r(0.0, 0.0, 2240.0, 1400.0),
            r(2240.0, -200.0, 1920.0, 1080.0),
        ];
        let bbox = shot::bounding(&rects).unwrap();
        assert_eq!(
            device_coords(3000.0, -100.0, &bbox),
            (3000, 100, 4160, 1600)
        );
    }

    #[test]
    fn point_on_output() {
        let rects = [r(0.0, 0.0, 100.0, 50.0), r(100.0, -20.0, 80.0, 60.0)];
        assert!(on_output(&rects, 10.0, 10.0));
        assert!(on_output(&rects, 150.0, -10.0));
        assert!(on_output(&rects, 100.0, 0.0));
        assert!(!on_output(&rects, 50.0, 50.0)); // below a, left of nothing
        assert!(!on_output(&rects, 50.0, -10.0)); // gap above a
    }
}
