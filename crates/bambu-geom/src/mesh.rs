//! Triangle mesh in millimeters.

use glam::{Mat4, Vec3};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Aabb3 {
    pub min: Vec3,
    pub max: Vec3,
}

impl Aabb3 {
    pub fn from_points(points: impl IntoIterator<Item = Vec3>) -> Option<Self> {
        let mut iter = points.into_iter();
        let first = iter.next()?;
        let mut min = first;
        let mut max = first;
        for p in iter {
            min = min.min(p);
            max = max.max(p);
        }
        Some(Self { min, max })
    }

    pub fn size(self) -> Vec3 {
        self.max - self.min
    }
}

#[derive(Debug, Clone, Default)]
pub struct TriangleMesh {
    pub vertices: Vec<Vec3>,
    pub indices: Vec<[u32; 3]>,
}

impl TriangleMesh {
    pub fn aabb(&self) -> Option<Aabb3> {
        Aabb3::from_points(self.vertices.iter().copied())
    }

    pub fn triangle(&self, idx: [u32; 3]) -> [Vec3; 3] {
        [
            self.vertices[idx[0] as usize],
            self.vertices[idx[1] as usize],
            self.vertices[idx[2] as usize],
        ]
    }

    pub fn translate(&mut self, delta: Vec3) {
        for v in &mut self.vertices {
            *v += delta;
        }
    }

    /// Apply a 4×4 transform to every vertex (Bambu volume `matrix` / 3MF component).
    pub fn transform(&mut self, xf: Mat4) {
        for v in &mut self.vertices {
            *v = xf.transform_point3(*v);
        }
    }

    /// C++ `filament_shrink` XY scale (100% → identity).
    pub fn scale_xy(&mut self, factor: f64) {
        if (factor - 1.0).abs() < 1e-12 {
            return;
        }
        let f = factor as f32;
        for v in &mut self.vertices {
            v.x *= f;
            v.y *= f;
        }
    }

    /// Move the mesh so it sits on z=0 and is centered on a square bed.
    pub fn place_on_bed(&mut self, bed_mm: f32) {
        let Some(aabb) = self.aabb() else {
            return;
        };
        let size = aabb.size();
        self.translate(Vec3::new(
            (bed_mm - size.x) * 0.5 - aabb.min.x,
            (bed_mm - size.y) * 0.5 - aabb.min.y,
            -aabb.min.z,
        ));
    }

    /// Axis-aligned box with the same face winding as [`cube`].
    pub fn aabb_box(min: Vec3, max: Vec3) -> Self {
        let vertices = vec![
            Vec3::new(min.x, min.y, min.z),
            Vec3::new(max.x, min.y, min.z),
            Vec3::new(max.x, max.y, min.z),
            Vec3::new(min.x, max.y, min.z),
            Vec3::new(min.x, min.y, max.z),
            Vec3::new(max.x, min.y, max.z),
            Vec3::new(max.x, max.y, max.z),
            Vec3::new(min.x, max.y, max.z),
        ];
        let indices = vec![
            [0, 1, 2],
            [0, 2, 3], // bottom
            [4, 6, 5],
            [4, 7, 6], // top
            [0, 4, 5],
            [0, 5, 1], // front
            [2, 6, 7],
            [2, 7, 3], // back
            [0, 3, 7],
            [0, 7, 4], // left
            [1, 5, 6],
            [1, 6, 2], // right
        ];
        Self { vertices, indices }
    }

    /// Axis-aligned cube from the origin to `size` millimeters on each axis.
    pub fn cube(size: f32) -> Self {
        Self::aabb_box(Vec3::ZERO, Vec3::splat(size))
    }

    pub fn box_mm(x: f32, y: f32, z: f32) -> Self {
        Self::aabb_box(Vec3::ZERO, Vec3::new(x, y, z))
    }

    pub fn append(&mut self, other: &Self) {
        let base = self.vertices.len() as u32;
        self.vertices.extend_from_slice(&other.vertices);
        self.indices.extend(
            other
                .indices
                .iter()
                .map(|&[a, b, c]| [a + base, b + base, c + base]),
        );
    }

