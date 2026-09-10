//! Sparse infill patterns (classic Slic3r / Bambu set).

use bambu_config::{InfillPattern, SliceSettings, SurfacePattern, LOOP_CLIPPING_OVER_NOZZLE};
use bambu_geom::{clip_end, offset_polygons, scale, Point, Polygon, Polyline};
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
    let spacing = settings.infill_spacing_for(layer_index == 0);
    match settings.infill_pattern {
        InfillPattern::Rectilinear => {
            rectilinear(region, spacing, layer_index, settings.infill_direction_deg)
        }
        InfillPattern::Grid => {
            let mut lines = rectilinear(region, spacing, 0, settings.infill_direction_deg);
            lines.extend(vertical(region, spacing, 1, settings.infill_direction_deg));
            lines
        }
        InfillPattern::Concentric => concentric(
            region,
            spacing,
            settings.nozzle_diameter_mm * LOOP_CLIPPING_OVER_NOZZLE,
        ),
        // C++ `FillGyroid::CorrectionAngle` (−45°) cancels the default 45° direction.
        InfillPattern::Gyroid => fill_at_angle(region, settings.infill_direction_deg - 45.0, |r| {
            gyroid::fill(r, spacing, settings.infill_density, z_mm)
        }),
        InfillPattern::Honeycomb => honeycomb::fill(
            region,
            spacing,
            settings.infill_density,
            layer_index,
            settings.infill_direction_deg,
        ),
        InfillPattern::Honeycomb3D => fill_at_angle(region, settings.infill_direction_deg, |r| {
            honeycomb3d::fill(r, spacing, settings.infill_density, z_mm)
        }),
        // Trees need every sparse layer; `prepare_infill` calls `generate_lightning`.
        InfillPattern::Lightning => Vec::new(),
        // Octree is built from the mesh in `prepare_infill`.
        InfillPattern::AdaptiveCubic | InfillPattern::SupportCubic => Vec::new(),
    }
}

pub fn rectilinear(
    polygons: &[Polygon],
    spacing_mm: f64,
    layer_index: usize,
    angle_deg: f64,
) -> Vec<Polyline> {
    scanlines(polygons, spacing_mm, layer_index, false, true, angle_deg)
}

/// 100% rectilinear fill, direction alternating each layer. Odd lines reverse (zig-zag).
pub fn solid(
    polygons: &[Polygon],
    spacing_mm: f64,
    layer_index: usize,
    angle_deg: f64,
) -> Vec<Polyline> {
    scanlines(
        polygons,
        spacing_mm,
        0,
        layer_index.is_multiple_of(2),
        true,
        angle_deg,
    )
}

/// Solid fill with every scanline in the same direction (C++ `params.monotonic`).
pub fn solid_monotonic(
    polygons: &[Polygon],
    spacing_mm: f64,
    layer_index: usize,
    angle_deg: f64,
) -> Vec<Polyline> {
    scanlines(
        polygons,
        spacing_mm,
        0,
        layer_index.is_multiple_of(2),
        false,
        angle_deg,
    )
}

pub fn solid_surface(
    polygons: &[Polygon],
    spacing_mm: f64,
    layer_index: usize,
    pattern: SurfacePattern,
    angle_deg: f64,
    nozzle_mm: f64,
) -> Vec<Polyline> {
    match pattern {
        SurfacePattern::Concentric => concentric(
            polygons,
            spacing_mm,
            nozzle_mm.max(0.0) * LOOP_CLIPPING_OVER_NOZZLE,
        ),
        SurfacePattern::Rectilinear => solid(polygons, spacing_mm, layer_index, angle_deg),
        SurfacePattern::Monotonic | SurfacePattern::MonotonicLine => {
            solid_monotonic(polygons, spacing_mm, layer_index, angle_deg)
        }
    }
}

fn vertical(
    polygons: &[Polygon],
    spacing_mm: f64,
    layer_index: usize,
    angle_deg: f64,
) -> Vec<Polyline> {
    scanlines(polygons, spacing_mm, layer_index, true, true, angle_deg)
}

fn scanlines(
    polygons: &[Polygon],
    spacing_mm: f64,
    layer_index: usize,
    vertical: bool,
    zigzag: bool,
    angle_deg: f64,
) -> Vec<Polyline> {
    if polygons.is_empty() || !spacing_mm.is_finite() || spacing_mm <= 0.0 {
        return Vec::new();
    }

    let angle = angle_deg.to_radians();
    let rotate = angle.abs() >= 1e-12;
    let (cos_a, sin_a) = angle.sin_cos();
    let rotated_storage;
    let polygons = if rotate {
        rotated_storage = rotate_polygons(polygons, cos_a, -sin_a);
        rotated_storage.as_slice()
    } else {
        polygons
    };

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
    if rotate {
        for line in &mut lines {
            for p in line {
                *p = rotate_point(*p, cos_a, sin_a);
            }
        }
    }
    lines
}

/// Rotate the region by `-angle`, fill, then rotate paths back (`FillGyroid` / `Fill3DHoneycomb`).
fn fill_at_angle(
    region: &[Polygon],
    angle_deg: f64,
    fill: impl FnOnce(&[Polygon]) -> Vec<Polyline>,
) -> Vec<Polyline> {
    let angle = angle_deg.to_radians();
    if angle.abs() < 1e-12 {
        return fill(region);
    }
    let (cos_a, sin_a) = angle.sin_cos();
    let rotated = rotate_polygons(region, cos_a, -sin_a);
    let mut lines = fill(&rotated);
    for line in &mut lines {
        for p in line {
            *p = rotate_point(*p, cos_a, sin_a);
        }
    }
    lines
}

