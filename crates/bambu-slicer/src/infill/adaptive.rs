//! Adaptive / support cubic infill (Bambu / PrusaSlicer `FillAdaptive`).
//!
//! An octree is densified where cubes meet the mesh (adaptive) or only under
//! upward faces (support cubic). Each cube that crosses a layer emits one
//! wall line in each of three 120° directions, then lines are clipped to the
//! sparse infill region.

use bambu_config::SliceSettings;
use bambu_geom::{Point, Polygon, Polyline, TriangleMesh};
use glam::{DQuat, DVec2, DVec3};

use super::clip_to_region;

const EPS: f64 = 1e-4;
const MERGE_GAP: i64 = 1000;
const CHILD_CENTERS: [DVec3; 8] = [
    DVec3::new(-1.0, -1.0, -1.0),
    DVec3::new(1.0, -1.0, -1.0),
    DVec3::new(-1.0, 1.0, -1.0),
    DVec3::new(1.0, 1.0, -1.0),
    DVec3::new(-1.0, -1.0, 1.0),
    DVec3::new(1.0, -1.0, 1.0),
    DVec3::new(-1.0, 1.0, 1.0),
    DVec3::new(1.0, 1.0, 1.0),
];
const TRAVERSAL: [[usize; 8]; 3] = [
    [2, 3, 0, 1, 6, 7, 4, 5],
    [4, 0, 6, 2, 5, 1, 7, 3],
    [1, 5, 0, 4, 3, 7, 2, 6],
];
const DIRECTION_ANGLES: [f64; 3] = [
    0.0,
    2.0 * std::f64::consts::PI / 3.0,
    -2.0 * std::f64::consts::PI / 3.0,
];

pub fn line_spacing_mm(settings: &SliceSettings) -> f64 {
    let density = settings.infill_density.max(0.05);
    settings.line_width_mm / (density / 3.0)
}

pub fn fill(region: &[Polygon], octree: &Octree, z_mm: f64) -> Vec<Polyline> {
    if region.is_empty() {
        return Vec::new();
    }
    clip_to_region(octree.lines_at_z(z_mm), region)
}

#[derive(Clone, Copy)]
struct CubeProps {
    edge_length: f64,
    height: f64,
    diagonal_length: f64,
    line_z_distance: f64,
    line_xy_distance: f64,
}

struct Cube {
    center: DVec3,
    children: [Option<usize>; 8],
}

pub(crate) struct Octree {
    cubes: Vec<Cube>,
    props: Vec<CubeProps>,
}

#[derive(Clone, Copy)]
struct AabbD {
    min: DVec3,
    max: DVec3,
}

impl Octree {
    pub(crate) fn build(
        mesh: &TriangleMesh,
        line_spacing: f64,
        support_only: bool,
    ) -> Option<Self> {
        if mesh.indices.is_empty() {
            return None;
        }
        let to_octree = transform_to_octree();
        let verts: Vec<DVec3> = mesh
            .vertices
            .iter()
            .map(|v| to_octree * v.as_dvec3())
            .collect();
        let (min, max) = bbox3(&verts)?;
        let origin = (min + max) * 0.5;
        let max_edge = (max - min).max_element();
        let props = cube_props(max_edge, line_spacing);
        let mut octree = Self {
            cubes: vec![Cube {
                center: origin,
                children: [None; 8],
            }],
            props,
        };
        if octree.props.len() > 1 {
            let half = octree.props.last()?.edge_length * 0.5;
            let diag = DVec3::splat(half);
            let root_bbox = AabbD {
                min: origin - diag,
                max: origin + diag,
            };
            let max_depth = octree.props.len() as i32 - 1;
            let up = to_octree * DVec3::Z;
            for idx in &mesh.indices {
                let a = verts[idx[0] as usize];
                let b = verts[idx[1] as usize];
                let c = verts[idx[2] as usize];
                if support_only && !is_overhang(a, b, c, up) {
                    continue;
                }
                octree.insert([a, b, c], 0, root_bbox, max_depth);
            }
            let to_world = transform_to_world();
            for cube in &mut octree.cubes {
                cube.center = to_world * cube.center;
            }
        }
        Some(octree)
    }

