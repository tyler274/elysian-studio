#![forbid(unsafe_code)]

use bambu_geom::unscale;
use bambu_slicer::SliceResult;
use glam::Vec3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtrusionRole {
    Perimeter,
    Infill,
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
            for path in &layer.perimeters {
                for p in path {
                    vertices.push(ToolpathVertex {
                        position: Vec3::new(unscale(p.x) as f32, unscale(p.y) as f32, z),
                        role: ExtrusionRole::Perimeter,
                    });
                }
            }
            for path in &layer.infill {
                for p in path {
                    vertices.push(ToolpathVertex {
                        position: Vec3::new(unscale(p.x) as f32, unscale(p.y) as f32, z),
                        role: ExtrusionRole::Infill,
                    });
                }
            }
        }
        Self { vertices }
    }
}
