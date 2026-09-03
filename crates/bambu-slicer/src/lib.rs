#![forbid(unsafe_code)]

mod infill;
mod slice_plane;
mod steps;

use bambu_config::SliceSettings;
use bambu_geom::{offset_polygons, union_polygons, Polygon, Polyline, TriangleMesh};
use thiserror::Error;

pub use steps::{PrintObjectStep, PrintStep};

#[derive(Debug, Error)]
pub enum SlicerError {
    #[error("mesh has no triangles")]
    EmptyMesh,
    #[error("mesh has no bounding box")]
    EmptyBounds,
}

#[derive(Debug, Clone)]
pub struct Layer {
    pub z_mm: f64,
    pub index: usize,
    pub perimeters: Vec<Polyline>,
    pub infill: Vec<Polyline>,
}

#[derive(Debug, Clone)]
pub struct SliceResult {
    pub layers: Vec<Layer>,
}

/// Horizontal slice → outer wall centerline → rectilinear infill.
pub fn slice_mesh(
    mesh: &TriangleMesh,
    settings: &SliceSettings,
) -> Result<SliceResult, SlicerError> {
    let aabb = mesh.aabb().ok_or(SlicerError::EmptyBounds)?;
    if mesh.indices.is_empty() {
        return Err(SlicerError::EmptyMesh);
    }

    let z_min = aabb.min.z as f64;
    let z_max = aabb.max.z as f64;
    let lh = settings.layer_height_mm;
    let mut layers = Vec::new();
    let mut index = 0usize;
    let mut z = z_min + lh * 0.5;
    while z < z_max - 1e-6 {
        let mut contours = slice_plane::slice_at_z(mesh, z as f32);
        contours = union_polygons(&contours);
        if contours.is_empty() {
            z += lh;
            continue;
        }

        let wall = offset_polygons(&contours, -settings.line_width_mm * 0.5);
        let perimeters: Vec<Polyline> = wall.iter().cloned().filter(|p| p.len() >= 3).collect();

        let infill_region = offset_polygons(&wall, -settings.line_width_mm);
        let infill = infill::rectilinear(&infill_region, settings.infill_spacing_mm(), index);

        layers.push(Layer {
            z_mm: z,
            index,
            perimeters,
            infill,
        });
        index += 1;
        z += lh;
    }

    Ok(SliceResult { layers })
}

pub fn contour_area_mm2(poly: &Polygon) -> f64 {
    if poly.len() < 3 {
        return 0.0;
    }
    let mut acc = 0.0;
    for i in 0..poly.len() {
        let a = poly[i];
        let b = poly[(i + 1) % poly.len()];
        let (ax, ay) = a.to_mm();
        let (bx, by) = b.to_mm();
        acc += ax * by - bx * ay;
    }
    acc.abs() * 0.5
}

#[cfg(test)]
mod tests {
    use super::*;
    use bambu_geom::TriangleMesh;

    #[test]
    fn cube_layer_count() {
        let mesh = TriangleMesh::cube(20.0);
        let settings = SliceSettings::default();
        let result = slice_mesh(&mesh, &settings).unwrap();
        // 20mm / 0.2mm = 100 mid-layer samples
        assert!(
            (90..=105).contains(&result.layers.len()),
            "layers={}",
            result.layers.len()
        );
        let mid = &result.layers[result.layers.len() / 2];
        assert!(!mid.perimeters.is_empty());
        assert!(!mid.infill.is_empty());
    }
}
