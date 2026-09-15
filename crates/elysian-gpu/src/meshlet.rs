//! Nanite-like triangle clusters for frustum-culled Fast raster.

use bambu_geom::{Aabb3, TriangleMesh};
use glam::{Mat4, Vec3, Vec4};
use rayon::prelude::*;

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
    let n = mesh.indices.len();
    let starts: Vec<usize> = (0..n).step_by(MESHLET_TRIS).collect();
    starts
        .into_par_iter()
        .map(|i| {
            let count = (n - i).min(MESHLET_TRIS);
            let mut aabb: Option<Aabb3> = None;
            for idx in &mesh.indices[i..i + count] {
                let [a, b, c] = mesh.triangle(*idx);
                if let Some(t) = Aabb3::from_points([a, b, c]) {
                    aabb = Some(match aabb {
                        Some(cur) => cur.union(t),
                        None => t,
                    });
                }
            }
            Meshlet {
                first_tri: i as u32,
                tri_count: count as u32,
                aabb: aabb.unwrap_or(Aabb3::empty()),
            }
        })
        .collect()
}

/// Six frustum planes (ax+by+cz+d >= 0 inside) from `proj * view`.
pub fn frustum_planes(view_proj: Mat4) -> [Vec4; 6] {
    let r0 = view_proj.row(0);
    let r1 = view_proj.row(1);
    let r2 = view_proj.row(2);
    let r3 = view_proj.row(3);
    [
        plane(r3 + r0),
        plane(r3 - r0),
        plane(r3 + r1),
        plane(r3 - r1),
        // wgpu clip z is [0, 1] (DX), not OpenGL [-1, 1].
        plane(r2),
        plane(r3 - r2),
    ]
}

fn plane(p: Vec4) -> Vec4 {
    let len = Vec3::new(p.x, p.y, p.z).length();
    if len <= 1e-8 {
        Vec4::ZERO
    } else {
        p / len
    }
}

pub fn aabb_in_frustum(aabb: Aabb3, planes: &[Vec4; 6]) -> bool {
    // Inverted/empty meshlet AABBs must not hide the model.
    if !aabb.min.is_finite()
        || !aabb.max.is_finite()
        || aabb.min.x > aabb.max.x
        || aabb.min.y > aabb.max.y
        || aabb.min.z > aabb.max.z
    {
        return true;
    }
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

    #[test]
    fn look_at_target_aabb_is_inside_frustum() {
        let cam =
            crate::camera::OrbitCamera::looking_at_center(Vec3::new(128.0, 128.0, 20.0), 256.0);
        let proj = cam.perspective(1.0);
        let planes = frustum_planes(proj * cam.view_matrix());
        let aabb = Aabb3 {
            min: Vec3::new(118.0, 118.0, 0.0),
            max: Vec3::new(138.0, 138.0, 40.0),
        };
        assert!(
            aabb_in_frustum(aabb, &planes),
            "a figure on the bed must survive frustum culling"
        );
    }

    #[test]
    fn behind_camera_aabb_is_outside_frustum() {
        let cam =
            crate::camera::OrbitCamera::looking_at_center(Vec3::new(128.0, 128.0, 0.0), 256.0);
        let proj = cam.perspective(1.0);
        let planes = frustum_planes(proj * cam.view_matrix());
        let behind = cam.eye() + (cam.eye() - cam.target).normalize() * 200.0;
        let aabb = Aabb3 {
            min: behind - Vec3::splat(5.0),
            max: behind + Vec3::splat(5.0),
        };
        assert!(!aabb_in_frustum(aabb, &planes));
    }

    #[test]
    fn inverted_aabb_is_not_culled() {
        let cam = crate::camera::OrbitCamera::looking_at_bed(256.0);
        let planes = frustum_planes(cam.perspective(1.0) * cam.view_matrix());
        assert!(aabb_in_frustum(Aabb3::empty(), &planes));
    }
}
