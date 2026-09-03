//! Rectilinear infill: horizontal scanlines clipped to polygons.

use bambu_geom::{scale, Point, Polygon, Polyline};

pub fn rectilinear(polygons: &[Polygon], spacing_mm: f64, layer_index: usize) -> Vec<Polyline> {
    if polygons.is_empty() || !spacing_mm.is_finite() || spacing_mm <= 0.0 {
        return Vec::new();
    }

    let mut min_x = i64::MAX;
    let mut max_x = i64::MIN;
    let mut min_y = i64::MAX;
    let mut max_y = i64::MIN;
    for poly in polygons {
        for p in poly {
            min_x = min_x.min(p.x);
            max_x = max_x.max(p.x);
            min_y = min_y.min(p.y);
            max_y = max_y.max(p.y);
        }
    }
    if min_x >= max_x || min_y >= max_y {
        return Vec::new();
    }

    let spacing = scale(spacing_mm);
    if spacing <= 0 {
        return Vec::new();
    }

    let stagger = if layer_index % 2 == 0 { 0 } else { spacing / 2 };
    let mut lines = Vec::new();
    let mut y = min_y + stagger;
    while y <= max_y {
        let mut xs = collect_scanline_xs(polygons, y);
        xs.sort_unstable();
        let mut i = 0;
        while i + 1 < xs.len() {
            let x0 = xs[i];
            let x1 = xs[i + 1];
            if x1 > x0 {
                lines.push(vec![Point::new(x0, y), Point::new(x1, y)]);
            }
            i += 2;
        }
        y += spacing;
    }

    // Alternate direction so adjacent lines chain more naturally.
    for (i, line) in lines.iter_mut().enumerate() {
        if i % 2 == 1 {
            line.reverse();
        }
    }

    lines
}

fn collect_scanline_xs(polygons: &[Polygon], y: i64) -> Vec<i64> {
    let mut xs = Vec::new();
    for poly in polygons {
        let n = poly.len();
        if n < 3 {
            continue;
        }
        for i in 0..n {
            let a = poly[i];
            let b = poly[(i + 1) % n];
            if a.y == b.y {
                continue;
            }
            let (lo, hi) = if a.y < b.y { (a, b) } else { (b, a) };
            if y < lo.y || y >= hi.y {
                continue;
            }
            let dy = hi.y - lo.y;
            let t = (y - lo.y) as i128;
            let x = lo.x as i128 + t * (hi.x - lo.x) as i128 / dy as i128;
            xs.push(x as i64);
        }
    }
    xs
}