    fn insert(&mut self, tri: [DVec3; 3], cube: usize, bbox: AabbD, depth: i32) {
        let depth = depth - 1;
        let child_edge = self.props[depth as usize].edge_length;
        let parent_center = self.cubes[cube].center;
        for (i, dir) in CHILD_CENTERS.iter().enumerate() {
            let child_bbox = child_aabb(bbox, parent_center, *dir);
            if !triangle_aabb_intersects(tri, child_bbox) {
                continue;
            }
            let child = match self.cubes[cube].children[i] {
                Some(idx) => idx,
                None => {
                    let idx = self.cubes.len();
                    self.cubes.push(Cube {
                        center: parent_center + *dir * (child_edge / 2.0),
                        children: [None; 8],
                    });
                    self.cubes[cube].children[i] = Some(idx);
                    idx
                }
            };
            if depth > 0 {
                self.insert(tri, child, child_bbox, depth);
            }
        }
    }

    fn lines_at_z(&self, z_mm: f64) -> Vec<Polyline> {
        let mut out = Vec::new();
        for dir in 0..3 {
            let mut ctx = FillCtx::new(&self.props, z_mm, dir);
            ctx.walk(&self.cubes, 0, 0, self.props.len() as i32 - 1);
            ctx.flush_into(&mut out);
        }
        out
    }
}

struct FillCtx<'a> {
    props: &'a [CubeProps],
    z: f64,
    order: &'a [usize; 8],
    cos_a: f64,
    sin_a: f64,
    temp: Vec<Option<(Point, Point)>>,
    output: Vec<Polyline>,
}

impl<'a> FillCtx<'a> {
    fn new(props: &'a [CubeProps], z: f64, dir: usize) -> Self {
        let n = props.len().min(16);
        let angle = DIRECTION_ANGLES[dir];
        Self {
            props,
            z,
            order: &TRAVERSAL[dir],
            cos_a: angle.cos(),
            sin_a: angle.sin(),
            temp: vec![None; (1usize << n).saturating_sub(1).max(1)],
            output: Vec::new(),
        }
    }

    fn walk(&mut self, cubes: &[Cube], cube_idx: usize, address: usize, depth: i32) {
        let center = cubes[cube_idx].center;
        let children = cubes[cube_idx].children;
        let CubeProps {
            height,
            diagonal_length: diagonal,
            line_z_distance: line_z,
            line_xy_distance: line_xy,
            ..
        } = self.props[depth as usize];
        let z_diff = self.z - center.z;
        let z_abs = z_diff.abs();
        if z_abs > height / 2.0 {
            return;
        }
        if z_abs < line_z {
            let from = DVec2::new(
                0.5 * diagonal * (line_z - z_abs) / line_z,
                line_xy - (line_z + z_diff) / std::f64::consts::SQRT_2,
            );
            let to = DVec2::new(-from.x, from.y);
            let from = self.rotate(from) + DVec2::new(center.x, center.y);
            let to = self.rotate(to) + DVec2::new(center.x, center.y);
            self.extend(
                address,
                Point::from_mm(from.x, from.y),
                Point::from_mm(to.x, to.y),
            );
        }
        let temp_len = self.temp.len();
        if depth == 0 || address >= temp_len {
            return;
        }
        let depth = depth - 1;
        let mut address = address * 2 + 1;
        let order = *self.order;
        for (i, child_idx) in order.into_iter().enumerate() {
            if let Some(child) = children[child_idx] {
                if address < temp_len {
                    self.walk(cubes, child, address, depth);
                }
            }
            if i == 3 {
                address += 1;
            }
        }
    }

    fn rotate(&self, v: DVec2) -> DVec2 {
        DVec2::new(
            self.cos_a * v.x - self.sin_a * v.y,
            self.sin_a * v.x + self.cos_a * v.y,
        )
    }

    fn extend(&mut self, address: usize, from: Point, to: Point) {
        if address >= self.temp.len() {
            self.output.push(vec![from, to]);
            return;
        }
        match self.temp[address] {
            None => self.temp[address] = Some((from, to)),
            Some((a, b)) => {
                let gap = (from.x - b.x).abs().max((from.y - b.y).abs());
                if gap > MERGE_GAP {
                    self.output.push(vec![a, b]);
                    self.temp[address] = Some((from, to));
                } else {
                    self.temp[address] = Some((a, to));
                }
            }
        }
    }

    fn flush_into(&mut self, out: &mut Vec<Polyline>) {
        out.append(&mut self.output);
        for (a, b) in self.temp.drain(..).flatten() {
            out.push(vec![a, b]);
        }
    }
}

fn cube_props(max_cube_edge: f64, line_spacing: f64) -> Vec<CubeProps> {
    let mut props = Vec::new();
    let mut edge = line_spacing * 2.0;
    loop {
        props.push(CubeProps {
            edge_length: edge,
            height: edge * 3.0_f64.sqrt(),
            diagonal_length: edge * std::f64::consts::SQRT_2,
            line_z_distance: edge / 3.0_f64.sqrt(),
            line_xy_distance: edge / 6.0_f64.sqrt(),
        });
        if edge > max_cube_edge + EPS || props.len() >= 16 {
            break;
        }
        edge *= 2.0;
    }
    props
}

