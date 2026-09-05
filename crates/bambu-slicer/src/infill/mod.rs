//! Sparse infill patterns (classic Slic3r / Bambu set).

use bambu_config::{InfillPattern, SliceSettings, SurfacePattern};
use bambu_geom::{offset_polygons, scale, Point, Polygon, Polyline};
use wide::{i64x4, CmpLt};

use crate::clip::clip_polylines;

pub(crate) mod adaptive;
mod gyroid;
mod honeycomb;
mod honeycomb3d;
mod lightning;

pub(crate) use lightning::generate_layers as generate_lightning;

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
        InfillPattern::Honeycomb => {
            honeycomb::fill(region, spacing, settings.infill_density, layer_index)
        }
        InfillPattern::Honeycomb3D => {
            honeycomb3d::fill(region, spacing, settings.infill_density, z_mm)
        }
        // Trees need every sparse layer; `prepare_infill` calls `generate_lightning`.
        InfillPattern::Lightning => Vec::new(),
        // Octree is built from the mesh in `prepare_infill`.
        InfillPattern::AdaptiveCubic | InfillPattern::SupportCubic => Vec::new(),
    }
}

pub fn rectilinear(polygons: &[Polygon], spacing_mm: f64, layer_index: usize) -> Vec<Polyline> {
    scanlines(polygons, spacing_mm, layer_index, false, true)
}

/// 100% rectilinear fill, direction alternating each layer. Odd lines reverse (zig-zag).
pub fn solid(polygons: &[Polygon], spacing_mm: f64, layer_index: usize) -> Vec<Polyline> {
    scanlines(polygons, spacing_mm, 0, layer_index.is_multiple_of(2), true)
}

/// Solid fill with every scanline in the same direction (C++ `params.monotonic`).
pub fn solid_monotonic(polygons: &[Polygon], spacing_mm: f64, layer_index: usize) -> Vec<Polyline> {
    scanlines(
        polygons,
        spacing_mm,
        0,
        layer_index.is_multiple_of(2),
        false,
    )
}

pub fn solid_surface(
    polygons: &[Polygon],
    spacing_mm: f64,
    layer_index: usize,
    pattern: SurfacePattern,
) -> Vec<Polyline> {
    match pattern {
        SurfacePattern::Concentric => concentric(polygons, spacing_mm),
        SurfacePattern::Rectilinear => solid(polygons, spacing_mm, layer_index),
        SurfacePattern::Monotonic | SurfacePattern::MonotonicLine => {
            solid_monotonic(polygons, spacing_mm, layer_index)
        }
    }
}

fn vertical(polygons: &[Polygon], spacing_mm: f64, layer_index: usize) -> Vec<Polyline> {
    scanlines(polygons, spacing_mm, layer_index, true, true)
}

fn scanlines(
    polygons: &[Polygon],
    spacing_mm: f64,
    layer_index: usize,
    vertical: bool,
    zigzag: bool,
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

    let stagger = if layer_index.is_multiple_of(2) {
        0
    } else {
        spacing / 2
    };
    let edges = collect_scan_edges(polygons, vertical);
    let mut lines = Vec::new();
    let mut v = min_v + stagger;
    while v <= max_v {
        let mut us = collect_scanline_us(&edges, v);
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

    if zigzag {
        for (i, line) in lines.iter_mut().enumerate() {
            if i % 2 == 1 {
                line.reverse();
            }
        }
    }
    lines
}

#[derive(Clone, Copy)]
struct ScanEdge {
    lo_u: i64,
    lo_v: i64,
    hi_u: i64,
    hi_v: i64,
}

fn collect_scan_edges(polygons: &[Polygon], vertical: bool) -> Vec<ScanEdge> {
    let mut edges = Vec::new();
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
            edges.push(ScanEdge {
                lo_u,
                lo_v,
                hi_u,
                hi_v,
            });
        }
    }
    edges
}

fn scan_u(edge: ScanEdge, v: i64) -> i64 {
    let dv = edge.hi_v - edge.lo_v;
    let t = (v - edge.lo_v) as i128;
    (edge.lo_u as i128 + t * (edge.hi_u - edge.lo_u) as i128 / dv as i128) as i64
}

fn collect_scanline_us(edges: &[ScanEdge], v: i64) -> Vec<i64> {
    let mut us = Vec::new();
    let (chunks, rem) = edges.as_chunks::<4>();
    for chunk in chunks {
        let lo_v = i64x4::from([chunk[0].lo_v, chunk[1].lo_v, chunk[2].lo_v, chunk[3].lo_v]);
        let hi_v = i64x4::from([chunk[0].hi_v, chunk[1].hi_v, chunk[2].hi_v, chunk[3].hi_v]);
        let vv = i64x4::splat(v);
        let hit = !vv.cmp_lt(lo_v) & vv.cmp_lt(hi_v);
        let bits: [i64; 4] = hit.to_array();
        for (edge, bit) in chunk.iter().zip(bits) {
            if bit != 0 {
                us.push(scan_u(*edge, v));
            }
        }
    }
    for edge in rem {
        if v >= edge.lo_v && v < edge.hi_v {
            us.push(scan_u(*edge, v));
        }
    }
    us
}

pub(crate) fn concentric(polygons: &[Polygon], spacing_mm: f64) -> Vec<Polyline> {
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

#[cfg(test)]
fn collect_scanline_us_scalar(edges: &[ScanEdge], v: i64) -> Vec<i64> {
    let mut us = Vec::new();
    for edge in edges {
        if v >= edge.lo_v && v < edge.hi_v {
            us.push(scan_u(*edge, v));
        }
    }
    us
}

#[cfg(test)]
mod tests {
    use super::*;
    use bambu_geom::scale;

    #[test]
    fn simd_scanline_cull_matches_scalar() {
        let s = scale(1.0);
        let hex = vec![
            Point::new(5 * s, 0),
            Point::new(10 * s, 2 * s),
            Point::new(10 * s, 8 * s),
            Point::new(5 * s, 10 * s),
            Point::new(0, 8 * s),
            Point::new(0, 2 * s),
        ];
        let edges = collect_scan_edges(&[hex], false);
        assert!(edges.len() >= 4);
        for v in [0, s, 5 * s, 10 * s - 1] {
            assert_eq!(
                collect_scanline_us(&edges, v),
                collect_scanline_us_scalar(&edges, v),
                "v={v}"
            );
        }
    }
}
