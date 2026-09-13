//! Nanite-like triangle clusters for frustum-culled Fast raster.

use bambu_geom::{Aabb3, TriangleMesh};
use glam::{Mat4, Vec3, Vec4};

pub const MESHLET_TRIS: usize = 128;

#[derive(Debug, Clone, Copy)]
pub struct Meshlet {
    pub first_tri: u32,
    pub tri_count: u32,
    pub aabb: Aabb3,
}

/// Pack triangles into clusters of at most `MESHLET_TRIS`.
pub fn clusterize(mesh: &TriangleMesh) -> Vec<Meshlet> {
    if mesh.indices.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < mesh.indices.len() {
        let n = (mesh.indices.len() - i).min(MESHLET_TRIS);
        let mut aabb: Option<Aabb3> = None;
        for idx in &mesh.indices[i..i + n] {
            let [a, b, c] = mesh.triangle(*idx);
            if let Some(t) = Aabb3::from_points([a, b, c]) {
                aabb = Some(match aabb {
                    Some(cur) => cur.union(t),
                    None => t,
                });
            }
        }
        out.push(Meshlet {
            first_tri: i as u32,
            tri_count: n as u32,
            aabb: aabb.unwrap_or(Aabb3::empty()),
        });
        i += n;
    }
    out
}

/// Six frustum planes (ax+by+cz+d >= 0 inside) from `proj * view`.
pub fn frustum_planes(view_proj: Mat4) -> [Vec4; 6] {
    let r0 = view_proj.row(0);
    let r1 = view_proj.row(1);
    let r2 = view_proj.row(2);
    let r3 = view_proj.row(3);
    [
        (r3 + r0).normalize_or_zero(),
        (r3 - r0).normalize_or_zero(),
        (r3 + r1).normalize_or_zero(),
        (r3 - r1).normalize_or_zero(),
        r2.normalize_or_zero(),
        (r3 - r2).normalize_or_zero(),
    ]
}

pub fn aabb_in_frustum(aabb: Aabb3, planes: &[Vec4; 6]) -> bool {
    let c = (aabb.min + aabb.max) * 0.5;
    let e = (aabb.max - aabb.min) * 0.5;
    for p in planes {
        let n = Vec3::new(p.x, p.y, p.z);
        let r = e.x * n.x.abs() + e.y * n.y.abs() + e.z * n.z.abs();
        if n.dot(c) + p.w + r < 0.0 {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cube_meshlets_cover_every_triangle() {
        let mesh = TriangleMesh::cube(20.0);
        let lets = clusterize(&mesh);
        let covered: u32 = lets.iter().map(|m| m.tri_count).sum();
        assert_eq!(covered as usize, mesh.indices.len());
        assert!(!lets.is_empty());
    }
}
