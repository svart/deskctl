//! Screenshots through grim, and how their pixels map to logical ones.

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::cli::Region;
use crate::niri;
use crate::target::Env;

/// Logical rectangles of all outputs, by name.
pub fn output_rects(outputs: &Value) -> Vec<(String, Region)> {
    let Some(map) = outputs.as_object() else {
        return Vec::new();
    };
    map.iter()
        .filter_map(|(name, o)| {
            let l = &o["logical"];
            Some((
                name.clone(),
                Region {
                    x: l["x"].as_f64()?,
                    y: l["y"].as_f64()?,
                    w: l["width"].as_f64()?,
                    h: l["height"].as_f64()?,
                },
            ))
        })
        .collect()
}

/// Smallest rectangle around all outputs.
pub fn bounding(rects: &[Region]) -> Option<Region> {
    let first = rects.first()?;
    let (mut x0, mut y0) = (first.x, first.y);
    let (mut x1, mut y1) = (first.x + first.w, first.y + first.h);
    for r in rects {
        x0 = x0.min(r.x);
        y0 = y0.min(r.y);
        x1 = x1.max(r.x + r.w);
        y1 = y1.max(r.y + r.h);
    }
    Some(Region {
        x: x0,
        y: y0,
        w: x1 - x0,
        h: y1 - y0,
    })
}

/// Width and height from a PNG header.
pub fn png_size(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 24 || &bytes[..8] != b"\x89PNG\r\n\x1a\n" || &bytes[12..16] != b"IHDR" {
        return None;
    }
    let w = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
    let h = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
    Some((w, h))
}

pub fn mapping(file: &Path, region: Region, image: (u32, u32)) -> Value {
    json!({
        "file": file,
        "region": {"x": region.x, "y": region.y, "w": region.w, "h": region.h},
        "image": {"w": image.0, "h": image.1},
        "scale": f64::from(image.0) / region.w,
    })
}

pub fn shot(
    env: &Env,
    output: Option<&str>,
    region: Option<Region>,
    scale: Option<f64>,
    file: &Path,
) -> Result<Value> {
    let rects = output_rects(&niri::query(env, "outputs")?);
    let mut grim = Command::new("grim");
    grim.envs(env.vars());
    let area = match (output, region) {
        (Some(name), _) => {
            let (_, r) = rects
                .iter()
                .find(|(n, _)| n == name)
                .with_context(|| format!("no output {name:?}"))?;
            grim.args(["-o", name]);
            *r
        }
        (None, Some(r)) => {
            let r = Region {
                x: r.x.round(),
                y: r.y.round(),
                w: r.w.round(),
                h: r.h.round(),
            };
            grim.args(["-g", &format!("{},{} {}x{}", r.x, r.y, r.w, r.h)]);
            r
        }
        (None, None) => {
            let all: Vec<Region> = rects.iter().map(|(_, r)| *r).collect();
            bounding(&all).context("niri reports no outputs")?
        }
    };
    if let Some(s) = scale {
        if s <= 0.0 {
            return Err(crate::Fail::Usage("--scale must be positive".into()).into());
        }
        grim.args(["-s", &s.to_string()]);
    }
    let file = std::path::absolute(file)?;
    let out = grim.arg(&file).output().context("running grim")?;
    if !out.status.success() {
        bail!("grim: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    let bytes = std::fs::read(&file).with_context(|| format!("reading {}", file.display()))?;
    let image = png_size(&bytes).context("grim did not write a PNG")?;
    Ok(mapping(&file, area, image))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rects_and_bounding() {
        let outputs = json!({
            "eDP-1": {"logical": {"x": 0, "y": 0, "width": 2240, "height": 1400}},
            "HDMI-1": {"logical": {"x": 2240, "y": -200, "width": 1920, "height": 1080}},
            "off": {"logical": null},
        });
        let rects = output_rects(&outputs);
        assert_eq!(rects.len(), 2);
        let all: Vec<Region> = rects.iter().map(|(_, r)| *r).collect();
        assert_eq!(
            bounding(&all),
            Some(Region {
                x: 0.0,
                y: -200.0,
                w: 4160.0,
                h: 1600.0
            })
        );
        assert_eq!(bounding(&[]), None);
    }

    #[test]
    fn png_header() {
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        png.extend(1120u32.to_be_bytes());
        png.extend(700u32.to_be_bytes());
        assert_eq!(png_size(&png), Some((1120, 700)));
        assert_eq!(png_size(b"GIF89a"), None);
    }

    #[test]
    fn mapping_scale() {
        let r = Region {
            x: 100.0,
            y: 0.0,
            w: 2240.0,
            h: 1400.0,
        };
        let m = mapping(Path::new("/tmp/s.png"), r, (1120, 700));
        assert_eq!(m["scale"], 0.5);
        assert_eq!(m["region"]["x"], 100.0);
        assert_eq!(m["image"]["h"], 700);
    }
}
