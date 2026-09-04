//! C++ `TimelapsePosPicker` for by-layer, single-object prints.
//!
//! Skips wipe-tower keep-out, by-object rod limits, and path-collision ranking.

use bambu_config::SliceSettings;
use bambu_geom::{
    difference_polygons, intersect_polygons, offset_polygons, union_polygons, unscale, Point,
    Polygon,
};
use bambu_slicer::{point_in_polygons, Layer, SliceResult};

/// C++ `DefaultTimelapsePos` after `unscale_` (integer millimeters).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimelapsePos {
    pub x: i32,
    pub y: i32,
}

impl TimelapsePos {
    pub const ORIGIN: Self = Self { x: 0, y: 0 };
}

const FILTER_THRESHOLD_MM: f64 = 5.0;
const CANDIDATE_SEGMENT_MM: f64 = 5.0;
const CAMERA_WEIGHT: f64 = 1.0 / 3.0;

/// C++ `pick_pos` for traditional (per-layer) timelapse.
pub fn pick_timelapse_pos(
    settings: &SliceSettings,
    object_min: (f64, f64),
    object_max: (f64, f64),
    farthest: Option<Point>,
) -> TimelapsePos {
    let Some(bed) = bed_polygon(settings) else {
        return TimelapsePos::ORIGIN;
    };
    let mut printable = vec![bed];
    if settings.bed_exclude_area.len() >= 3 {
        printable = difference_polygons(&printable, &[mm_polygon(&settings.bed_exclude_area)]);
    }
    let extruder_i = settings.filament_extruder_index(0);
    if let Some(area) = settings.extruder_printable_areas.get(extruder_i) {
        if area.len() >= 3 {
            printable = intersect_polygons(&printable, &[mm_polygon(area)]);
        }
    }
    if printable.is_empty() {
        return TimelapsePos::ORIGIN;
    }

    let radius = settings.extruder_clearance_max_radius_mm.max(0.0) * 0.5;
    let object = expand_bbox(object_min, object_max, radius);
    let camera = camera_limit(object_min, object_max, radius);
    let mut unplacable = Vec::new();
    if object.len() >= 3 {
        unplacable.push(object);
    }
    if camera.len() >= 3 {
        unplacable.push(camera);
    }
    let unplacable = union_polygons(&unplacable);
    let safe = difference_polygons(&printable, &unplacable);
    let safe = opening_polygons(&safe, FILTER_THRESHOLD_MM);
    if safe.is_empty() {
        return TimelapsePos::ORIGIN;
    }

    let center = Point::from_mm(
        (object_min.0 + object_max.0) * 0.5,
        (object_min.1 + object_max.1) * 0.5,
    );
    if point_in_polygons(center, &safe) {
        return to_mm_int(center);
    }
    pick_nearest(&safe, center, farthest).unwrap_or(TimelapsePos::ORIGIN)
}

/// Mesh XY AABB from layer contours (C++ instance bbox, not brim).
pub fn object_xy_bbox(sliced: &SliceResult) -> ((f64, f64), (f64, f64)) {
    let mut min_x = f64::INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    for layer in &sliced.layers {
        for poly in &layer.contours {
            for p in poly {
                let (x, y) = p.to_mm();
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
            }
        }
    }
    if !min_x.is_finite() {
        return ((0.0, 0.0), (0.0, 0.0));
    }
    ((min_x, min_y), (max_x, max_y))
}

/// C++ `compute_farthest_point`: prefer outer-wall endpoints, else infill/shells.
pub fn farthest_layer_point(layer: &Layer) -> Option<Point> {
    let mut best: Option<(i64, Point)> = None;
    update_farthest(&layer.outer_walls, &mut best);
    if best.is_none() {
        update_farthest(&layer.infill, &mut best);
        update_farthest(&layer.solid_infill, &mut best);
        update_farthest(&layer.top_surface, &mut best);
        update_farthest(&layer.bottom_surface, &mut best);
        update_farthest(&layer.support, &mut best);
        update_farthest(&layer.support_interface, &mut best);
    }
    best.map(|(_, p)| p)
}

fn update_farthest(paths: &[Vec<Point>], best: &mut Option<(i64, Point)>) {
    for path in paths {
        for &p in path {
            let dsq =
                p.x.saturating_mul(p.x)
                    .saturating_add(p.y.saturating_mul(p.y));
            match *best {
                Some((prev, _)) if dsq <= prev => {}
                _ => *best = Some((dsq, p)),
            }
        }
    }
}

fn bed_polygon(settings: &SliceSettings) -> Option<Polygon> {
    if settings.printable_area.len() >= 3 {
        return Some(mm_polygon(&settings.printable_area));
    }
    if settings.bed_bbox_valid {
        return Some(vec![
            Point::from_mm(settings.bed_min_x, settings.bed_min_y),
            Point::from_mm(settings.bed_max_x, settings.bed_min_y),
            Point::from_mm(settings.bed_max_x, settings.bed_max_y),
            Point::from_mm(settings.bed_min_x, settings.bed_max_y),
        ]);
    }
    None
}

fn mm_polygon(pts: &[(f64, f64)]) -> Polygon {
    pts.iter().map(|&(x, y)| Point::from_mm(x, y)).collect()
}

