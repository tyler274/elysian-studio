//! FFF slice step machine.
//!
//! **CPU pool:** Rayon replaces TBB. Independent layers (contours, infill,
//! ironing, classic support fills) use ordered `par_iter` / `par_chunks` so
//! join-by-index matches `RAYON_NUM_THREADS=1`. Do not `ThreadPoolBuilder::install`
//! inside a Rayon job. Perimeters stay sequential because `seam_hint` chains
//! across layers. Lightning infill, tree drop, and downward support columns stay sequential.
//! Tokio is UI/LAN only; wgpu stays on the GPU queue. Clipper2 is per-job — never
//! share a document across threads.
//!
//! **SIMD:** `wide` `f32x4`/`i64x4`/`f64x4` culls and scanline tests; leftover
//! lanes use the scalar kernel. Integer Clipper stays scalar. Release builds
//! should pass `-C target-cpu=x86-64-v3` (AVX2) or the host NEON equivalent.

#![forbid(unsafe_code)]

mod clip;
mod fuzzy;
mod infill;
mod ironing;
mod perimeters;
mod prepare_infill;
mod raft;
mod seams;
mod skirt_brim;
mod slice_plane;
mod slicing;
mod steps;
mod support;

use bambu_config::SliceSettings;
use bambu_geom::{
    difference_polygons, offset_polygons, union_polygons, Point, Polygon, Polyline, TriangleMesh,
};
use bambu_model::ModelVolume;
use rayon::prelude::*;
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
        .par_iter()
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

