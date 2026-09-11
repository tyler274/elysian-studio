//! `PrintObjectStep::PrepareInfill`: top / bottom / bridge vs sparse regions.
//!
//! Simplified `detect_surfaces_type` + `discover_horizontal_shells` /
//! `discover_vertical_shells`. Neighbor contours are grown slightly so clipper
//! slivers are not treated as shells. Shell windows follow C++ layer count **or**
//! `top_shell_thickness` / `bottom_shell_thickness`. When
//! `ensure_vertical_shell_thickness` is enabled, slope rings become extra
//! internal solid (`diff(infill, intersect(neighbor infills))`), then C++
//! `offset2_ex` / `shrink_ex` regularization. Sparse islands at or below
//! `minimum_sparse_infill_area` become internal solid.
//! Parameter modifiers fill each `LayerRegion` with its own settings (C++).

use bambu_config::{EnsureVerticalShellThickness, FlowRole, InfillPattern, SliceSettings};
use bambu_geom::{
    difference_polygons, intersect_polygons, offset_polygons, offset_polygons_square,
    union_polygons, Point, Polygon, Polyline, TriangleMesh,
};
use rayon::prelude::*;

use crate::infill;
use crate::Layer;

const COVER_MM: f64 = 0.15;
/// C++ `EPSILON` used with shell thickness windows.
const SHELL_THICKNESS_EPSILON: f64 = 1e-4;
/// C++ `min_perimeter_infill_spacing = solid_infill_spacing * 1.05`.
const VERTICAL_SHELL_SPACING_SCALE: f64 = 1.05;

pub fn apply(layers: &mut [Layer], settings: &SliceSettings, mesh: Option<&TriangleMesh>) {
    if layers.is_empty() {
        return;
    }
    let nreg = layers
        .iter()
        .map(|layer| layer.region_infill.len())
        .max()
        .unwrap_or(0);
    if nreg <= 1 {
        fill_into(layers, settings, mesh, false, None);
        return;
    }
    for layer in layers.iter_mut() {
        layer.infill.clear();
        layer.solid_infill.clear();
        layer.floating_vertical_shell.clear();
        layer.floating_areas.clear();
        layer.top_surface.clear();
        layer.bottom_surface.clear();
        layer.bridge.clear();
        layer.top_region.clear();
    }
    let n = layers.len();
    let zs: Vec<f64> = layers.iter().map(|l| l.print_z_mm).collect();
    let union_infill: Vec<Vec<Polygon>> = layers.iter().map(|l| l.infill_region.clone()).collect();
    let mut shells = Vec::with_capacity(nreg);
    let mut cfgs = Vec::with_capacity(nreg);
    for r in 0..nreg {
        let regions: Vec<Vec<Polygon>> = layers
            .iter()
            .map(|layer| layer.region_infill.get(r).cloned().unwrap_or_default())
            .collect();
        let cfg = layers
            .iter()
            .find_map(|layer| layer.region_settings.get(r).cloned())
            .unwrap_or_else(|| settings.clone());
        shells.push(detect_shells(&regions, &zs, &cfg));
        cfgs.push((cfg, regions));
    }
    let mut shared_sparse = vec![Vec::new(); n];
    for map in &shells {
        for (i, polys) in map.sparse.iter().enumerate() {
            append_union(&mut shared_sparse[i], polys.clone());
        }
    }
    for (r, (cfg, regions)) in cfgs.into_iter().enumerate() {
        for (layer, region) in layers.iter_mut().zip(&regions) {
            layer.infill_region = region.clone();
        }
        emit_shells(layers, &cfg, mesh, &shells[r], true, Some(&shared_sparse));
    }
    for (layer, region) in layers.iter_mut().zip(union_infill) {
        layer.infill_region = region;
    }
}

fn fill_into(
    layers: &mut [Layer],
    settings: &SliceSettings,
    mesh: Option<&TriangleMesh>,
    append: bool,
    shared_sparse: Option<&[Vec<Polygon>]>,
) {
    let regions: Vec<Vec<Polygon>> = layers.iter().map(|l| l.infill_region.clone()).collect();
    let zs: Vec<f64> = layers.iter().map(|l| l.print_z_mm).collect();
    let shells = detect_shells(&regions, &zs, settings);
    emit_shells(layers, settings, mesh, &shells, append, shared_sparse);
}

