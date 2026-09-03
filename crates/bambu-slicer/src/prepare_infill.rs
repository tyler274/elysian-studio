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
        fill_into(layers, settings, mesh, false);
        return;
    }
    for layer in layers.iter_mut() {
        layer.infill.clear();
        layer.solid_infill.clear();
        layer.top_surface.clear();
        layer.bottom_surface.clear();
        layer.bridge.clear();
        layer.top_region.clear();
    }
    let union_infill: Vec<Vec<Polygon>> = layers.iter().map(|l| l.infill_region.clone()).collect();
    for r in 0..nreg {
        let regions: Vec<Vec<Polygon>> = layers
            .iter()
            .map(|layer| layer.region_infill.get(r).cloned().unwrap_or_default())
            .collect();
        let cfg = layers
            .iter()
            .find_map(|layer| layer.region_settings.get(r).cloned())
            .unwrap_or_else(|| settings.clone());
        for (layer, region) in layers.iter_mut().zip(&regions) {
            layer.infill_region = region.clone();
        }
        fill_into(layers, &cfg, mesh, true);
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
) {
    let n = layers.len();
    let top_n = settings.top_shell_layers as usize;
    let bottom_n = settings.bottom_shell_layers as usize;
    let regions: Vec<Vec<Polygon>> = layers.iter().map(|l| l.infill_region.clone()).collect();

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

    let spacing = settings.line_width_mm;
    let sparse_all: Vec<_> = (0..n)
        .into_par_iter()
        .map(|i| difference_polygons(&regions[i], &solid[i]))
        .collect();
    let zs: Vec<f64> = layers.iter().map(|l| l.z_mm).collect();
    let sparse_paths = sparse_paths(&sparse_all, &zs, settings, mesh);

    layers.par_iter_mut().enumerate().for_each(|(i, layer)| {
        let mut rest = difference_polygons(&solid[i], &top[i]);
        rest = difference_polygons(&rest, &bottom[i]);

        let top_region = top[i].clone();
        let top_surface = infill::solid_surface(&top[i], spacing, i, settings.top_surface_pattern);
        let bottom_paths = infill::solid_surface(
            &bottom[i],
            spacing,
            i.wrapping_add(1),
            settings.bottom_surface_pattern,
        );
        let solid_infill = infill::solid(&rest, spacing, i);
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
            layer.infill = infill;
        }
    });
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
