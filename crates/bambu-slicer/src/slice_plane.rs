//! Triangle–plane intersection and loop stitching.

use std::collections::HashMap;

use bambu_geom::{scale, Point, Polygon, TriangleMesh};
use glam::Vec3;

pub fn slice_at_z(mesh: &TriangleMesh, z: f32) -> Vec<Polygon> {
    let mut segments: Vec<(Point, Point)> = Vec::new();
    for idx in &mesh.indices {
        let [a, b, c] = mesh.triangle(*idx);
        if let Some((p0, p1)) = triangle_plane_segment(a, b, c, z) {
            if p0 != p1 {
                segments.push((p0, p1));
            }
        }
    }
    loops_from_segments(&segments)
}

/// Convert plane-hit segments into closed contours. Used by the CPU slicer and
/// by the Vulkan compute readback path.
pub fn loops_from_segments(segments: &[(Point, Point)]) -> Vec<Polygon> {
    stitch_loops(segments)
}

pub fn point_from_xy_mm(x: f64, y: f64) -> Point {
    Point::new(snap(scale(x)), snap(scale(y)))
}

fn triangle_plane_segment(a: Vec3, b: Vec3, c: Vec3, z: f32) -> Option<(Point, Point)> {
    let z = z as f64;
    let a = dvec(a);
    let b = dvec(b);
    let c = dvec(c);
    let mut hits = Vec::new();
    push_edge_hit(&mut hits, a, b, z);
    push_edge_hit(&mut hits, b, c, z);
    push_edge_hit(&mut hits, c, a, z);
    hits.dedup();
    if hits.len() >= 2 {
        Some((hits[0], hits[1]))
    } else {
        None
    }
}

fn dvec(v: Vec3) -> [f64; 3] {
    [v.x as f64, v.y as f64, v.z as f64]
}

fn push_edge_hit(hits: &mut Vec<Point>, a: [f64; 3], b: [f64; 3], z: f64) {
    let da = a[2] - z;
    let db = b[2] - z;
    const EPS: f64 = 1e-9;
    if da.abs() <= EPS && db.abs() <= EPS {
        hits.push(to_point(a));
        hits.push(to_point(b));
        return;
    }
    if da.abs() <= EPS {
        hits.push(to_point(a));
        return;
    }
    if db.abs() <= EPS {
        hits.push(to_point(b));
        return;
    }
    if da * db < 0.0 {
        let t = da / (da - db);
        let p = [
            a[0] + (b[0] - a[0]) * t,
            a[1] + (b[1] - a[1]) * t,
            a[2] + (b[2] - a[2]) * t,
        ];
        hits.push(to_point(p));
    }
}

fn to_point(v: [f64; 3]) -> Point {
    Point::new(snap(scale(v[0])), snap(scale(v[1])))
}

/// Quantize scaled coordinates so f32/f64 interpolation noise still welds.
fn snap(v: i64) -> i64 {
    const SNAP: i64 = 32;
    let half = SNAP / 2;
    if v >= 0 {
        (v + half) / SNAP * SNAP
    } else {
        (v - half) / SNAP * SNAP
    }
}

fn stitch_loops(segments: &[(Point, Point)]) -> Vec<Polygon> {
    if segments.is_empty() {
        return Vec::new();
    }

    let mut adj: HashMap<Point, Vec<Point>> = HashMap::new();
    for (a, b) in segments {
        adj.entry(*a).or_default().push(*b);
        adj.entry(*b).or_default().push(*a);
    }

    let mut used: HashMap<(Point, Point), bool> = HashMap::new();
    for (a, b) in segments {
        used.insert(edge_key(*a, *b), false);
    }

    let mut loops = Vec::new();
    for (a, b) in segments {
        let key = edge_key(*a, *b);
        if used.get(&key).copied().unwrap_or(true) {
            continue;
        }
        let mut loop_pts = vec![*a, *b];
        mark_used(&mut used, *a, *b);
        let start = *a;
        let mut prev = *a;
        let mut cur = *b;
        let mut guard = 0;
        while cur != start && guard < segments.len() + 2 {
            guard += 1;
            let Some(nexts) = adj.get(&cur) else {
                break;
            };
            let mut found = None;
            for n in nexts {
                if *n != prev && !used.get(&edge_key(cur, *n)).copied().unwrap_or(true) {
                    found = Some(*n);
                    break;
                }
            }
            let Some(next) = found else {
                break;
            };
            mark_used(&mut used, cur, next);
            loop_pts.push(next);
            prev = cur;
            cur = next;
        }
        if cur == start && loop_pts.len() >= 4 {
            loop_pts.pop();
            if loop_pts.len() >= 3 {
                loops.push(loop_pts);
            }
        }
    }
    loops
}

fn edge_key(a: Point, b: Point) -> (Point, Point) {
    if (a.x, a.y) <= (b.x, b.y) {
        (a, b)
    } else {
        (b, a)
    }
}

fn mark_used(used: &mut HashMap<(Point, Point), bool>, a: Point, b: Point) {
    used.insert(edge_key(a, b), true);
}