struct ShellMap {
    top: Vec<Vec<Polygon>>,
    bottom: Vec<Vec<Polygon>>,
    solid: Vec<Vec<Polygon>>,
    sparse: Vec<Vec<Polygon>>,
}

fn detect_shells(regions: &[Vec<Polygon>], zs: &[f64], settings: &SliceSettings) -> ShellMap {
    let n = regions.len();
    let top_n = settings.top_shell_layers as usize;
    let bottom_n = settings.bottom_shell_layers as usize;
    let top_th = settings.top_shell_thickness_mm;
    let bottom_th = settings.bottom_shell_thickness_mm;

    let mut top = vec![Vec::new(); n];
    let mut bottom = vec![Vec::new(); n];
    if top_n > 0 {
        top.par_iter_mut().enumerate().for_each(|(i, slot)| {
            let above = regions.get(i + 1).map_or(&[][..], Vec::as_slice);
            *slot = difference_polygons(&regions[i], &cover(above));
        });
    }
    if bottom_n > 0 {
        bottom.par_iter_mut().enumerate().for_each(|(i, slot)| {
            let below = if i > 0 {
                regions[i - 1].as_slice()
            } else {
                &[]
            };
            *slot = difference_polygons(&regions[i], &cover(below));
        });
    }

    let mut solid = vec![Vec::new(); n];
    solid.par_iter_mut().enumerate().for_each(|(i, slot)| {
        let mut acc = top[i].clone();
        acc.extend(bottom[i].iter().cloned());
        *slot = union_polygons(&acc);
    });

    for j in 0..n {
        if top_n > 0 {
            for k in 1..=j {
                let dz = zs.get(j).copied().unwrap_or(0.0) - zs.get(j - k).copied().unwrap_or(0.0);
                if past_shell_window(k, top_n, top_th, dz) {
                    break;
                }
                if !within_shell_window(k, top_n, top_th, dz) {
                    continue;
                }
                let extra = intersect_polygons(&regions[j - k], &top[j]);
                append_union(&mut solid[j - k], extra);
            }
        }
        if bottom_n > 0 {
            for k in 1..(n - j) {
                let dz = zs.get(j + k).copied().unwrap_or(0.0) - zs.get(j).copied().unwrap_or(0.0);
                if past_shell_window(k, bottom_n, bottom_th, dz) {
                    break;
                }
                if !within_shell_window(k, bottom_n, bottom_th, dz) {
                    continue;
                }
                let extra = intersect_polygons(&regions[j + k], &bottom[j]);
                append_union(&mut solid[j + k], extra);
            }
        }
    }

    if settings.ensure_vertical_shell_thickness == EnsureVerticalShellThickness::Enabled {
        for (i, slot) in solid.iter_mut().enumerate() {
            let extra = vertical_shell_extra(i, regions, zs, settings);
            append_union(slot, extra);
        }
    }

    let mut sparse: Vec<Vec<Polygon>> = (0..n)
        .into_par_iter()
        .map(|i| difference_polygons(&regions[i], &solid[i]))
        .collect();
    promote_small_sparse(&mut solid, &mut sparse, settings);
    ShellMap {
        top,
        bottom,
        solid,
        sparse,
    }
}

/// C++ `i < idx + n_layers || z_span < thickness - EPSILON`.
fn within_shell_window(k: usize, n_layers: usize, thickness_mm: f64, z_span: f64) -> bool {
    n_layers > 0
        && (k < n_layers || (thickness_mm > 0.0 && z_span + SHELL_THICKNESS_EPSILON < thickness_mm))
}

fn past_shell_window(k: usize, n_layers: usize, thickness_mm: f64, z_span: f64) -> bool {
    k >= n_layers && (thickness_mm <= 0.0 || z_span + SHELL_THICKNESS_EPSILON >= thickness_mm)
}

