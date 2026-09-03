//! Sparse infill patterns (classic Slic3r / Bambu set, minus Lightning/Adaptive).

use bambu_config::{InfillPattern, SliceSettings};
use bambu_geom::{offset_polygons, scale, Point, Polygon, Polyline};

use crate::clip::clip_polylines;

mod gyroid;
mod honeycomb;

pub fn generate(
    region: &[Polygon],
    settings: &SliceSettings,
    layer_index: usize,
    z_mm: f64,
) -> Vec<Polyline> {
    if region.is_empty() || settings.infill_density <= 0.0 {
        return Vec::new();
    }
    let spacing = settings.infill_spacing_mm();
    match settings.infill_pattern {
        InfillPattern::Rectilinear => rectilinear(region, spacing, layer_index),
        InfillPattern::Grid => {
            let mut lines = rectilinear(region, spacing, 0);
            lines.extend(vertical(region, spacing, 1));
            lines
        }
        InfillPattern::Concentric => concentric(region, spacing),
        InfillPattern::Gyroid => gyroid::fill(region, spacing, settings.infill_density, z_mm),
        InfillPattern::Honeycomb => honeycomb::fill(region, spacing, settings.infill_density),
    }
}

pub fn rectilinear(polygons: &[Polygon], spacing_mm: f64, layer_index: usize) -> Vec<Polyline> {
    scanlines(polygons, spacing_mm, layer_index, false)
}

fn vertical(polygons: &[Polygon], spacing_mm: f64, layer_index: usize) -> Vec<Polyline> {
    scanlines(polygons, spacing_mm, layer_index, true)
}

fn scanlines(
    polygons: &[Polygon],
    spacing_mm: f64,
    layer_index: usize,
    vertical: bool,
) -> Vec<Polyline> {
    if polygons.is_empty() || !spacing_mm.is_finite() || spacing_mm <= 0.0 {
        return Vec::new();
    }

    let mut min_v = i64::MAX;
    let mut max_v = i64::MIN;
    for poly in polygons {
        for p in poly {
            let v = if vertical { p.x } else { p.y };
            min_v = min_v.min(v);
            max_v = max_v.max(v);
        }
    }
    if min_v >= max_v {
        return Vec::new();
    }

    let spacing = scale(spacing_mm);
    if spacing <= 0 {
        return Vec::new();
    }

    let stagger = if layer_index.is_multiple_of(2) { 0 } else { spacing / 2 };
    let mut lines = Vec::new();
    let mut v = min_v + stagger;
    while v <= max_v {
        let mut us = collect_scanline_us(polygons, v, vertical);
        us.sort_unstable();
        let mut i = 0;
        while i + 1 < us.len() {
            let u0 = us[i];
            let u1 = us[i + 1];
            if u1 > u0 {
                let (a, b) = if vertical {
                    (Point::new(v, u0), Point::new(v, u1))
                } else {
                    (Point::new(u0, v), Point::new(u1, v))
                };
                lines.push(vec![a, b]);
            }
            i += 2;
        }
        v += spacing;
    }

    for (i, line) in lines.iter_mut().enumerate() {
        if i % 2 == 1 {
            line.reverse();
        }
    }
    lines
}

fn collect_scanline_us(polygons: &[Polygon], v: i64, vertical: bool) -> Vec<i64> {
    let mut us = Vec::new();
    for poly in polygons {
        let n = poly.len();
        if n < 3 {
            continue;
        }
        for i in 0..n {
            let a = poly[i];
            let b = poly[(i + 1) % n];
            let (a_u, a_v) = if vertical { (a.y, a.x) } else { (a.x, a.y) };
            let (b_u, b_v) = if vertical { (b.y, b.x) } else { (b.x, b.y) };
            if a_v == b_v {
                continue;
            }
            let (lo_u, lo_v, hi_u, hi_v) = if a_v < b_v {
                (a_u, a_v, b_u, b_v)
            } else {
                (b_u, b_v, a_u, a_v)
            };
            if v < lo_v || v >= hi_v {
                continue;
            }
            let dv = hi_v - lo_v;
            let t = (v - lo_v) as i128;
            let u = lo_u as i128 + t * (hi_u - lo_u) as i128 / dv as i128;
            us.push(u as i64);
        }
    }
    us
}

fn concentric(polygons: &[Polygon], spacing_mm: f64) -> Vec<Polyline> {
    let mut out = Vec::new();
    let mut current = polygons.to_vec();
    for _ in 0..64 {
        let rings = offset_polygons(&current, -spacing_mm);
        if rings.is_empty() {
            break;
        }
        out.extend(rings.iter().filter(|r| r.len() >= 3).cloned());
        current = rings;
    }
    out
}

pub(crate) fn bbox(polygons: &[Polygon]) -> Option<(Point, Point)> {
    let mut min = Point::new(i64::MAX, i64::MAX);
    let mut max = Point::new(i64::MIN, i64::MIN);
    let mut any = false;
    for poly in polygons {
        for p in poly {
            any = true;
            min.x = min.x.min(p.x);
            min.y = min.y.min(p.y);
            max.x = max.x.max(p.x);
            max.y = max.y.max(p.y);
        }
    }
    any.then_some((min, max))
}

pub(crate) fn clip_to_region(paths: Vec<Polyline>, region: &[Polygon]) -> Vec<Polyline> {
    clip_polylines(&paths, region)
}