/// Slice model-part meshes, then subtract [`bambu_model::VolumeType::Negative`]
/// contours with Clipper (C++ `PrintObjectSlice` negative volume).
pub fn slice_volumes(
    volumes: &[ModelVolume],
    settings: &SliceSettings,
) -> Result<SliceResult, SlicerError> {
    let parts: Vec<&TriangleMesh> = volumes
        .iter()
        .filter(|v| v.volume_type.is_model_part())
        .map(|v| &v.mesh)
        .collect();
    let negatives: Vec<&TriangleMesh> = volumes
        .iter()
        .filter(|v| v.volume_type.is_negative())
        .map(|v| &v.mesh)
        .collect();
    if parts.is_empty() {
        return Err(SlicerError::EmptyMesh);
    }
    if negatives.is_empty() && parts.len() == 1 {
        return slice_mesh(parts[0], settings);
    }
    let mut merged = TriangleMesh::default();
    for part in &parts {
        merged.append(part);
    }
    let plan = layer_plan(&merged, settings)?;
    let contours = plan
        .par_iter()
        .copied()
        .map(|spec| {
            let z = spec.slice_z_mm as f32;
            let mut pos = Vec::new();
            for part in &parts {
                pos.extend(slice_at_z(part, z));
            }
            let pos = union_polygons(&pos);
            if negatives.is_empty() {
                return (spec, pos);
            }
            let mut neg = Vec::new();
            for hole in &negatives {
                neg.extend(slice_at_z(hole, z));
            }
            (spec, difference_polygons(&pos, &union_polygons(&neg)))
        })
        .collect();
    Ok(slice_from_contours(contours, settings, Some(&merged)))
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
    let prepared: Vec<_> = layers
        .into_par_iter()
        .map(|(spec, contours)| prepare_layer_contours(spec, contours, settings))
        .collect();
    let prepared: Vec<_> = prepared
        .into_iter()
        .filter(|(_, contours)| !contours.is_empty())
        .collect();

    let mut out = Vec::new();
    let mut seam_hint = None;
    for i in 0..prepared.len() {
        let upper = prepared.get(i + 1).map(|(_, c)| c.as_slice());
        let peri = perimeters::generate(&prepared[i].1, settings, seam_hint, upper);
        seam_hint = peri.seam_hint;
        let (spec, contours) = prepared[i].clone();
        let mut outer_walls = peri.outer;
        let mut inner_walls = peri.inner;
        fuzzy::apply_walls(
            &mut outer_walls,
            &mut inner_walls,
            settings,
            spec.index,
            spec.slice_z_mm,
        );
        out.push(Layer {
            z_mm: spec.slice_z_mm,
            index: i,
            height_mm: spec.height_mm,
            print_z_mm: spec.print_z_mm,
            contours,
            outer_walls,
            inner_walls,
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
    }

    prepare_infill::apply(&mut out, settings, mesh);
    ironing::apply(&mut out, settings);
    support::apply(&mut out, settings);
    raft::apply(&mut out, settings);
    if let Some(first) = out.first() {
        let rafted = settings.raft_layers > 0;
        let brim = if rafted {
            Vec::new()
        } else {
            skirt_brim::brim(&first.contours, settings)
        };
        let footprint = support::first_layer_footprint(first);
        let mut skirt_settings = settings.clone();
        if rafted {
            skirt_settings.brim_width_mm = 0.0;
        }
        let skirt = skirt_brim::skirt(&footprint, &skirt_settings);
        if let Some(first) = out.first_mut() {
            first.brim = brim;
            first.skirt = skirt;
        }
    }

    SliceResult { layers: out }
}

fn prepare_layer_contours(
    spec: LayerSpec,
    mut contours: Vec<Polygon>,
    settings: &SliceSettings,
) -> (LayerSpec, Vec<Polygon>) {
    contours = union_polygons(&contours);
    contours = compensate_xy(
        &contours,
        settings.xy_contour_compensation_mm,
        settings.xy_hole_compensation_mm,
    );
    if spec.index == 0 && settings.elephant_foot_mm > 1e-9 {
        let shrunk = offset_polygons(&contours, -settings.elephant_foot_mm);
        if !shrunk.is_empty() {
            contours = union_polygons(&shrunk);
        }
    }
    (spec, contours)
}

/// Bambu `_shrink_contour_holes`: offset outer rings by `contour_mm` and holes
/// by `-hole_mm` (positive hole compensation enlarges holes).
fn compensate_xy(polygons: &[Polygon], contour_mm: f64, hole_mm: f64) -> Vec<Polygon> {
    if contour_mm.abs() < 1e-9 && hole_mm.abs() < 1e-9 {
        return polygons.to_vec();
    }
    let mut outers = Vec::new();
    let mut holes = Vec::new();
    for (i, poly) in polygons.iter().enumerate() {
        let c = ring_centroid(poly);
        if clip::point_in_polygons_skip(c, polygons, i) {
            holes.push(poly.clone());
        } else {
            outers.push(poly.clone());
        }
    }
    let mut acc = if contour_mm.abs() < 1e-9 {
        outers
    } else {
        offset_polygons(&outers, contour_mm)
    };
    if acc.is_empty() {
        return polygons.to_vec();
    }
    if !holes.is_empty() {
        let hole_offs = if hole_mm.abs() < 1e-9 {
            holes
        } else {
            offset_polygons(&holes, -hole_mm)
        };
        acc = difference_polygons(&acc, &hole_offs);
    }
    union_polygons(&acc)
}

fn ring_centroid(poly: &[Point]) -> Point {
    let n = poly.len().max(1) as i64;
    Point::new(
        poly.iter().map(|p| p.x).sum::<i64>() / n,
        poly.iter().map(|p| p.y).sum::<i64>() / n,
    )
}

pub fn contour_area_mm2(poly: &Polygon) -> f64 {
    signed_contour_area_mm2(poly).abs()
}

fn signed_contour_area_mm2(poly: &Polygon) -> f64 {
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
    acc * 0.5
}

#[cfg(test)]
mod tests {
    use super::*;
    use bambu_config::{
        FuzzySkinType, InfillPattern, SeamPosition, SliceSettings, SupportType, SurfacePattern,
        TopOneWallType, WallGenerator,
    };
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
    fn arachne_thin_wall_keeps_centerline() {
        let mesh = TriangleMesh::aabb_box(glam::Vec3::ZERO, glam::Vec3::new(0.7, 20.0, 10.0));
        let mut classic = SliceSettings::default();
        classic.infill_pattern = InfillPattern::Rectilinear;
        classic.wall_loops = 2;
        classic.wall_generator = WallGenerator::Classic;
        let mut arachne = classic.clone();
        arachne.wall_generator = WallGenerator::Arachne;
        let a = slice_mesh(&mesh, &classic).unwrap();
        let b = slice_mesh(&mesh, &arachne).unwrap();
        let mid_a = &a.layers[a.layers.len() / 2];
        let mid_b = &b.layers[b.layers.len() / 2];
        assert_eq!(mid_a.outer_walls.len(), 1);
        assert!(mid_a.inner_walls.is_empty());
        assert_eq!(mid_b.outer_walls.len(), 1);
        assert!(
            !mid_b.inner_walls.is_empty(),
            "arachne should keep a leftover centerline on a 0.7 mm wall"
        );
        let classic_len = polyline_len_mm(&mid_a.outer_walls) + polyline_len_mm(&mid_a.inner_walls);
        let arachne_len = polyline_len_mm(&mid_b.outer_walls) + polyline_len_mm(&mid_b.inner_walls);
        assert!(
            arachne_len > classic_len * 1.4,
            "thin wall arachne={arachne_len} classic={classic_len}"
        );
        let cube = TriangleMesh::cube(20.0);
        let thick_a = slice_mesh(&cube, &classic).unwrap();
        let thick_b = slice_mesh(&cube, &arachne).unwrap();
        let t_a = &thick_a.layers[thick_a.layers.len() / 2];
        let t_b = &thick_b.layers[thick_b.layers.len() / 2];
        assert_eq!(t_a.outer_walls.len(), t_b.outer_walls.len());
        assert_eq!(t_a.inner_walls.len(), t_b.inner_walls.len());
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
    fn one_wall_topmost_drops_last_layer_inner_walls() {
        let mesh = TriangleMesh::cube(20.0);
        let mut settings = SliceSettings::default();
        settings.infill_pattern = InfillPattern::Rectilinear;
        settings.top_one_wall = TopOneWallType::Topmost;
        let result = slice_mesh(&mesh, &settings).unwrap();
        let n = result.layers.len();
        let last = &result.layers[n - 1];
        let mid = &result.layers[n / 2];
        assert!(!last.outer_walls.is_empty());
        assert!(last.inner_walls.is_empty());
        assert!(!mid.inner_walls.is_empty());
        assert!(!last.top_surface.is_empty());
    }

    #[test]
    fn one_wall_all_top_opens_terrace() {
        let mut mesh = TriangleMesh::aabb_box(glam::Vec3::ZERO, glam::Vec3::new(20.0, 20.0, 10.0));
        mesh.append(&TriangleMesh::aabb_box(
            glam::Vec3::new(5.0, 5.0, 10.0),
            glam::Vec3::new(15.0, 15.0, 20.0),
        ));
        let mut settings = SliceSettings::default();
        settings.infill_pattern = InfillPattern::Rectilinear;
        settings.wall_loops = 2;
        let mut none = settings.clone();
        none.top_one_wall = TopOneWallType::None;
        let mut all_top = settings.clone();
        all_top.top_one_wall = TopOneWallType::AllTop;
        let a = slice_mesh(&mesh, &none).unwrap();
        let b = slice_mesh(&mesh, &all_top).unwrap();
        let terrace = a
            .layers
            .iter()
            .position(|l| (l.print_z_mm - 10.0).abs() < 0.15)
            .expect("layer at the 10 mm step");
        assert!(
            !a.layers[terrace].inner_walls.is_empty(),
            "full walls on the terrace without the option"
        );
        let none_inner = polyline_len_mm(&a.layers[terrace].inner_walls);
        let all_inner = polyline_len_mm(&b.layers[terrace].inner_walls);
        assert!(
            all_inner < none_inner * 0.7,
            "AllTop should skip terrace inner walls: {all_inner} vs {none_inner}"
        );
        let last = b.layers.last().unwrap();
        assert!(last.inner_walls.is_empty());
        assert!(!last.outer_walls.is_empty());
    }

    #[test]
    fn fuzzy_skin_jitters_outer_walls_not_first_layer() {
        let mesh = TriangleMesh::cube(20.0);
        let mut settings = SliceSettings::default();
        settings.infill_pattern = InfillPattern::Rectilinear;
        let plain = slice_mesh(&mesh, &settings).unwrap();
        settings.fuzzy_skin = FuzzySkinType::External;
        let fuzzy = slice_mesh(&mesh, &settings).unwrap();
        let mid = plain.layers.len() / 2;
        let plain_n: usize = plain.layers[mid].outer_walls.iter().map(|p| p.len()).sum();
        let fuzzy_n: usize = fuzzy.layers[mid].outer_walls.iter().map(|p| p.len()).sum();
        assert!(
            fuzzy_n > plain_n * 5,
            "mid outer wall should gain jitter points: {fuzzy_n} vs {plain_n}"
        );
        let first_plain: usize = plain.layers[0].outer_walls.iter().map(|p| p.len()).sum();
        let first_fuzzy: usize = fuzzy.layers[0].outer_walls.iter().map(|p| p.len()).sum();
        assert_eq!(
            first_plain, first_fuzzy,
            "first layer stays unfuzzed unless fuzzy_skin_first_layer"
        );
        let inner_plain: usize = plain.layers[mid].inner_walls.iter().map(|p| p.len()).sum();
        let inner_fuzzy: usize = fuzzy.layers[mid].inner_walls.iter().map(|p| p.len()).sum();
        assert_eq!(
            inner_plain, inner_fuzzy,
            "External leaves inner walls alone"
        );
        let again = slice_mesh(&mesh, &settings).unwrap();
        assert_eq!(
            fuzzy.layers[mid].outer_walls, again.layers[mid].outer_walls,
            "seeded noise is stable"
        );
    }

    #[test]
    fn fuzzy_all_walls_jitters_inner_loops() {
        let mesh = TriangleMesh::cube(20.0);
        let mut settings = SliceSettings::default();
        settings.infill_pattern = InfillPattern::Rectilinear;
        let plain = slice_mesh(&mesh, &settings).unwrap();
        settings.fuzzy_skin = FuzzySkinType::AllWalls;
        let fuzzy = slice_mesh(&mesh, &settings).unwrap();
        let mid = plain.layers.len() / 2;
        let inner_plain: usize = plain.layers[mid].inner_walls.iter().map(|p| p.len()).sum();
        let inner_fuzzy: usize = fuzzy.layers[mid].inner_walls.iter().map(|p| p.len()).sum();
        assert!(
            inner_fuzzy > inner_plain * 3,
            "AllWalls should jitter inner walls: {inner_fuzzy} vs {inner_plain}"
        );
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
        settings.support_type = SupportType::Classic;
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
    fn table_overhang_gets_tree_support() {
        let mesh = TriangleMesh::overhang_table(8.0, 8.0, 24.0, 4.0);
        let mut tree = SliceSettings::default();
        tree.enable_support = true;
        tree.support_type = SupportType::Tree;
        tree.infill_pattern = InfillPattern::Rectilinear;
        let mut classic = tree.clone();
        classic.support_type = SupportType::Classic;
        let a = slice_mesh(&mesh, &tree).unwrap();
        let b = slice_mesh(&mesh, &classic).unwrap();
        let tree_layers = a
            .layers
            .iter()
            .filter(|l| !l.support.is_empty() || !l.support_interface.is_empty())
            .count();
        assert!(
            tree_layers >= 10,
            "expected tree trunks under the slab, got {tree_layers} layers"
        );
        assert!(!a.layers[0].support.is_empty() || !a.layers[0].support_interface.is_empty());
        let region_area = |layers: &[Layer]| {
            layers
                .iter()
                .map(|l| l.support_region.iter().map(contour_area_mm2).sum::<f64>())
                .sum::<f64>()
        };
        let tree_area = region_area(&a.layers);
        let classic_area = region_area(&b.layers);
        assert!(
            tree_area < classic_area * 0.5,
            "tree footprint {tree_area} should be smaller than classic columns {classic_area}"
        );
        let cube = slice_mesh(&TriangleMesh::cube(20.0), &tree).unwrap();
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
    fn raft_prepends_layers_and_raises_object() {
        let mesh = TriangleMesh::cube(20.0);
        let mut settings = SliceSettings::default();
        settings.infill_pattern = InfillPattern::Rectilinear;
        let plain = slice_mesh(&mesh, &settings).unwrap();
        settings.raft_layers = 2;
        let rafted = slice_mesh(&mesh, &settings).unwrap();
        assert_eq!(rafted.layers.len(), plain.layers.len() + 2);

        let raft0 = &rafted.layers[0];
        let raft1 = &rafted.layers[1];
        assert!(raft0.outer_walls.is_empty());
        assert!(raft1.outer_walls.is_empty());
        assert!(!raft0.support.is_empty());
        assert!(!raft1.support_interface.is_empty());
        assert!((raft0.print_z_mm - 0.2).abs() < 1e-6);
        assert!((raft1.print_z_mm - 0.5).abs() < 1e-6);
        assert!(!raft0.skirt.is_empty());
        assert!(raft0.brim.is_empty());

        let object0 = &rafted.layers[2];
        assert!(!object0.outer_walls.is_empty());
        assert!((object0.print_z_mm - 0.8).abs() < 1e-6);
        assert_eq!(object0.index, 2);

        let raft_area: f64 = raft0.contours.iter().map(contour_area_mm2).sum();
        let object_area: f64 = object0.contours.iter().map(contour_area_mm2).sum();
        assert!(
            raft_area > object_area + 10.0,
            "raft flange should expand past the object: {raft_area} vs {object_area}"
        );
        let last = rafted.layers.last().unwrap();
        let plain_last = plain.layers.last().unwrap();
        assert!((last.print_z_mm - (plain_last.print_z_mm + 0.6)).abs() < 1e-6);
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

    fn scanline_dx_signs(paths: &[Polyline]) -> Vec<i32> {
        paths
            .iter()
            .filter(|pl| pl.len() >= 2)
            .map(|pl| {
                let dx = pl[pl.len() - 1].x - pl[0].x;
                dx.signum() as i32
            })
            .filter(|s| *s != 0)
            .collect()
    }

    #[test]
    fn monotonic_top_scanlines_share_direction() {
        let mesh = TriangleMesh::cube(20.0);
        let mut zigzag = SliceSettings::default();
        zigzag.top_surface_pattern = SurfacePattern::Rectilinear;
        let mut monotonic = zigzag.clone();
        monotonic.top_surface_pattern = SurfacePattern::MonotonicLine;
        let a = slice_mesh(&mesh, &zigzag).unwrap();
        let b = slice_mesh(&mesh, &monotonic).unwrap();
        let top_z = a.layers.last().unwrap();
        let top_m = b.layers.last().unwrap();
        let zig = scanline_dx_signs(&top_z.top_surface);
        let mono = scanline_dx_signs(&top_m.top_surface);
        assert!(
            zig.iter().any(|s| *s < 0) && zig.iter().any(|s| *s > 0),
            "zig-zag top should reverse every other line: {zig:?}"
        );
        assert!(
            !mono.is_empty() && mono.iter().all(|s| *s == mono[0]),
            "monotonic top should keep one direction: {mono:?}"
        );
    }

    #[test]
    fn xy_contour_compensation_grows_all_layers() {
        let mesh = TriangleMesh::cube(20.0);
        let plain = SliceSettings::default();
        let mut grown = plain.clone();
        grown.xy_contour_compensation_mm = 0.4;
        grown.elephant_foot_mm = 0.0;
        let a = slice_mesh(&mesh, &plain).unwrap();
        let b = slice_mesh(&mesh, &grown).unwrap();
        let area = |layers: &[Layer], i: usize| {
            layers[i].contours.iter().map(contour_area_mm2).sum::<f64>()
        };
        assert!(
            area(&b.layers, 0) > area(&a.layers, 0) + 5.0,
            "first layer should grow"
        );
        let mid = a.layers.len() / 2;
        assert!(
            area(&b.layers, mid) > area(&a.layers, mid) + 5.0,
            "inner layers should grow"
        );
    }

    #[test]
    fn n_threads_match_single_thread() {
        let mesh = TriangleMesh::cube(20.0);
        let mut settings = SliceSettings::default();
        settings.infill_pattern = InfillPattern::Rectilinear;
        settings.ironing_type = bambu_config::IroningType::TopSurfaces;
        let one = rayon::ThreadPoolBuilder::new()
            .num_threads(1)
            .build()
            .unwrap()
            .install(|| slice_mesh(&mesh, &settings).unwrap());
        let many = rayon::ThreadPoolBuilder::new()
            .num_threads(4)
            .build()
            .unwrap()
            .install(|| slice_mesh(&mesh, &settings).unwrap());
        assert_eq!(one.layers.len(), many.layers.len());
        for (a, b) in one.layers.iter().zip(&many.layers) {
            assert_eq!(a.print_z_mm, b.print_z_mm);
            assert_eq!(a.contours, b.contours);
            assert_eq!(a.outer_walls, b.outer_walls);
            assert_eq!(a.inner_walls, b.inner_walls);
            assert_eq!(a.infill, b.infill);
            assert_eq!(a.solid_infill, b.solid_infill);
            assert_eq!(a.top_surface, b.top_surface);
            assert_eq!(a.ironing, b.ironing);
        }
    }

    #[test]
    fn negative_volume_cuts_a_hole() {
        let settings = SliceSettings::default();
        let solid = TriangleMesh::cube(20.0);
        let mut cutter = TriangleMesh::cube(10.0);
        cutter.translate(glam::Vec3::new(5.0, 5.0, 5.0));
        let mut hole = bambu_model::ModelVolume::model_part("cut", cutter, 2);
        hole.volume_type = bambu_model::VolumeType::Negative;
        let volumes = vec![
            bambu_model::ModelVolume::model_part("body", solid.clone(), 1),
            hole,
        ];
        let solid_slice = slice_mesh(&solid, &settings).unwrap();
        let cut = slice_volumes(&volumes, &settings).unwrap();
        let mid_solid = &solid_slice.layers[solid_slice.layers.len() / 2];
        let mid_cut = &cut.layers[cut.layers.len() / 2];
        let net_area = |layer: &Layer| {
            layer
                .contours
                .iter()
                .map(signed_contour_area_mm2)
                .sum::<f64>()
                .abs()
        };
        let a = net_area(mid_solid);
        let b = net_area(mid_cut);
        assert!(
            b < a - 50.0,
            "negative volume should shrink net contour area: solid={a} cut={b}"
        );
        assert!(
            mid_cut.contours.len() > mid_solid.contours.len(),
            "expected a hole ring, solid={} cut={}",
            mid_solid.contours.len(),
            mid_cut.contours.len()
        );
    }
}
