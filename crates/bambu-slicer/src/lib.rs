#![forbid(unsafe_code)]

mod clip;
mod infill;
mod ironing;
mod perimeters;
mod prepare_infill;
mod seams;
mod skirt_brim;
mod slice_plane;
mod slicing;
mod steps;
mod support;

use bambu_config::SliceSettings;
use bambu_geom::{offset_polygons, union_polygons, Polygon, Polyline, TriangleMesh};
use thiserror::Error;

pub use slice_plane::{loops_from_segments, point_from_xy_mm, slice_at_z};
pub use slicing::{generate_object_layers, layer_plan, layer_z_values, LayerSpec};
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
    /// Slab thickness (`Layer::height`). First layer may differ from the rest.
    pub height_mm: f64,
    /// Top of the slab (`Layer::print_z`); G-code Z and toolpath preview.
    pub print_z_mm: f64,
    pub contours: Vec<Polygon>,
    pub outer_walls: Vec<Polyline>,
    pub inner_walls: Vec<Polyline>,
    pub infill_region: Vec<Polygon>,
    pub infill: Vec<Polyline>,
    pub solid_infill: Vec<Polyline>,
    pub top_surface: Vec<Polyline>,
    pub bottom_surface: Vec<Polyline>,
    pub bridge: Vec<Polyline>,
    pub support: Vec<Polyline>,
    pub support_interface: Vec<Polyline>,
    pub support_region: Vec<Polygon>,
    pub skirt: Vec<Polyline>,
    pub brim: Vec<Polyline>,
    pub ironing: Vec<Polyline>,
    /// Top-shell polygons (before infill fill), used by ironing.
    pub top_region: Vec<Polygon>,
}

impl Layer {
    pub fn perimeters(&self) -> impl Iterator<Item = &Polyline> {
        self.outer_walls.iter().chain(self.inner_walls.iter())
    }
}

#[derive(Debug, Clone)]
pub struct SliceResult {
    pub layers: Vec<Layer>,
}

pub fn slice_mesh(
    mesh: &TriangleMesh,
    settings: &SliceSettings,
) -> Result<SliceResult, SlicerError> {
    let plan = layer_plan(mesh, settings)?;
    let contours = plan
        .iter()
        .copied()
        .map(|spec| {
            (
                spec,
                union_polygons(&slice_at_z(mesh, spec.slice_z_mm as f32)),
            )
        })
        .collect();
    Ok(slice_from_contours(contours, settings, Some(mesh)))
}

/// Pair GPU/CPU contour samples with the [`LayerSpec`] plan (same order as `slice_z`).
pub fn zip_plan_contours(
    plan: &[LayerSpec],
    contours: Vec<(f64, Vec<Polygon>)>,
) -> Vec<(LayerSpec, Vec<Polygon>)> {
    plan.iter()
        .copied()
        .zip(contours.into_iter().map(|(_, polys)| polys))
        .collect()
}

