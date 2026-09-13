//! Binary SAH (binned) BVH over mesh triangles, for CPU picking.

use glam::Vec3;

use crate::mesh::{ray_triangle, Aabb3, TriangleMesh};

const BINS: usize = 8;
const LEAF_TRIS: usize = 8;

#[derive(Debug, Clone)]
struct Node {
    aabb: Aabb3,
    /// Leaf: `left` indexes `prims`, `right` is triangle count.
    /// Inner: `left` / `right` are child node indices.
    left: u32,
    right: u32,
    leaf: bool,
}

/// Bounding-volume hierarchy over triangle indices of a [`TriangleMesh`].
#[derive(Debug, Clone)]
pub struct Bvh {
    nodes: Vec<Node>,
    prims: Vec<u32>,
}

impl Bvh {
    pub fn build(mesh: &TriangleMesh) -> Self {
        let n = mesh.indices.len();
        let mut prims: Vec<u32> = (0..n as u32).collect();
        let aabbs: Vec<Aabb3> = mesh
            .indices
            .iter()
            .map(|&idx| {
                let [a, b, c] = mesh.triangle(idx);
                Aabb3::from_points([a, b, c]).unwrap_or(Aabb3 {
                    min: Vec3::ZERO,
                    max: Vec3::ZERO,
                })
            })
            .collect();
        let mut nodes = Vec::with_capacity(n.max(1) * 2);
        if n == 0 {
            return Self { nodes, prims };
        }
        build_node(&mut nodes, &mut prims, &aabbs, 0, n);
        Self { nodes, prims }
    }

    /// Closest triangle along a ray, or `None`.
    pub fn pick_triangle(&self, mesh: &TriangleMesh, origin: Vec3, dir: Vec3) -> Option<usize> {
        let dir = dir.normalize_or_zero();
        if dir.length_squared() < 1e-12 || self.nodes.is_empty() {
            return None;
        }
        let inv = Vec3::new(
            if dir.x.abs() > 1e-12 {
                1.0 / dir.x
            } else {
                f32::INFINITY
            },
            if dir.y.abs() > 1e-12 {
                1.0 / dir.y
            } else {
                f32::INFINITY
            },
            if dir.z.abs() > 1e-12 {
                1.0 / dir.z
            } else {
                f32::INFINITY
            },
        );
        let mut stack = [0u32; 64];
        let mut sp = 1usize;
        stack[0] = 0;
        let mut best = f32::MAX;
        let mut hit = None;
        while sp > 0 {
            sp -= 1;
            let node = &self.nodes[stack[sp] as usize];
            if !node.aabb.intersects_ray(origin, inv) {
                continue;
            }
            if node.leaf {
                for i in 0..node.right {
                    let tri = self.prims[(node.left + i) as usize] as usize;
                    let [a, b, c] = mesh.triangle(mesh.indices[tri]);
                    if let Some(t) = ray_triangle(origin, dir, a, b, c) {
                        if t < best {
                            best = t;
                            hit = Some(tri);
                        }
                    }
                }
            } else if sp + 2 <= stack.len() {
                stack[sp] = node.left;
                stack[sp + 1] = node.right;
                sp += 2;
            }
        }
        hit
    }

    /// Triangle indices whose centroids lie within `radius` of `point`.
    pub fn triangles_near(&self, mesh: &TriangleMesh, point: Vec3, radius: f32) -> Vec<usize> {
        let r = radius.max(0.0);
        if self.nodes.is_empty() {
            return Vec::new();
        }
        let mut out = Vec::new();
        let mut stack = vec![0u32];
        while let Some(ni) = stack.pop() {
            let node = &self.nodes[ni as usize];
            if !node.aabb.intersects_sphere(point, r) {
                continue;
            }
            if node.leaf {
                for i in 0..node.right {
                    let tri = self.prims[(node.left + i) as usize] as usize;
                    let [a, b, c] = mesh.triangle(mesh.indices[tri]);
                    let centroid = (a + b + c) / 3.0;
                    if centroid.distance(point) <= r {
                        out.push(tri);
                    }
                }
            } else {
                stack.push(node.left);
                stack.push(node.right);
            }
        }
        out
    }
}

fn build_node(
    nodes: &mut Vec<Node>,
    prims: &mut [u32],
    aabbs: &[Aabb3],
    start: usize,
    count: usize,
) -> u32 {
    let idx = nodes.len() as u32;
    nodes.push(Node {
        aabb: bounds(prims, aabbs, start, count),
        left: start as u32,
        right: count as u32,
        leaf: true,
    });
    if count <= LEAF_TRIS {
        return idx;
    }
    let aabb = nodes[idx as usize].aabb;
    let axis = aabb.longest_axis();
    let Some(split) = sah_split(prims, aabbs, start, count, axis, aabb) else {
        return idx;
    };
    if split <= start || split >= start + count {
        return idx;
    }
    let left = build_node(nodes, prims, aabbs, start, split - start);
    let right = build_node(nodes, prims, aabbs, split, start + count - split);
    nodes[idx as usize] = Node {
        aabb,
        left,
        right,
        leaf: false,
    };
    idx
}

