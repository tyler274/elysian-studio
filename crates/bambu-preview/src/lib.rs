#![forbid(unsafe_code)]

use bambu_geom::unscale;
use bambu_slicer::SliceResult;
use glam::Vec3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtrusionRole {
    OuterWall,
    InnerWall,
    Infill,
    Skirt,
    Brim,
    Support,
    SupportInterface,
}

#[derive(Debug, Clone)]
pub struct ToolpathVertex {
    pub position: Vec3,
    pub role: ExtrusionRole,
}

#[derive(Debug, Clone, Default)]
pub struct ToolpathBuffer {
    pub vertices: Vec<ToolpathVertex>,
}

impl ToolpathBuffer {
    pub fn from_slice(sliced: &SliceResult) -> Self {
        let mut vertices = Vec::new();
        for layer in &sliced.layers {
            let z = layer.z_mm as f32;
            emit_paths(&mut vertices, &layer.skirt, z, ExtrusionRole::Skirt, true);
            emit_paths(&mut vertices, &layer.brim, z, ExtrusionRole::Brim, true);
            emit_paths(&mut vertices, &layer.support, z, ExtrusionRole::Support, false);
            emit_paths(
                &mut vertices,
                &layer.support_interface,
                z,
                ExtrusionRole::SupportInterface,
                false,
            );
            emit_paths(&mut vertices, &layer.outer_walls, z, ExtrusionRole::OuterWall, true);
            emit_paths(&mut vertices, &layer.inner_walls, z, ExtrusionRole::InnerWall, true);
            emit_paths(&mut vertices, &layer.infill, z, ExtrusionRole::Infill, false);
        }
        Self { vertices }
    }

    pub fn is_empty(&self) -> bool {
        self.vertices.is_empty()
    }
}

fn emit_paths(
    vertices: &mut Vec<ToolpathVertex>,
    paths: &[bambu_geom::Polyline],
    z: f32,
    role: ExtrusionRole,
    closed: bool,
) {
    for path in paths {
        if path.len() < 2 {
            continue;
        }
        let n = path.len();
        let count = if closed { n } else { n - 1 };
        for i in 0..count {
            let a = path[i];
            let b = path[(i + 1) % n];
            vertices.push(vertex(a, z, role));
            vertices.push(vertex(b, z, role));
        }
    }
}

fn vertex(p: bambu_geom::Point, z: f32, role: ExtrusionRole) -> ToolpathVertex {
    ToolpathVertex {
        position: Vec3::new(unscale(p.x) as f32, unscale(p.y) as f32, z),
        role,
    }
}
