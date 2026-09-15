//! Gyroid sparse infill (Slic3r `FillGyroid`, simplified wave generator).

use bambu_geom::{unscale, Point, Polygon, Polyline};

use super::{bbox, clip_to_region};

const DENSITY_ADJUST: f64 = 2.44;

pub fn fill(region: &[Polygon], spacing_mm: f64, density: f64, z_mm: f64) -> Vec<Polyline> {
    let Some((min, max)) = bbox(region) else {
        return Vec::new();
    };
    let density_adj = (density * DENSITY_ADJUST).max(0.05);
    let period = spacing_mm / density_adj;
    if period <= 1e-4 {
        return Vec::new();
    }

    let min_x = unscale(min.x);
    let min_y = unscale(min.y);
    let width = unscale(max.x - min.x);
    let height = unscale(max.y - min.y);
    let z = z_mm / period;
    let z_sin = z.sin();
    let z_cos = z.cos();
    let vertical = z_sin.abs() <= z_cos.abs();

    let mut waves = Vec::new();
    let (w, h) = if vertical {
        (height, width)
    } else {
        (width, height)
    };
    let mut y0 = if vertical { -std::f64::consts::PI } else { 0.0 };
    let upper = if vertical { w } else { h };
    let mut flip = !vertical;
    while y0 < upper {
        waves.push(make_wave(
            w, h, y0, period, z_sin, z_cos, vertical, flip, min_x, min_y,
        ));
        y0 += std::f64::consts::PI;
        flip = !flip;
    }
    clip_to_region(waves, region)
}

fn wave_y(x: f64, z_sin: f64, z_cos: f64, vertical: bool, flip: bool) -> f64 {
    if vertical {
        let phase = if z_cos < 0.0 {
            std::f64::consts::PI
        } else {
            0.0
        } + std::f64::consts::PI;
        let a = (x + phase).sin();
        let b = -z_cos;
        let res = z_sin * (x + phase + if flip { std::f64::consts::PI } else { 0.0 }).cos();
        let r = (a * a + b * b).sqrt().max(1e-9);
        a.atan2(r) + (res / r).clamp(-1.0, 1.0).asin() + std::f64::consts::PI
    } else {
        let phase = if z_sin < 0.0 {
            std::f64::consts::PI
        } else {
            0.0
        };
        let a = (x + phase).cos();
        let b = -z_sin;
        let res = z_cos * (x + phase + if flip { 0.0 } else { std::f64::consts::PI }).sin();
        let r = (a * a + b * b).sqrt().max(1e-9);
        (a / r).clamp(-1.0, 1.0).asin()
            + (res / r).clamp(-1.0, 1.0).asin()
            + 0.5 * std::f64::consts::PI
    }
}

#[allow(clippy::too_many_arguments)]
fn make_wave(
    width: f64,
    height: f64,
    offset: f64,
    period: f64,
    z_sin: f64,
    z_cos: f64,
    vertical: bool,
    flip: bool,
    origin_x: f64,
    origin_y: f64,
) -> Polyline {
    let mut pts = Vec::new();
    let dx = period * 0.25;
    let mut x = 0.0;
    while x <= width {
        let mut y = wave_y(x / period, z_sin, z_cos, vertical, flip) * period + offset;
        y = y.clamp(0.0, height);
        let (px, py) = if vertical { (y, x) } else { (x, y) };
        pts.push(Point::from_mm(origin_x + px, origin_y + py));
        x += dx;
    }
    pts
}