fn rotate_polygons(polygons: &[Polygon], cos_a: f64, sin_a: f64) -> Vec<Polygon> {
    polygons
        .iter()
        .map(|poly| {
            poly.iter()
                .map(|p| rotate_point(*p, cos_a, sin_a))
                .collect()
        })
        .collect()
}

fn rotate_point(p: Point, cos_a: f64, sin_a: f64) -> Point {
    let x = p.x as f64;
    let y = p.y as f64;
    Point::new(
        (x * cos_a - y * sin_a).round() as i64,
        (x * sin_a + y * cos_a).round() as i64,
    )
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

pub(crate) fn concentric(polygons: &[Polygon], spacing_mm: f64, clip_mm: f64) -> Vec<Polyline> {
    let mut out = Vec::new();
    let mut current = polygons.to_vec();
    for _ in 0..64 {
        let rings = offset_polygons(&current, -spacing_mm);
        if rings.is_empty() {
            break;
        }
        for mut ring in rings.iter().filter(|r| r.len() >= 3).cloned() {
            if let (Some(&first), Some(&last)) = (ring.first(), ring.last()) {
                if first != last {
                    ring.push(first);
                }
            }
            // C++ `FillConcentric` `clip_end(loop_clipping)` so G-code sees an open loop.
            let clipped = clip_end(&ring, clip_mm);
            if clipped.len() >= 2 {
                out.push(clipped);
            }
        }
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
    use bambu_config::{InfillPattern, SliceSettings};
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

    fn square_mm(size: f64) -> Vec<Polygon> {
        vec![vec![
            Point::from_mm(0.0, 0.0),
            Point::from_mm(size, 0.0),
            Point::from_mm(size, size),
            Point::from_mm(0.0, size),
        ]]
    }

    fn mean_abs_dir(paths: &[Polyline]) -> (f64, f64) {
        let mut ax = 0.0;
        let mut ay = 0.0;
        let mut n = 0.0;
        for path in paths {
            if path.len() < 2 {
                continue;
            }
            let (x0, y0) = path[0].to_mm();
            let (x1, y1) = path[path.len() - 1].to_mm();
            let dx = x1 - x0;
            let dy = y1 - y0;
            let len = (dx * dx + dy * dy).sqrt();
            if len < 1.0 {
                continue;
            }
            ax += dx.abs() / len;
            ay += dy.abs() / len;
            n += 1.0;
        }
        (ax / n, ay / n)
    }

    #[test]
    fn rectilinear_follows_infill_direction() {
        let region = square_mm(20.0);
        let along_x = rectilinear(&region, 1.5, 0, 0.0);
        let diagonal = rectilinear(&region, 1.5, 0, 45.0);
        assert!(!along_x.is_empty());
        assert!(!diagonal.is_empty());
        let (x0, y0) = mean_abs_dir(&along_x);
        let (x45, y45) = mean_abs_dir(&diagonal);
        assert!(x0 > 0.95, "0° lines should run along X, got ({x0}, {y0})");
        assert!(y0 < 0.1, "0° lines should be horizontal, got ({x0}, {y0})");
        assert!(
            (x45 - y45).abs() < 0.2,
            "45° lines should have similar |dx| and |dy|, got ({x45}, {y45})"
        );
    }

    #[test]
    fn gyroid_default_45_matches_unrotated() {
        let region = square_mm(20.0);
        let mut settings = SliceSettings::default();
        settings.infill_pattern = InfillPattern::Gyroid;
        settings.infill_density = 0.15;
        settings.infill_direction_deg = 45.0;
        let via_generate = generate(&region, &settings, 0, 1.0);
        let direct = gyroid::fill(
            &region,
            settings.infill_spacing_mm(),
            settings.infill_density,
            1.0,
        );
        assert!(!via_generate.is_empty());
        assert_eq!(
            via_generate, direct,
            "C++ CorrectionAngle −45° should cancel default infill_direction"
        );
    }

    #[test]
    fn gyroid_follows_infill_direction() {
        let region = square_mm(20.0);
        let mut settings = SliceSettings::default();
        settings.infill_pattern = InfillPattern::Gyroid;
        settings.infill_density = 0.15;
        settings.infill_direction_deg = 45.0;
        let at_45 = generate(&region, &settings, 0, 1.0);
        settings.infill_direction_deg = 0.0;
        let at_0 = generate(&region, &settings, 0, 1.0);
        assert!(!at_45.is_empty());
        assert!(!at_0.is_empty());
        assert_ne!(at_45, at_0);
        let (x45, y45) = mean_abs_dir(&at_45);
        let (x0, y0) = mean_abs_dir(&at_0);
        assert!(
            (x45 - y45).abs() > 0.25,
            "default 45° gyroid should stay axis-aligned, got ({x45}, {y45})"
        );
        assert!(
            (x0 - y0).abs() < 0.35,
            "0° gyroid should be diagonal after −45° correction, got ({x0}, {y0})"
        );
    }

    #[test]
    fn concentric_closes_loop_then_clips_nozzle_gap() {
        let region = square_mm(20.0);
        let clip = 0.4 * LOOP_CLIPPING_OVER_NOZZLE;
        let paths = concentric(&region, 2.0, clip);
        assert!(!paths.is_empty());
        let ring = &paths[0];
        assert!(ring.len() >= 4);
        let first = ring[0];
        let last = *ring.last().unwrap();
        assert_ne!(first, last);
        let gap = first.distance_mm(last);
        assert!(
            (gap - clip).abs() < 0.02,
            "C++ loop_clipping is 0.15×nozzle, got {gap} want {clip}"
        );
    }
}
