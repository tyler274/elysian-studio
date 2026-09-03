//! Triangle mesh in millimeters.

use glam::Vec3;

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

    /// Axis-aligned cube from the origin to `size` millimeters on each axis.
    pub fn cube(size: f32) -> Self {
        let s = size;
        let vertices = vec![
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(s, 0.0, 0.0),
            Vec3::new(s, s, 0.0),
            Vec3::new(0.0, s, 0.0),
            Vec3::new(0.0, 0.0, s),
            Vec3::new(s, 0.0, s),
            Vec3::new(s, s, s),
            Vec3::new(0.0, s, s),
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
}
