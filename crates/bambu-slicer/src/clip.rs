//! Point-in-polygon and open-polyline clipping.

use bambu_geom::{Point, Polygon, Polyline};

pub fn point_in_polygon(poly: &[Point], p: Point) -> bool {
    let n = poly.len();
    if n < 3 {
        return false;
    }
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let a = poly[i];
        let b = poly[j];
        if (a.y > p.y) != (b.y > p.y) {
            let dy = b.y - a.y;
            if dy != 0 {
                let xints = a.x as i128 + (b.x - a.x) as i128 * (p.y - a.y) as i128 / dy as i128;
                if (p.x as i128) < xints {
                    inside = !inside;
                }
            }
        }
        j = i;
    }
    inside
}

pub fn point_in_polygons(p: Point, polygons: &[Polygon]) -> bool {
    let mut inside = false;
    for poly in polygons {
        if point_in_polygon(poly, p) {
            inside = !inside;
        }
    }
    inside
}

fn lerp(a: Point, b: Point, t: f64) -> Point {
    Point::new(
        a.x + ((b.x - a.x) as f64 * t).round() as i64,
        a.y + ((b.y - a.y) as f64 * t).round() as i64,
    )
}

/// Keep the portions of an open polyline that lie inside `polygons`.
pub fn clip_polyline(path: &[Point], polygons: &[Polygon]) -> Vec<Polyline> {
    if path.len() < 2 || polygons.is_empty() {
        return Vec::new();
    }

    let mut densified = Vec::new();
    for w in path.windows(2) {
        densified.push(w[0]);
        let dist2 = (w[1].x - w[0].x).abs().max((w[1].y - w[0].y).abs());
        let steps = (dist2 / 200_000).clamp(1, 32); // ~0.2mm
        for s in 1..steps {
            densified.push(lerp(w[0], w[1], s as f64 / steps as f64));
        }
    }
    densified.push(*path.last().unwrap());

    let flags: Vec<bool> = densified
        .iter()
        .map(|p| point_in_polygons(*p, polygons))
        .collect();

    let mut out = Vec::new();
    let mut current = Vec::new();
    for (p, inside) in densified.into_iter().zip(flags) {
        if inside {
            current.push(p);
        } else if current.len() >= 2 {
            out.push(std::mem::take(&mut current));
        } else {
            current.clear();
        }
    }
    if current.len() >= 2 {
        out.push(current);
    }
    out
}

pub fn clip_polylines(paths: &[Polyline], polygons: &[Polygon]) -> Vec<Polyline> {
    paths
        .iter()
        .flat_map(|p| clip_polyline(p, polygons))
        .collect()
}