fn transform_to_world() -> DQuat {
    let r = octree_rot();
    DQuat::from_axis_angle(DVec3::Z, r[2])
        * DQuat::from_axis_angle(DVec3::Y, r[1])
        * DQuat::from_axis_angle(DVec3::X, r[0])
}

fn transform_to_octree() -> DQuat {
    let r = octree_rot();
    DQuat::from_axis_angle(DVec3::X, -r[0])
        * DQuat::from_axis_angle(DVec3::Y, -r[1])
        * DQuat::from_axis_angle(DVec3::Z, -r[2])
}

fn octree_rot() -> [f64; 3] {
    [
        5.0 * std::f64::consts::PI / 4.0,
        215.264_f64.to_radians(),
        std::f64::consts::PI / 6.0,
    ]
}

fn is_overhang(a: DVec3, b: DVec3, c: DVec3, up: DVec3) -> bool {
    let n = (b - a).cross(c - b);
    n.dot(up) > 0.707 * n.length()
}

fn bbox3(verts: &[DVec3]) -> Option<(DVec3, DVec3)> {
    let mut iter = verts.iter().copied();
    let first = iter.next()?;
    let mut min = first;
    let mut max = first;
    for v in iter {
        min = min.min(v);
        max = max.max(v);
    }
    Some((min, max))
}

fn child_aabb(parent: AabbD, center: DVec3, dir: DVec3) -> AabbD {
    let mut min = DVec3::ZERO;
    let mut max = DVec3::ZERO;
    for k in 0..3 {
        if dir[k] < 0.0 {
            min[k] = parent.min[k];
            max[k] = center[k] + EPS;
        } else {
            min[k] = center[k] - EPS;
            max[k] = parent.max[k];
        }
    }
    AabbD { min, max }
}

fn triangle_aabb_intersects(tri: [DVec3; 3], aabb: AabbD) -> bool {
    let [a, b, c] = tri;
    let t_min = a.min(b.min(c));
    let t_max = a.max(b.max(c));
    if t_min.x >= aabb.max.x
        || t_max.x <= aabb.min.x
        || t_min.y >= aabb.max.y
        || t_max.y <= aabb.min.y
        || t_min.z >= aabb.max.z
        || t_max.z <= aabb.min.z
    {
        return false;
    }

    let center = (aabb.min + aabb.max) * 0.5;
    let h = aabb.max - center;
    let t = [b - a, c - a, c - b];
    let ac = a - center;
    let n = t[0].cross(t[1]);
    let s = n.dot(ac);
    let r = h.dot(n.abs()).abs();
    if s.abs() >= r {
        return false;
    }

    let at = [t[0].abs(), t[1].abs(), t[2].abs()];
    let bc = b - center;
    let cc = c - center;
    let tests = [
        (t[0], ac, cc, h.y, h.z, at[0].z, at[0].y, true),
        (t[1], ac, bc, h.y, h.z, at[1].z, at[1].y, true),
        (t[2], ac, bc, h.y, h.z, at[2].z, at[2].y, true),
        (t[0], ac, cc, h.x, h.z, at[0].z, at[0].x, false),
        (t[1], ac, bc, h.x, h.z, at[1].z, at[1].x, false),
        (t[2], ac, bc, h.x, h.z, at[2].z, at[2].x, false),
    ];
    for (edge, p0, p1, ha, hb, ata, atb, ex) in tests {
        let (d1, d2) = if ex {
            (edge.y * p0.z - edge.z * p0.y, edge.y * p1.z - edge.z * p1.y)
        } else {
            (edge.z * p0.x - edge.x * p0.z, edge.z * p1.x - edge.x * p1.z)
        };
        let tc = (d1 + d2) * 0.5;
        let r = (ha * ata + hb * atb).abs();
        if r + (tc - d1).abs() < tc.abs() {
            return false;
        }
    }

    // eZ × t[0..2]
    for (edge, p1) in [(t[0], cc), (t[1], bc), (t[2], bc)] {
        let d1 = edge.x * ac.y - edge.y * ac.x;
        let d2 = edge.x * p1.y - edge.y * p1.x;
        let tc = (d1 + d2) * 0.5;
        let at = edge.abs();
        let r = (h.y * at.x + h.x * at.y).abs();
        if r + (tc - d1).abs() < tc.abs() {
            return false;
        }
    }
    true
}
