//! Classic leftover gap fill: C++ `opening_ex` / too-wide `offset2_ex`, then an
//! open centerline instead of walking a closed leftover ring.

use bambu_geom::{difference_polygons, offset_polygons, Point, Polygon, Polyline};

/// C++ `ClipperSafetyOffset` in millimetres (10 scaled units).
const SAFETY_OFFSET_MM: f64 = 10.0 / 1_000_000.0;

fn offset2(polygons: &[Polygon], delta1: f64, delta2: f64) -> Vec<Polygon> {
    offset_polygons(&offset_polygons(polygons, delta1), delta2)
}

/// C++ `diff_ex(opening_ex(gaps, min/2), offset2_ex(gaps, -max/2, max/2))`.
pub(crate) fn collapse_gap_areas(gaps: &[Polygon], min_width: f64, max_width: f64) -> Vec<Polygon> {
    if gaps.is_empty() || min_width <= 0.0 {
        return Vec::new();
    }
    let opened = offset2(gaps, -min_width * 0.5, min_width * 0.5);
    if opened.is_empty() {
        return Vec::new();
    }
    if max_width <= min_width {
        return opened;
    }
    let too_wide = offset2(gaps, -max_width * 0.5, max_width * 0.5 + SAFETY_OFFSET_MM);
    if too_wide.is_empty() {
        opened
    } else {
        difference_polygons(&opened, &too_wide)
    }
}

pub(crate) fn is_thin_corridor(poly: &[Point], max_width: f64) -> bool {
    let Some((dx, dy)) = extents_mm(poly) else {
        return false;
    };
    let short = dx.min(dy);
    let long = dx.max(dy);
    short > 1e-6 && short < max_width * 1.5 && long / short >= 1.8
}

/// Midline of an elongated leftover, sampled along the longer bbox axis.
pub(crate) fn open_centerline(poly: &[Point]) -> Option<Polyline> {
    let (min_x, min_y, max_x, max_y) = bbox_mm(poly)?;
    let dx = max_x - min_x;
    let dy = max_y - min_y;
    let along_x = dx >= dy;
    let span = if along_x { dx } else { dy };
    if span < 1e-4 {
        return None;
    }
    let step = 0.2_f64.min(span / 4.0).max(0.05);
    let n = ((span / step).ceil() as usize).max(2);
    let mut mids = Vec::with_capacity(n + 1);
    for i in 0..=n {
        let t = i as f64 / n as f64;
        let mid = if along_x {
            let x = min_x + t * dx;
            longest_span_mid(scan_vertical(poly, x)).map(|y| Point::from_mm(x, y))
        } else {
            let y = min_y + t * dy;
            longest_span_mid(scan_horizontal(poly, y)).map(|x| Point::from_mm(x, y))
        };
        if let Some(p) = mid {
            if mids
                .last()
                .is_none_or(|prev: &Point| prev.distance_mm(p) > 0.04)
            {
                mids.push(p);
            }
        }
    }
    (mids.len() >= 2).then_some(mids)
}

fn extents_mm(poly: &[Point]) -> Option<(f64, f64)> {
    let (min_x, min_y, max_x, max_y) = bbox_mm(poly)?;
    Some((max_x - min_x, max_y - min_y))
}

fn bbox_mm(poly: &[Point]) -> Option<(f64, f64, f64, f64)> {
    if poly.len() < 3 {
        return None;
    }
    let mut min_x = f64::INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    for p in poly {
        let (x, y) = p.to_mm();
        min_x = min_x.min(x);
        min_y = min_y.min(y);
        max_x = max_x.max(x);
        max_y = max_y.max(y);
    }
    Some((min_x, min_y, max_x, max_y))
}

fn longest_span_mid(mut hits: Vec<f64>) -> Option<f64> {
    if hits.len() < 2 {
        return None;
    }
    hits.sort_by(|a, b| a.total_cmp(b));
    let mut best_a = hits[0];
    let mut best_b = hits[1];
    let mut best_len = best_b - best_a;
    let mut i = 0;
    while i + 1 < hits.len() {
        let a = hits[i];
        let b = hits[i + 1];
        let len = b - a;
        if len > best_len {
            best_len = len;
            best_a = a;
            best_b = b;
        }
        i += 2;
    }
    (best_len > 1e-6).then_some(0.5 * (best_a + best_b))
}

fn scan_vertical(poly: &[Point], x_mm: f64) -> Vec<f64> {
    let x = Point::from_mm(x_mm, 0.0).x;
    let mut ys = Vec::new();
    let n = poly.len();
    for i in 0..n {
        let a = poly[i];
        let b = poly[(i + 1) % n];
        if a.x == b.x {
            continue;
        }
        if (a.x > x) == (b.x > x) {
            continue;
        }
        let dy = b.y - a.y;
        let y = a.y as f64 + dy as f64 * (x - a.x) as f64 / (b.x - a.x) as f64;
        ys.push(y / bambu_geom::SCALING_FACTOR_F64);
    }
    ys
}

fn scan_horizontal(poly: &[Point], y_mm: f64) -> Vec<f64> {
    let y = Point::from_mm(0.0, y_mm).y;
    let mut xs = Vec::new();
    let n = poly.len();
    for i in 0..n {
        let a = poly[i];
        let b = poly[(i + 1) % n];
        if a.y == b.y {
            continue;
        }
        if (a.y > y) == (b.y > y) {
            continue;
        }
        let dx = b.x - a.x;
        let x = a.x as f64 + dx as f64 * (y - a.y) as f64 / (b.y - a.y) as f64;
        xs.push(x / bambu_geom::SCALING_FACTOR_F64);
    }
    xs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(width_mm: f64, height_mm: f64) -> Polygon {
        vec![
            Point::from_mm(0.0, 0.0),
            Point::from_mm(width_mm, 0.0),
            Point::from_mm(width_mm, height_mm),
            Point::from_mm(0.0, height_mm),
        ]
    }

    #[test]
    fn opening_drops_dust() {
        let dust = rect(0.02, 0.02);
        let keep = collapse_gap_areas(&[dust], 0.05, 0.84);
        assert!(keep.is_empty());
    }

    #[test]
    fn too_wide_region_is_removed() {
        let fat = rect(10.0, 10.0);
        let keep = collapse_gap_areas(&[fat], 0.05, 0.84);
        assert!(keep.is_empty(), "10 mm square is wider than max gap");
    }

    #[test]
    fn thin_strip_keeps_an_open_midline() {
        let strip = rect(0.3, 20.0);
        let keep = collapse_gap_areas(&[strip.clone()], 0.05, 0.84);
        assert_eq!(keep.len(), 1);
        assert!(is_thin_corridor(&keep[0], 0.84));
        let path = open_centerline(&keep[0]).expect("midline");
        let len: f64 = path.windows(2).map(|w| w[0].distance_mm(w[1])).sum();
        assert!(len > 15.0, "len={len}");
        assert!(path[0].distance_mm(*path.last().unwrap()) > 15.0);
    }
}