fn bounds(prims: &[u32], aabbs: &[Aabb3], start: usize, count: usize) -> Aabb3 {
    let mut aabb = aabbs[prims[start] as usize];
    for i in 1..count {
        aabb = aabb.union(aabbs[prims[start + i] as usize]);
    }
    aabb
}

fn sah_split(
    prims: &mut [u32],
    aabbs: &[Aabb3],
    start: usize,
    count: usize,
    axis: usize,
    aabb: Aabb3,
) -> Option<usize> {
    let extent = aabb.size()[axis];
    if extent < 1e-8 {
        prims[start..start + count].sort_by(|a, b| {
            centroid(aabbs[*a as usize])[axis]
                .partial_cmp(&centroid(aabbs[*b as usize])[axis])
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        return Some(start + count / 2);
    }
    let mut bins = [(0u32, Aabb3::empty()); BINS];
    let scale = BINS as f32 / extent;
    for i in 0..count {
        let p = prims[start + i] as usize;
        let c = centroid(aabbs[p]);
        let mut b = ((c[axis] - aabb.min[axis]) * scale) as usize;
        b = b.min(BINS - 1);
        bins[b].0 += 1;
        bins[b].1 = if bins[b].0 == 1 {
            aabbs[p]
        } else {
            bins[b].1.union(aabbs[p])
        };
    }
    let mut best_cost = f32::MAX;
    let mut best_bin = 0usize;
    for s in 0..BINS - 1 {
        let mut left_n = 0u32;
        let mut right_n = 0u32;
        let mut left_a = Aabb3::empty();
        let mut right_a = Aabb3::empty();
        for (n, aabb) in bins.iter().take(s + 1) {
            if *n > 0 {
                left_a = if left_n == 0 {
                    *aabb
                } else {
                    left_a.union(*aabb)
                };
                left_n += *n;
            }
        }
        for (n, aabb) in bins.iter().skip(s + 1) {
            if *n > 0 {
                right_a = if right_n == 0 {
                    *aabb
                } else {
                    right_a.union(*aabb)
                };
                right_n += *n;
            }
        }
        if left_n == 0 || right_n == 0 {
            continue;
        }
        let cost = left_a.surface_area() * left_n as f32 + right_a.surface_area() * right_n as f32;
        if cost < best_cost {
            best_cost = cost;
            best_bin = s;
        }
    }
    let mid = aabb.min[axis] + extent * ((best_bin + 1) as f32 / BINS as f32);
    let slice = &mut prims[start..start + count];
    slice.sort_by(|a, b| {
        centroid(aabbs[*a as usize])[axis]
            .partial_cmp(&centroid(aabbs[*b as usize])[axis])
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let split = slice
        .iter()
        .position(|&p| centroid(aabbs[p as usize])[axis] >= mid);
    split
        .map(|p| start + p)
        .filter(|&s| s > start && s < start + count)
}

fn centroid(aabb: Aabb3) -> Vec3 {
    (aabb.min + aabb.max) * 0.5
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::TriangleMesh;

    #[test]
    fn cube_bvh_matches_linear_pick() {
        let mesh = TriangleMesh::cube(20.0);
        let bvh = Bvh::build(&mesh);
        let origin = Vec3::new(10.0, 10.0, 40.0);
        let dir = Vec3::new(0.0, 0.0, -1.0);
        assert_eq!(
            bvh.pick_triangle(&mesh, origin, dir),
            mesh.pick_triangle_linear(origin, dir)
        );
        let side = Vec3::new(-10.0, 10.0, 10.0);
        let along = Vec3::new(1.0, 0.0, 0.0);
        assert_eq!(
            bvh.pick_triangle(&mesh, side, along),
            mesh.pick_triangle_linear(side, along)
        );
    }

    #[test]
    fn cube_near_matches_linear() {
        let mesh = TriangleMesh::cube(20.0);
        let bvh = Bvh::build(&mesh);
        let p = Vec3::new(20.0, 10.0, 10.0);
        let mut a = bvh.triangles_near(&mesh, p, 2.0);
        let mut b = mesh.triangles_near_linear(p, 2.0);
        a.sort_unstable();
        b.sort_unstable();
        assert_eq!(a, b);
    }
}