fn expand_bbox(min: (f64, f64), max: (f64, f64), radius_mm: f64) -> Polygon {
    let rect = vec![
        Point::from_mm(min.0, min.1),
        Point::from_mm(max.0, min.1),
        Point::from_mm(max.0, max.1),
        Point::from_mm(min.0, max.1),
    ];
    offset_polygons(&[rect], radius_mm)
        .into_iter()
        .next()
        .unwrap_or_else(|| {
            vec![
                Point::from_mm(min.0, min.1),
                Point::from_mm(max.0, min.1),
                Point::from_mm(max.0, max.1),
                Point::from_mm(min.0, max.1),
            ]
        })
}

fn camera_limit(min: (f64, f64), max: (f64, f64), radius_mm: f64) -> Polygon {
    let inflate = std::f64::consts::SQRT_2 * radius_mm;
    let min_x = (min.0 - inflate).max(0.0);
    let min_y = (min.1 - inflate).max(0.0);
    let max_x = (max.0 + inflate).max(0.0);
    let max_y = (max.1 + inflate).max(0.0);
    vec![
        Point::from_mm(0.0, 0.0),
        Point::from_mm(max_x, min_y),
        Point::from_mm(max_x, max_y),
        Point::from_mm(min_x, max_y),
    ]
}

fn opening_polygons(polygons: &[Polygon], delta_mm: f64) -> Vec<Polygon> {
    offset_polygons(&offset_polygons(polygons, -delta_mm), delta_mm)
}

fn pick_nearest(safe: &[Polygon], curr: Point, farthest: Option<Point>) -> Option<TimelapsePos> {
    let seg = Point::from_mm(CANDIDATE_SEGMENT_MM, 0.0).x;
    let mut best_pen = f64::INFINITY;
    let mut best = None;
    for poly in safe {
        let n = poly.len();
        if n < 2 {
            continue;
        }
        for i in 0..n {
            let a = poly[i];
            let b = poly[(i + 1) % n];
            let dx = b.x - a.x;
            let dy = b.y - a.y;
            let length_l1 = dx.abs() + dy.abs();
            if length_l1 < seg {
                consider(a, curr, farthest, &mut best_pen, &mut best);
                continue;
            }
            let steps = length_l1 / seg;
            let step_x = (dx as f64 * seg as f64 / length_l1 as f64).round() as i64;
            let step_y = (dy as f64 * seg as f64 / length_l1 as f64).round() as i64;
            for k in 0..=steps {
                let p = Point::new(a.x + step_x * k, a.y + step_y * k);
                consider(p, curr, farthest, &mut best_pen, &mut best);
            }
        }
    }
    best
}

fn consider(
    candidate: Point,
    curr: Point,
    farthest: Option<Point>,
    best_pen: &mut f64,
    best: &mut Option<TimelapsePos>,
) {
    let pen = penalty(curr, candidate, farthest);
    if pen < *best_pen {
        *best_pen = pen;
        *best = Some(to_mm_int(candidate));
    }
}

fn penalty(curr: Point, candidate: Point, farthest: Option<Point>) -> f64 {
    let l1 = |a: Point, b: Point| (a.x - b.x).abs() as f64 + (a.y - b.y).abs() as f64;
    if let Some(far) = farthest {
        return l1(far, candidate);
    }
    l1(curr, candidate) - CAMERA_WEIGHT * l1(Point::new(0, 0), candidate)
}

fn to_mm_int(p: Point) -> TimelapsePos {
    TimelapsePos {
        x: unscale(p.x) as i32,
        y: unscale(p.y) as i32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bambu_config::SliceSettings;

    fn h2c_like() -> SliceSettings {
        let mut s = SliceSettings::default();
        s.printable_area = vec![(0.0, 0.0), (330.0, 0.0), (330.0, 320.0), (0.0, 320.0)];
        s.extruder_printable_areas = vec![
            vec![(0.0, 0.0), (325.0, 0.0), (325.0, 320.0), (0.0, 320.0)],
            vec![(25.0, 0.0), (330.0, 0.0), (330.0, 320.0), (25.0, 320.0)],
        ];
        s.extruder_clearance_max_radius_mm = 96.0;
        s.filament_map = vec![1];
        s
    }

    #[test]
    fn cube_on_h2c_picks_non_origin() {
        let pos = pick_timelapse_pos(&h2c_like(), (0.0, 0.0), (20.0, 20.0), None);
        assert_ne!(
            pos,
            TimelapsePos::ORIGIN,
            "20 mm cube on H2C bed should have a safe corner, got {pos:?}"
        );
        assert!(pos.x >= 0 && pos.y >= 0, "{pos:?}");
        assert!(pos.x <= 330 && pos.y <= 320, "{pos:?}");
    }

    #[test]
    fn missing_bed_is_origin() {
        let pos = pick_timelapse_pos(&SliceSettings::default(), (0.0, 0.0), (20.0, 20.0), None);
        assert_eq!(pos, TimelapsePos::ORIGIN);
    }

    #[test]
    fn farthest_point_pulls_toward_object() {
        let settings = h2c_like();
        let far = Point::from_mm(20.0, 20.0);
        let with = pick_timelapse_pos(&settings, (0.0, 0.0), (20.0, 20.0), Some(far));
        let without = pick_timelapse_pos(&settings, (0.0, 0.0), (20.0, 20.0), None);
        assert_ne!(with, TimelapsePos::ORIGIN);
        let d_with = (with.x - 20).abs() + (with.y - 20).abs();
        let d_without = (without.x - 20).abs() + (without.y - 20).abs();
        assert!(
            d_with <= d_without,
            "L1-to-farthest should sit closer to (20,20) ({with:?} vs {without:?})"
        );
    }
}