/// C++ `LayerRegion::prepare_fill_surfaces`: sparse islands at or below
/// `minimum_sparse_infill_area` become internal solid.
fn promote_small_sparse(
    solid: &mut [Vec<Polygon>],
    sparse: &mut [Vec<Polygon>],
    settings: &SliceSettings,
) {
    if settings.spiral_mode || settings.infill_density <= 0.0 {
        return;
    }
    let min_area = settings.minimum_sparse_infill_area_mm2;
    if min_area <= 0.0 {
        return;
    }
    for i in 0..sparse.len() {
        let mut small = Vec::new();
        sparse[i].retain(|poly| {
            let area = signed_area_mm2(poly);
            if area > 0.0 && area <= min_area {
                small.push(poly.clone());
                false
            } else {
                true
            }
        });
        if small.is_empty() {
            continue;
        }
        append_union(&mut solid[i], small);
        sparse[i] = difference_polygons(&sparse[i], &solid[i]);
    }
}

fn signed_area_mm2(poly: &[Point]) -> f64 {
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

/// C++ `discover_vertical_shells`: extra internal solid is infill minus the
/// intersection of neighbor `fill_expolygons` in the shell window.
fn vertical_shell_extra(
    i: usize,
    regions: &[Vec<Polygon>],
    zs: &[f64],
    settings: &SliceSettings,
) -> Vec<Polygon> {
    let n = regions.len();
    if i >= n || regions[i].is_empty() {
        return Vec::new();
    }
    let mut holes = regions[i].clone();
    let z_i = zs.get(i).copied().unwrap_or(0.0);
    let top_n = settings.top_shell_layers as usize;
    let bottom_n = settings.bottom_shell_layers as usize;
    if top_n > 0 {
        for k in 1..n - i {
            let j = i + k;
            let dz = zs.get(j).copied().unwrap_or(z_i) - z_i;
            if past_shell_window(k, top_n, settings.top_shell_thickness_mm, dz) {
                break;
            }
            if !within_shell_window(k, top_n, settings.top_shell_thickness_mm, dz) {
                continue;
            }
            if combine_holes(&mut holes, &regions[j]) {
                break;
            }
        }
    }
    if bottom_n > 0 && !holes.is_empty() {
        for k in 1..=i {
            let j = i - k;
            let dz = z_i - zs.get(j).copied().unwrap_or(z_i);
            if past_shell_window(k, bottom_n, settings.bottom_shell_thickness_mm, dz) {
                break;
            }
            if !within_shell_window(k, bottom_n, settings.bottom_shell_thickness_mm, dz) {
                continue;
            }
            if combine_holes(&mut holes, &regions[j]) {
                break;
            }
        }
    }
    regularize_vertical_shell(
        difference_polygons(&regions[i], &holes),
        settings.line_width_for(FlowRole::SolidInfill, i == 0),
    )
}

/// C++ `combine_holes`: empty neighbor clears the intersection (all internal
/// becomes extra solid).
fn combine_holes(holes: &mut Vec<Polygon>, neighbor: &[Polygon]) -> bool {
    if holes.is_empty() || neighbor.is_empty() {
        holes.clear();
        return true;
    }
    *holes = intersect_polygons(holes, neighbor);
    holes.is_empty()
}

/// C++ `discover_vertical_shells` `offset2_ex` / `shrink_ex` (jtSquare).
/// Open regions narrower than 0.65× spacing, close gaps under 1.2× spacing,
/// then expand by 0.2× spacing and drop crumbs smaller than 1.5× spacing mm².
fn regularize_vertical_shell(shell: Vec<Polygon>, line_width_mm: f64) -> Vec<Polygon> {
    if shell.is_empty() || line_width_mm <= 0.0 {
        return shell;
    }
    let spacing = line_width_mm * VERTICAL_SHELL_SPACING_SCALE;
    let narrow_ensure = 0.5 * 0.65 * spacing;
    let narrow_sparse = 0.5 * 1.2 * spacing;
    let tiny_overlap = 0.2 * spacing;
    let opened = offset_polygons_square(&union_polygons(&shell), -narrow_ensure);
    if opened.is_empty() {
        return Vec::new();
    }
    let closed = offset_polygons_square(&opened, narrow_ensure + narrow_sparse);
    let grown = offset_polygons_square(&closed, -(narrow_sparse - tiny_overlap));
    let min_area = spacing * 1.5;
    grown
        .into_iter()
        .filter(|p| signed_area_mm2(p).abs() >= min_area)
        .collect()
}

fn emit_shells(
    layers: &mut [Layer],
    settings: &SliceSettings,
    mesh: Option<&TriangleMesh>,
    shells: &ShellMap,
    append: bool,
    shared_sparse: Option<&[Vec<Polygon>]>,
) {
    let zs: Vec<f64> = layers.iter().map(|l| l.z_mm).collect();
    let sparse_paths = sparse_paths(&shells.sparse, &zs, settings, mesh);
    let lower_src = shared_sparse.unwrap_or(&shells.sparse);

    layers.par_iter_mut().enumerate().for_each(|(i, layer)| {
        let first = i == 0;
        let solid_w = settings.line_width_for(FlowRole::SolidInfill, first);
        let top_w = settings.line_width_for(FlowRole::TopSolidInfill, first);
        let mut rest = difference_polygons(&shells.solid[i], &shells.top[i]);
        rest = difference_polygons(&rest, &shells.bottom[i]);
        let lower_sparse = if i > 0 {
            lower_src[i - 1].as_slice()
        } else {
            &[]
        };
        let (wide, narrow, floating) = classify_internal_solid(&rest, lower_sparse, settings);

        let top_region = shells.top[i].clone();
        let top_surface = infill::solid_surface(
            &shells.top[i],
            top_w,
            i,
            settings.top_surface_pattern,
            settings.infill_direction_deg,
            settings.nozzle_diameter_mm,
        );
        let bottom_paths = infill::solid_surface(
            &shells.bottom[i],
            solid_w,
            i.wrapping_add(1),
            settings.bottom_surface_pattern,
            settings.infill_direction_deg,
            settings.nozzle_diameter_mm,
        );
        let mut solid_infill = infill::solid(&wide, solid_w, i, settings.infill_direction_deg);
        solid_infill.extend(closed_concentric(
            &narrow,
            solid_w,
            settings.nozzle_diameter_mm,
        ));
        let floating_vertical_shell =
            closed_concentric(&floating, solid_w, settings.nozzle_diameter_mm);
        let infill = sparse_paths[i].clone();
        if append {
            append_union(&mut layer.top_region, top_region);
            layer.top_surface.extend(top_surface);
            if i == 0 {
                layer.bottom_surface.extend(bottom_paths);
            } else {
                layer.bridge.extend(bottom_paths);
            }
            layer.solid_infill.extend(solid_infill);
            layer
                .floating_vertical_shell
                .extend(floating_vertical_shell);
            append_union(&mut layer.floating_areas, lower_sparse.to_vec());
            layer.infill.extend(infill);
        } else {
            layer.top_region = top_region;
            layer.top_surface = top_surface;
            if i == 0 {
                layer.bottom_surface = bottom_paths;
            } else {
                layer.bridge = bottom_paths;
            }
            layer.solid_infill = solid_infill;
            layer.floating_vertical_shell = floating_vertical_shell;
            layer.floating_areas = lower_sparse.to_vec();
            layer.infill = infill;
        }
    });
}

/// C++ `NARROW_INFILL_AREA_THRESHOLD` in `Fill.cpp`.
const NARROW_INFILL_AREA_THRESHOLD_MM: f64 = 3.0;

/// C++ `group_fills`: narrow internal solid over lower-layer sparse becomes
/// `stFloatingVerticalShell` (`ipFloatingConcentric`); other narrow islands use
/// `ipConcentricInternal`.
fn classify_internal_solid(
    rest: &[Polygon],
    lower_sparse: &[Polygon],
    settings: &SliceSettings,
) -> (Vec<Polygon>, Vec<Polygon>, Vec<Polygon>) {
    if !settings.detect_narrow_internal_solid_infill {
        return (rest.to_vec(), Vec::new(), Vec::new());
    }
    let mut wide = Vec::new();
    let mut narrow = Vec::new();
    let mut floating = Vec::new();
    for poly in rest {
        if poly.len() < 3 || !is_narrow_infill_area(poly) {
            wide.push(poly.clone());
            continue;
        }
        if overlaps_lower_internal(poly, lower_sparse) {
            floating.push(poly.clone());
        } else {
            narrow.push(poly.clone());
        }
    }
    (wide, narrow, floating)
}

fn is_narrow_infill_area(poly: &Polygon) -> bool {
    offset_polygons(std::slice::from_ref(poly), -NARROW_INFILL_AREA_THRESHOLD_MM).is_empty()
}

fn overlaps_lower_internal(poly: &Polygon, lower: &[Polygon]) -> bool {
    if lower.is_empty() {
        return false;
    }
    let grown = offset_polygons(std::slice::from_ref(poly), 1e-3);
    !intersect_polygons(&grown, lower).is_empty()
}

fn closed_concentric(region: &[Polygon], spacing_mm: f64, nozzle_mm: f64) -> Vec<Polyline> {
    infill::concentric(
        region,
        spacing_mm,
        nozzle_mm.max(0.0) * bambu_config::LOOP_CLIPPING_OVER_NOZZLE,
    )
}

fn sparse_paths(
    sparse: &[Vec<Polygon>],
    zs: &[f64],
    settings: &SliceSettings,
    mesh: Option<&TriangleMesh>,
) -> Vec<Vec<Polyline>> {
    match settings.infill_pattern {
        InfillPattern::Lightning => infill::generate_lightning(sparse, settings)
            .into_iter()
            .map(|paths| infill::apply_sparse_multiline(paths, settings))
            .collect(),
        InfillPattern::AdaptiveCubic | InfillPattern::SupportCubic => {
            let support_only = settings.infill_pattern == InfillPattern::SupportCubic;
            let spacing = infill::adaptive::line_spacing_mm(settings);
            let octree =
                mesh.and_then(|mesh| infill::adaptive::Octree::build(mesh, spacing, support_only));
            (0..sparse.len())
                .into_par_iter()
                .map(|i| {
                    let paths = octree
                        .as_ref()
                        .map(|octree| infill::adaptive::fill(&sparse[i], octree, zs[i]))
                        .unwrap_or_default();
                    infill::apply_sparse_multiline(paths, settings)
                })
                .collect()
        }
        _ => (0..sparse.len())
            .into_par_iter()
            .map(|i| infill::generate(&sparse[i], settings, i, zs[i]))
            .collect(),
    }
}

fn cover(polygons: &[Polygon]) -> Vec<Polygon> {
    if polygons.is_empty() {
        Vec::new()
    } else {
        offset_polygons(polygons, COVER_MM)
    }
}

fn append_union(dst: &mut Vec<Polygon>, extra: Vec<Polygon>) {
    if extra.is_empty() {
        return;
    }
    let mut acc = std::mem::take(dst);
    acc.extend(extra);
    *dst = union_polygons(&acc);
}

#[cfg(test)]
mod tests {
    use super::*;
    use bambu_geom::Point;

    fn rect(width_mm: f64, height_mm: f64) -> Polygon {
        vec![
            Point::from_mm(0.0, 0.0),
            Point::from_mm(width_mm, 0.0),
            Point::from_mm(width_mm, height_mm),
            Point::from_mm(0.0, height_mm),
        ]
    }

    #[test]
    fn regularize_drops_narrow_vertical_shell() {
        let w = 0.42;
        assert!(
            regularize_vertical_shell(vec![rect(0.15, 10.0)], w).is_empty(),
            "opening 0.65× spacing should drop a 0.15 mm ring"
        );
        let kept = regularize_vertical_shell(vec![rect(2.0, 10.0)], w);
        assert!(
            !kept.is_empty(),
            "a 2 mm slope band should survive regularization"
        );
    }
}