/// Toolpath generation from already-computed layer contours (CPU Clipper).
/// Contours may come from the CPU plane slicer or Vulkan compute readback.
pub fn slice_from_contours(
    layers: Vec<(LayerSpec, Vec<Polygon>)>,
    settings: &SliceSettings,
    mesh: Option<&TriangleMesh>,
) -> SliceResult {
    let mut out = Vec::new();
    let mut seam_hint = None;
    let mut index = 0usize;
    for (spec, mut contours) in layers {
        contours = union_polygons(&contours);
        if spec.index == 0 && settings.elephant_foot_mm > 1e-9 {
            let shrunk = offset_polygons(&contours, -settings.elephant_foot_mm);
            if !shrunk.is_empty() {
                contours = union_polygons(&shrunk);
            }
        }
        if contours.is_empty() {
            continue;
        }
        let peri = perimeters::classic_perimeters(&contours, settings, seam_hint);
        seam_hint = peri.seam_hint;
        out.push(Layer {
            z_mm: spec.slice_z_mm,
            index,
            height_mm: spec.height_mm,
            print_z_mm: spec.print_z_mm,
            contours,
            outer_walls: peri.outer,
            inner_walls: peri.inner,
            infill_region: peri.infill_region,
            infill: Vec::new(),
            solid_infill: Vec::new(),
            top_surface: Vec::new(),
            bottom_surface: Vec::new(),
            bridge: Vec::new(),
            support: Vec::new(),
            support_interface: Vec::new(),
            support_region: Vec::new(),
            skirt: Vec::new(),
            brim: Vec::new(),
            ironing: Vec::new(),
            top_region: Vec::new(),
        });
        index += 1;
    }

    prepare_infill::apply(&mut out, settings, mesh);
    ironing::apply(&mut out, settings);
    support::apply_classic(&mut out, settings);
    if let Some(first) = out.first() {
        let brim = skirt_brim::brim(&first.contours, settings);
        let footprint = support::first_layer_footprint(first);
        let skirt = skirt_brim::skirt(&footprint, settings);
        if let Some(first) = out.first_mut() {
            first.brim = brim;
            first.skirt = skirt;
        }
    }

    SliceResult { layers: out }
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
    use bambu_config::{InfillPattern, SeamPosition, SliceSettings};
    use bambu_geom::TriangleMesh;

    #[test]
    fn cube_layer_count() {
        let mesh = TriangleMesh::cube(20.0);
        let settings = SliceSettings::default();
        let result = slice_mesh(&mesh, &settings).unwrap();
        assert!(
            (90..=105).contains(&result.layers.len()),
            "layers={}",
            result.layers.len()
        );
        let mid = &result.layers[result.layers.len() / 2];
        assert!(mid.perimeters().next().is_some());
        assert!(!mid.infill.is_empty());
        assert_eq!(mid.outer_walls.len(), 1);
        assert!(!mid.inner_walls.is_empty());
    }

    #[test]
    fn two_walls_more_than_one() {
        let mesh = TriangleMesh::cube(20.0);
        let mut one = SliceSettings::default();
        one.wall_loops = 1;
        one.infill_pattern = InfillPattern::Rectilinear;
        let mut two = one.clone();
        two.wall_loops = 2;
        let a = slice_mesh(&mesh, &one).unwrap();
        let b = slice_mesh(&mesh, &two).unwrap();
        let mid_a = &a.layers[a.layers.len() / 2];
        let mid_b = &b.layers[b.layers.len() / 2];
        assert!(mid_a.inner_walls.is_empty());
        assert!(!mid_b.inner_walls.is_empty());
    }

    #[test]
    fn rear_seam_on_outer_wall() {
        let mesh = TriangleMesh::cube(20.0);
        let mut settings = SliceSettings::default();
        settings.seam = SeamPosition::Rear;
        settings.infill_pattern = InfillPattern::Rectilinear;
        let result = slice_mesh(&mesh, &settings).unwrap();
        let mid = &result.layers[result.layers.len() / 2];
        let start_y = mid.outer_walls[0][0].to_mm().1;
        let max_y = mid.outer_walls[0]
            .iter()
            .map(|p| p.to_mm().1)
            .fold(f64::MIN, f64::max);
        assert!(
            (start_y - max_y).abs() < 0.05,
            "start_y={start_y} max_y={max_y}"
        );
    }

    #[test]
    fn cube_gets_skirt_not_support() {
        let mesh = TriangleMesh::cube(20.0);
        let settings = SliceSettings::default();
        let result = slice_mesh(&mesh, &settings).unwrap();
        let first = &result.layers[0];
        assert_eq!(first.skirt.len(), settings.skirt_loops as usize);
        assert!(first.brim.is_empty());
        assert!(result.layers.iter().all(|l| l.support.is_empty()));
    }

    #[test]
    fn brim_loops_scale_with_width() {
        let mesh = TriangleMesh::cube(20.0);
        let mut settings = SliceSettings::default();
        settings.skirt_loops = 0;
        settings.brim_width_mm = settings.line_width_mm * 3.0;
        let result = slice_mesh(&mesh, &settings).unwrap();
        assert_eq!(result.layers[0].brim.len(), 3);
    }

    #[test]
    fn table_overhang_gets_classic_support() {
        let mesh = TriangleMesh::overhang_table(8.0, 8.0, 24.0, 4.0);
        let mut settings = SliceSettings::default();
        settings.enable_support = true;
        settings.infill_pattern = InfillPattern::Rectilinear;
        let result = slice_mesh(&mesh, &settings).unwrap();
        let support_layers = result
            .layers
            .iter()
            .filter(|l| !l.support.is_empty() || !l.support_interface.is_empty())
            .count();
        assert!(
            support_layers >= 10,
            "expected support under the slab, got {support_layers} layers"
        );
        let cube = slice_mesh(&TriangleMesh::cube(20.0), &settings).unwrap();
        assert!(cube
            .layers
            .iter()
            .all(|l| l.support.is_empty() && l.support_interface.is_empty()));
    }

    #[test]
    fn cube_top_and_bottom_shells() {
        let mesh = TriangleMesh::cube(20.0);
        let settings = SliceSettings::default();
        let result = slice_mesh(&mesh, &settings).unwrap();
        let n = result.layers.len();
        let bottom_n = settings.bottom_shell_layers as usize;
        let top_n = settings.top_shell_layers as usize;
        assert!(!result.layers[0].bottom_surface.is_empty());
        assert!(result.layers[0].infill.is_empty());
        assert!(!result.layers[n - 1].top_surface.is_empty());
        for layer in result.layers.iter().take(bottom_n).skip(1) {
            assert!(
                !layer.solid_infill.is_empty() || !layer.bottom_surface.is_empty(),
                "expected solid bottom shell on layer {}",
                layer.index
            );
        }
        for layer in result.layers.iter().skip(n - top_n).take(top_n - 1) {
            assert!(
                !layer.solid_infill.is_empty() || !layer.top_surface.is_empty(),
                "expected solid top shell on layer {}",
                layer.index
            );
        }
        let mid = &result.layers[n / 2];
        assert!(mid.top_surface.is_empty());
        assert!(mid.bottom_surface.is_empty());
        assert!(mid.solid_infill.is_empty());
        assert!(!mid.infill.is_empty());
    }

    #[test]
    fn bbl_0_20_cube_has_brim_not_skirt() {
        let mesh = TriangleMesh::cube(20.0);
        let settings = SliceSettings::bbl_0_20();
        let result = slice_mesh(&mesh, &settings).unwrap();
        assert!(result.layers[0].skirt.is_empty());
        assert!(!result.layers[0].brim.is_empty());
        assert!((90..=105).contains(&result.layers.len()));
    }

    #[test]
    fn table_overhang_is_bridged() {
        let mesh = TriangleMesh::overhang_table(8.0, 8.0, 24.0, 4.0);
        let mut settings = SliceSettings::default();
        settings.enable_support = false;
        settings.infill_pattern = InfillPattern::Rectilinear;
        let result = slice_mesh(&mesh, &settings).unwrap();
        let bridges = result
            .layers
            .iter()
            .filter(|l| !l.bridge.is_empty())
            .count();
        assert!(
            bridges >= 1,
            "expected bridge fill on the slab overhang, got {bridges}"
        );
    }

    #[test]
    fn cube_top_surfaces_get_ironing() {
        let mesh = TriangleMesh::cube(20.0);
        let mut settings = SliceSettings::default();
        settings.ironing_type = bambu_config::IroningType::TopSurfaces;
        let result = slice_mesh(&mesh, &settings).unwrap();
        let n = result.layers.len();
        assert!(
            !result.layers[n - 1].ironing.is_empty(),
            "expected ironing on the topmost layer"
        );
        let ironed = result
            .layers
            .iter()
            .filter(|l| !l.ironing.is_empty())
            .count();
        // C++ `top` irons `stTop` only (exposed tops), not every top-shell layer.
        assert_eq!(ironed, 1);
        let mid = &result.layers[n / 2];
        assert!(mid.ironing.is_empty());
        assert!(mid.top_region.is_empty());
    }

    #[test]
    fn all_solid_irons_shells_without_sparse() {
        let mesh = TriangleMesh::cube(20.0);
        let mut settings = SliceSettings::default();
        settings.ironing_type = bambu_config::IroningType::AllSolid;
        let result = slice_mesh(&mesh, &settings).unwrap();
        let ironed = result
            .layers
            .iter()
            .filter(|l| !l.ironing.is_empty())
            .count();
        assert!(
            ironed > 1,
            "AllSolid should iron more than the exposed top, got {ironed}"
        );
        let mid = result.layers.len() / 2;
        assert!(
            result.layers[mid].ironing.is_empty(),
            "sparse gyroid middle should not be ironed"
        );
    }

    #[test]
    fn topmost_only_irons_last_layer() {
        let mesh = TriangleMesh::cube(20.0);
        let mut settings = SliceSettings::default();
        settings.ironing_type = bambu_config::IroningType::TopmostOnly;
        let result = slice_mesh(&mesh, &settings).unwrap();
        let n = result.layers.len();
        assert!(!result.layers[n - 1].ironing.is_empty());
        assert!(result.layers[..n - 1].iter().all(|l| l.ironing.is_empty()));
    }

    #[test]
    fn default_settings_skip_ironing() {
        let mesh = TriangleMesh::cube(20.0);
        let result = slice_mesh(&mesh, &SliceSettings::default()).unwrap();
        assert!(result.layers.iter().all(|l| l.ironing.is_empty()));
    }

    #[test]
    fn first_layer_uses_print_z_and_height() {
        let mesh = TriangleMesh::cube(20.0);
        let mut settings = SliceSettings::default();
        settings.first_layer_height_mm = 0.28;
        settings.layer_height_mm = 0.2;
        let result = slice_mesh(&mesh, &settings).unwrap();
        let first = &result.layers[0];
        assert!((first.height_mm - 0.28).abs() < 1e-6);
        assert!((first.print_z_mm - 0.28).abs() < 1e-6);
        assert!((first.z_mm - 0.14).abs() < 1e-6);
        let second = &result.layers[1];
        assert!((second.height_mm - 0.2).abs() < 1e-6);
        assert!((second.print_z_mm - 0.48).abs() < 1e-6);
    }

    #[test]
    fn elephant_foot_shrinks_first_layer_only() {
        let mesh = TriangleMesh::cube(20.0);
        let mut settings = SliceSettings::default();
        settings.infill_pattern = InfillPattern::Rectilinear;
        let plain = slice_mesh(&mesh, &settings).unwrap();
        settings.elephant_foot_mm = 0.15;
        let compensated = slice_mesh(&mesh, &settings).unwrap();
        let a0 = plain.layers[0]
            .contours
            .iter()
            .map(contour_area_mm2)
            .sum::<f64>();
        let b0 = compensated.layers[0]
            .contours
            .iter()
            .map(contour_area_mm2)
            .sum::<f64>();
        assert!(
            b0 < a0 - 1.0,
            "first layer should shrink under elephant foot: {b0} vs {a0}"
        );
        let a1 = plain.layers[1]
            .contours
            .iter()
            .map(contour_area_mm2)
            .sum::<f64>();
        let b1 = compensated.layers[1]
            .contours
            .iter()
            .map(contour_area_mm2)
            .sum::<f64>();
        assert!((a1 - b1).abs() < 1.0, "upper layers stay uncompensated");
    }

    #[test]
    fn honeycomb3d_fills_sparse_region() {
        let mesh = TriangleMesh::cube(20.0);
        let mut settings = SliceSettings::default();
        settings.infill_pattern = InfillPattern::Honeycomb3D;
        settings.infill_density = 0.15;
        let result = slice_mesh(&mesh, &settings).unwrap();
        let mid = &result.layers[result.layers.len() / 2];
        assert!(!mid.infill.is_empty());
    }

    fn polyline_len_mm(paths: &[Polyline]) -> f64 {
        paths
            .iter()
            .flat_map(|pl| pl.windows(2))
            .map(|w| w[0].distance_mm(w[1]))
            .sum()
    }

    #[test]
    fn lightning_supports_top_shells() {
        let mesh = TriangleMesh::cube(20.0);
        let mut settings = SliceSettings::default();
        settings.infill_pattern = InfillPattern::Lightning;
        settings.infill_density = 0.15;
        let result = slice_mesh(&mesh, &settings).unwrap();
        let sparse_layers: Vec<_> = result
            .layers
            .iter()
            .filter(|l| !l.infill.is_empty())
            .collect();
        assert!(
            sparse_layers.len() >= 3,
            "lightning trees should run through internal layers, got {}",
            sparse_layers.len()
        );
        let top_internal = sparse_layers.last().unwrap();
        assert!(
            !top_internal.infill.is_empty(),
            "trees should support the underside of the top shells"
        );
    }

    #[test]
    fn lightning_uses_less_filament_than_grid() {
        let mesh = TriangleMesh::cube(20.0);
        let mut lightning = SliceSettings::default();
        lightning.infill_pattern = InfillPattern::Lightning;
        lightning.infill_density = 0.15;
        let mut grid = lightning.clone();
        grid.infill_pattern = InfillPattern::Grid;
        let a = slice_mesh(&mesh, &lightning).unwrap();
        let b = slice_mesh(&mesh, &grid).unwrap();
        let mid = a.layers.len() / 2;
        let lightning_len = polyline_len_mm(&a.layers[mid].infill);
        let grid_len = polyline_len_mm(&b.layers[mid].infill);
        assert!(
            lightning_len > 1.0,
            "middle layer still has supporting trees ({lightning_len})"
        );
        assert!(
            lightning_len < grid_len * 0.75,
            "lightning {lightning_len} should be sparser than grid {grid_len}"
        );
    }

    #[test]
    fn adaptive_cubic_fills_sparse_region() {
        let mesh = TriangleMesh::cube(20.0);
        let mut settings = SliceSettings::default();
        settings.infill_pattern = InfillPattern::AdaptiveCubic;
        settings.infill_density = 0.15;
        let result = slice_mesh(&mesh, &settings).unwrap();
        let mid = &result.layers[result.layers.len() / 2];
        assert!(!mid.infill.is_empty());
    }

    #[test]
    fn support_cubic_sparser_than_adaptive() {
        let mesh = TriangleMesh::cube(20.0);
        let mut adaptive = SliceSettings::default();
        adaptive.infill_pattern = InfillPattern::AdaptiveCubic;
        adaptive.infill_density = 0.15;
        let mut support = adaptive.clone();
        support.infill_pattern = InfillPattern::SupportCubic;
        let a = slice_mesh(&mesh, &adaptive).unwrap();
        let b = slice_mesh(&mesh, &support).unwrap();
        let mid = a.layers.len() / 2;
        let adaptive_len = polyline_len_mm(&a.layers[mid].infill);
        let support_len = polyline_len_mm(&b.layers[mid].infill);
        assert!(
            adaptive_len > 1.0,
            "adaptive cubic should fill the cube ({adaptive_len})"
        );
        assert!(
            support_len < adaptive_len,
            "support cubic {support_len} should be sparser than adaptive {adaptive_len}"
        );
    }
}
