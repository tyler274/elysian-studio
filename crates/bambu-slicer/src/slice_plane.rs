//! Triangle–plane intersection and loop stitching.
//!
//! Triangle walks are Rayon-parallel on large meshes (ordered `par_chunks`
//! collect). A `wide::f32x4` Z-overlap cull feeds the scalar f64 edge
//! interpolator so clipper input stays bit-identical to a fully scalar walk.

use std::collections::HashMap;

use bambu_geom::{scale, Point, Polygon, TriangleMesh};
use glam::Vec3;
use rayon::prelude::*;
use wide::{f32x4, CmpGe, CmpLe};

/// Parallelize the triangle walk only when a single plane has enough work to
/// pay for Rayon splitting. Layer-parallel `slice_mesh` already fills the pool
/// on typical parts.
const TRI_PARALLEL_MIN: usize = 4096;
const TRI_CHUNK: usize = 256;
/// Inclusive f32 pad so the SIMD cull never drops a triangle the 1e-9 scalar
/// interpolator would still hit.
const CULL_EPS: f32 = 1e-5;

pub fn slice_at_z(mesh: &TriangleMesh, z: f32) -> Vec<Polygon> {
    loops_from_segments(&collect_segments(mesh, z))
}

/// Convert plane-hit segments into closed contours. Used by the CPU slicer and
/// by the Vulkan compute readback path.
pub fn loops_from_segments(segments: &[(Point, Point)]) -> Vec<Polygon> {
    stitch_loops(segments)
}

pub fn point_from_xy_mm(x: f64, y: f64) -> Point {
    Point::new(snap(scale(x)), snap(scale(y)))
}

fn collect_segments(mesh: &TriangleMesh, z: f32) -> Vec<(Point, Point)> {
    if mesh.indices.len() >= TRI_PARALLEL_MIN {
        let parts: Vec<Vec<(Point, Point)>> = mesh
            .indices
            .par_chunks(TRI_CHUNK)
            .map(|chunk| segments_from_indices(mesh, chunk, z))
            .collect();
        parts.into_iter().flatten().collect()
    } else {
        segments_from_indices(mesh, &mesh.indices, z)
    }
}

fn segments_from_indices(mesh: &TriangleMesh, indices: &[[u32; 3]], z: f32) -> Vec<(Point, Point)> {
    let mut segments = Vec::new();
    let (chunks, rem) = indices.as_chunks::<4>();
    for chunk in chunks {
        let tris = [
            mesh.triangle(chunk[0]),
            mesh.triangle(chunk[1]),
            mesh.triangle(chunk[2]),
            mesh.triangle(chunk[3]),
        ];
        let hits = triangles_overlap_z4(tris, z);
        for (tri, hit) in tris.into_iter().zip(hits) {
            if hit {
                push_segment(&mut segments, tri, z);
            }
        }
    }
    for idx in rem {
        let tri = mesh.triangle(*idx);
        if triangle_overlaps_z(tri, z) {
            push_segment(&mut segments, tri, z);
        }
    }
    segments
}

fn push_segment(segments: &mut Vec<(Point, Point)>, tri: [Vec3; 3], z: f32) {
    if let Some((p0, p1)) = triangle_plane_segment(tri[0], tri[1], tri[2], z) {
        if p0 != p1 {
            segments.push((p0, p1));
        }
    }
}

fn triangle_overlaps_z(tri: [Vec3; 3], z: f32) -> bool {
    let min_z = tri[0].z.min(tri[1].z).min(tri[2].z);
    let max_z = tri[0].z.max(tri[1].z).max(tri[2].z);
    min_z - CULL_EPS <= z && max_z + CULL_EPS >= z
}

fn triangles_overlap_z4(tris: [[Vec3; 3]; 4], z: f32) -> [bool; 4] {
    let z0 = f32x4::from([tris[0][0].z, tris[1][0].z, tris[2][0].z, tris[3][0].z]);
    let z1 = f32x4::from([tris[0][1].z, tris[1][1].z, tris[2][1].z, tris[3][1].z]);
    let z2 = f32x4::from([tris[0][2].z, tris[1][2].z, tris[2][2].z, tris[3][2].z]);
    let mn = z0.min(z1.min(z2));
    let mx = z0.max(z1.max(z2));
    let zz = f32x4::splat(z);
    let eps = f32x4::splat(CULL_EPS);
    let hit = mn.cmp_le(zz + eps) & mx.cmp_ge(zz - eps);
    let bits: [f32; 4] = hit.to_array();
    [
        bits[0] != 0.0,
        bits[1] != 0.0,
        bits[2] != 0.0,
        bits[3] != 0.0,
    ]
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

#[cfg(test)]
fn collect_segments_scalar(mesh: &TriangleMesh, z: f32) -> Vec<(Point, Point)> {
    let mut segments = Vec::new();
    for idx in &mesh.indices {
        push_segment(&mut segments, mesh.triangle(*idx), z);
    }
    segments
}

#[cfg(test)]
mod tests {
    use super::*;
    use bambu_geom::TriangleMesh;

    #[test]
    fn simd_cull_matches_scalar_segments() {
        let mesh = TriangleMesh::cube(20.0);
        for z in [0.1_f32, 0.14, 10.0, 19.9] {
            let simd = collect_segments(&mesh, z);
            let scalar = collect_segments_scalar(&mesh, z);
            assert_eq!(simd, scalar, "z={z}");
        }
    }

    #[test]
    fn overlap_mask_matches_scalar_cull() {
        let mesh = TriangleMesh::cube(20.0);
        let z = 10.0_f32;
        for chunk in mesh.indices.as_chunks::<4>().0 {
            let tris = [
                mesh.triangle(chunk[0]),
                mesh.triangle(chunk[1]),
                mesh.triangle(chunk[2]),
                mesh.triangle(chunk[3]),
            ];
            let simd = triangles_overlap_z4(tris, z);
            let scalar = [
                triangle_overlaps_z(tris[0], z),
                triangle_overlaps_z(tris[1], z),
                triangle_overlaps_z(tris[2], z),
                triangle_overlaps_z(tris[3], z),
            ];
            assert_eq!(simd, scalar);
        }
    }
}
