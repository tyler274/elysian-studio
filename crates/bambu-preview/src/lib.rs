#![forbid(unsafe_code)]

use bambu_geom::unscale;
use bambu_slicer::SliceResult;
use glam::Vec3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtrusionRole {
    OuterWall,
    InnerWall,
    Infill,
    SolidInfill,
    FloatingVerticalShell,
    TopSurface,
    BottomSurface,
    Bridge,
    Skirt,
    Brim,
    PrimeTower,
    Support,
    SupportInterface,
    Ironing,
}

#[derive(Debug, Clone)]
pub struct ToolpathVertex {
    pub position: Vec3,
    pub role: ExtrusionRole,
}

#[derive(Debug, Clone, Default)]
pub struct ToolpathBuffer {
    pub vertices: Vec<ToolpathVertex>,
    pub layer_zs: Vec<f32>,
    pub layer_vertex_ends: Vec<usize>,
}

impl ToolpathBuffer {
    pub fn from_slice(sliced: &SliceResult) -> Self {
        let mut vertices = Vec::new();
        let mut layer_zs = Vec::with_capacity(sliced.layers.len());
        let mut layer_vertex_ends = Vec::with_capacity(sliced.layers.len());
        for layer in &sliced.layers {
            let z = layer.print_z_mm as f32;
            layer_zs.push(z);
            emit_paths(&mut vertices, &layer.skirt, z, ExtrusionRole::Skirt, true);
            emit_paths(&mut vertices, &layer.brim, z, ExtrusionRole::Brim, true);
            emit_paths(
                &mut vertices,
                &layer.prime_tower,
                z,
                ExtrusionRole::PrimeTower,
                true,
            );
            emit_paths(
                &mut vertices,
                &layer.support,
                z,
                ExtrusionRole::Support,
                false,
            );
            emit_paths(
                &mut vertices,
                &layer.support_interface,
                z,
                ExtrusionRole::SupportInterface,
                false,
            );
            emit_paths(
                &mut vertices,
                &layer.outer_walls,
                z,
                ExtrusionRole::OuterWall,
                true,
            );
            emit_paths(
                &mut vertices,
                &layer.inner_walls,
                z,
                ExtrusionRole::InnerWall,
                true,
            );
            emit_paths(
                &mut vertices,
                &layer.infill,
                z,
                ExtrusionRole::Infill,
                false,
            );
            emit_paths(
                &mut vertices,
                &layer.combined_infill,
                z,
                ExtrusionRole::Infill,
                false,
            );
            emit_paths(
                &mut vertices,
                &layer.solid_infill,
                z,
                ExtrusionRole::SolidInfill,
                false,
            );
            emit_paths(
                &mut vertices,
                &layer.floating_vertical_shell,
                z,
                ExtrusionRole::FloatingVerticalShell,
                false,
            );
            emit_paths(
                &mut vertices,
                &layer.bridge,
                z,
                ExtrusionRole::Bridge,
                false,
            );
            emit_paths(
                &mut vertices,
                &layer.bottom_surface,
                z,
                ExtrusionRole::BottomSurface,
                false,
            );
            emit_paths(
                &mut vertices,
                &layer.top_surface,
                z,
                ExtrusionRole::TopSurface,
                false,
            );
            emit_paths(
                &mut vertices,
                &layer.ironing,
                z,
                ExtrusionRole::Ironing,
                layer.ironing.iter().any(|p| p.len() > 2),
            );
            emit_paths(
                &mut vertices,
                &layer.support_ironing,
                z,
                ExtrusionRole::Ironing,
                false,
            );
            layer_vertex_ends.push(vertices.len());
        }
        Self {
            vertices,
            layer_zs,
            layer_vertex_ends,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.vertices.is_empty()
    }

    pub fn visible(
        &self,
        max_z: f32,
        max_vertices: usize,
        hide: impl Fn(ExtrusionRole) -> bool,
    ) -> impl Iterator<Item = &ToolpathVertex> {
        self.vertices
            .iter()
            .take(if max_vertices == 0 {
                self.vertices.len()
            } else {
                max_vertices.min(self.vertices.len())
            })
            .filter(move |v| v.position.z <= max_z + 1e-4 && !hide(v.role))
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