    /// Two posts with a connecting beam — a two-sided bridge of `gap` mm.
    pub fn bridge_beam(post_xy: f32, post_z: f32, gap: f32, beam_z: f32) -> Self {
        let mut mesh = Self::aabb_box(Vec3::ZERO, Vec3::new(post_xy, post_xy, post_z));
        let x1 = post_xy + gap;
        mesh.append(&Self::aabb_box(
            Vec3::new(x1, 0.0, 0.0),
            Vec3::new(x1 + post_xy, post_xy, post_z),
        ));
        mesh.append(&Self::aabb_box(
            Vec3::new(0.0, 0.0, post_z),
            Vec3::new(x1 + post_xy, post_xy, post_z + beam_z),
        ));
        mesh
    }

    /// Cube with a thin side tab (`tab_x` out, `tab_y` wide, from `tab_z0` for `tab_z`).
    pub fn cube_with_tab(cube: f32, tab_x: f32, tab_y: f32, tab_z0: f32, tab_z: f32) -> Self {
        let mut mesh = Self::cube(cube);
        mesh.append(&Self::aabb_box(
            Vec3::new(cube, 0.0, tab_z0),
            Vec3::new(cube + tab_x, tab_y, tab_z0 + tab_z),
        ));
        mesh
    }

    /// Pillar with a larger slab on top — a reliable classic-support overhang.
    pub fn overhang_table(pillar_xy: f32, pillar_z: f32, slab_xy: f32, slab_z: f32) -> Self {
        let inset = (slab_xy - pillar_xy) * 0.5;
        let mut mesh = Self::aabb_box(
            Vec3::new(inset, inset, 0.0),
            Vec3::new(inset + pillar_xy, inset + pillar_xy, pillar_z),
        );
        mesh.append(&Self::aabb_box(
            Vec3::new(0.0, 0.0, pillar_z),
            Vec3::new(slab_xy, slab_xy, pillar_z + slab_z),
        ));
        mesh
    }

    /// Wide plate, thin stem, and a cap whose XY sits inside the plate.
    /// Overhangs of the cap cannot reach the build plate without resting on the model.
    pub fn pedestal_cap(
        base_xy: f32,
        stem_xy: f32,
        cap_xy: f32,
        base_z: f32,
        stem_z: f32,
        cap_z: f32,
    ) -> Self {
        let stem_inset = (base_xy - stem_xy) * 0.5;
        let cap_inset = (base_xy - cap_xy) * 0.5;
        let mut mesh = Self::aabb_box(Vec3::ZERO, Vec3::new(base_xy, base_xy, base_z));
        mesh.append(&Self::aabb_box(
            Vec3::new(stem_inset, stem_inset, base_z),
            Vec3::new(stem_inset + stem_xy, stem_inset + stem_xy, base_z + stem_z),
        ));
        let cap_z0 = base_z + stem_z;
        mesh.append(&Self::aabb_box(
            Vec3::new(cap_inset, cap_inset, cap_z0),
            Vec3::new(cap_inset + cap_xy, cap_inset + cap_xy, cap_z0 + cap_z),
        ));
        mesh
    }

    /// Square frustum from `base` mm at z=0 to `top` mm at z=`height`.
    pub fn frustum(base: f32, top: f32, height: f32) -> Self {
        let inset = (base - top) * 0.5;
        Self {
            vertices: vec![
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(base, 0.0, 0.0),
                Vec3::new(base, base, 0.0),
                Vec3::new(0.0, base, 0.0),
                Vec3::new(inset, inset, height),
                Vec3::new(inset + top, inset, height),
                Vec3::new(inset + top, inset + top, height),
                Vec3::new(inset, inset + top, height),
            ],
            indices: vec![
                [0, 1, 2],
                [0, 2, 3],
                [4, 6, 5],
                [4, 7, 6],
                [0, 4, 5],
                [0, 5, 1],
                [2, 6, 7],
                [2, 7, 3],
                [0, 3, 7],
                [0, 7, 4],
                [1, 5, 6],
                [1, 6, 2],
            ],
        }
    }
}
