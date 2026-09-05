//! `PrintObjectStep::PrepareInfill`: top / bottom / bridge vs sparse regions.
//!
//! Simplified `detect_surfaces_type` + `discover_horizontal_shells`. Neighbor
//! contours are grown slightly so clipper slivers are not treated as shells.
//! Parameter modifiers fill each `LayerRegion` with its own settings (C++).

use bambu_config::{InfillPattern, SliceSettings};
use bambu_geom::{
    difference_polygons, intersect_polygons, offset_polygons, union_polygons, Polygon, Polyline,
    TriangleMesh,
};
use rayon::prelude::*;

use crate::infill;
use crate::Layer;

const COVER_MM: f64 = 0.15;

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
        layer.top_surface.clear();
        layer.bottom_surface.clear();
        layer.bridge.clear();
        layer.top_region.clear();
    }
    let n = layers.len();
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
        shells.push(detect_shells(&regions, &cfg));
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
    let shells = detect_shells(&regions, settings);
    emit_shells(layers, settings, mesh, &shells, append, shared_sparse);
}

struct ShellMap {
    top: Vec<Vec<Polygon>>,
    bottom: Vec<Vec<Polygon>>,
    solid: Vec<Vec<Polygon>>,
    sparse: Vec<Vec<Polygon>>,
}

fn detect_shells(regions: &[Vec<Polygon>], settings: &SliceSettings) -> ShellMap {
    let n = regions.len();
    let top_n = settings.top_shell_layers as usize;
    let bottom_n = settings.bottom_shell_layers as usize;

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
        if top_n > 1 {
            for k in 1..top_n {
                if j >= k {
                    let extra = intersect_polygons(&regions[j - k], &top[j]);
                    append_union(&mut solid[j - k], extra);
                }
            }
        }
        if bottom_n > 1 {
            for k in 1..bottom_n {
                if j + k < n {
                    let extra = intersect_polygons(&regions[j + k], &bottom[j]);
                    append_union(&mut solid[j + k], extra);
                }
            }
        }
    }

    let sparse = (0..n)
        .into_par_iter()
        .map(|i| difference_polygons(&regions[i], &solid[i]))
        .collect();
    ShellMap {
        top,
        bottom,
        solid,
        sparse,
    }
}

fn emit_shells(
    layers: &mut [Layer],
    settings: &SliceSettings,
    mesh: Option<&TriangleMesh>,
    shells: &ShellMap,
    append: bool,
    shared_sparse: Option<&[Vec<Polygon>]>,
) {
    let spacing = settings.line_width_mm;
    let zs: Vec<f64> = layers.iter().map(|l| l.z_mm).collect();
    let sparse_paths = sparse_paths(&shells.sparse, &zs, settings, mesh);
    let lower_src = shared_sparse.unwrap_or(&shells.sparse);

    layers.par_iter_mut().enumerate().for_each(|(i, layer)| {
        let mut rest = difference_polygons(&shells.solid[i], &shells.top[i]);
        rest = difference_polygons(&rest, &shells.bottom[i]);
        let lower_sparse = if i > 0 {
            lower_src[i - 1].as_slice()
        } else {
            &[]
        };
        let (wide, narrow, floating) = classify_internal_solid(&rest, lower_sparse, settings);

        let top_region = shells.top[i].clone();
        let top_surface =
            infill::solid_surface(&shells.top[i], spacing, i, settings.top_surface_pattern);
        let bottom_paths = infill::solid_surface(
            &shells.bottom[i],
            spacing,
            i.wrapping_add(1),
            settings.bottom_surface_pattern,
        );
        let mut solid_infill = infill::solid(&wide, spacing, i);
        solid_infill.extend(closed_concentric(&narrow, spacing));
        let floating_vertical_shell = closed_concentric(&floating, spacing);
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

fn closed_concentric(region: &[Polygon], spacing_mm: f64) -> Vec<Polyline> {
    infill::concentric(region, spacing_mm)
        .into_iter()
        .map(|mut ring| {
            if let (Some(&first), Some(&last)) = (ring.first(), ring.last()) {
                if first != last {
                    ring.push(first);
                }
            }
            ring
        })
        .collect()
}

fn sparse_paths(
    sparse: &[Vec<Polygon>],
    zs: &[f64],
    settings: &SliceSettings,
    mesh: Option<&TriangleMesh>,
) -> Vec<Vec<Polyline>> {
    match settings.infill_pattern {
        InfillPattern::Lightning => infill::generate_lightning(sparse, settings),
        InfillPattern::AdaptiveCubic | InfillPattern::SupportCubic => {
            let support_only = settings.infill_pattern == InfillPattern::SupportCubic;
            let spacing = infill::adaptive::line_spacing_mm(settings);
            let octree =
                mesh.and_then(|mesh| infill::adaptive::Octree::build(mesh, spacing, support_only));
            (0..sparse.len())
                .into_par_iter()
                .map(|i| {
                    octree
                        .as_ref()
                        .map(|octree| infill::adaptive::fill(&sparse[i], octree, zs[i]))
                        .unwrap_or_default()
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
