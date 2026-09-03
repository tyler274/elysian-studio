//! `PrintObjectStep::PrepareInfill`: top / bottom / bridge vs sparse regions.
//!
//! Simplified `detect_surfaces_type` + `discover_horizontal_shells`. Neighbor
//! contours are grown slightly so clipper slivers are not treated as shells.

use bambu_config::{InfillPattern, SliceSettings};
use bambu_geom::{
    difference_polygons, intersect_polygons, offset_polygons, union_polygons, Polygon, TriangleMesh,
};
use rayon::prelude::*;

use crate::infill;
use crate::Layer;

const COVER_MM: f64 = 0.15;

pub fn apply(layers: &mut [Layer], settings: &SliceSettings, mesh: Option<&TriangleMesh>) {
    if layers.is_empty() {
        return;
    }

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
    layers.par_iter_mut().enumerate().for_each(|(i, layer)| {
        let mut rest = difference_polygons(&solid[i], &top[i]);
        rest = difference_polygons(&rest, &bottom[i]);

        layer.top_region = top[i].clone();
        layer.top_surface =
            infill::solid_surface(&top[i], spacing, i, settings.top_surface_pattern);
        let bottom_paths = infill::solid_surface(
            &bottom[i],
            spacing,
            i.wrapping_add(1),
            settings.bottom_surface_pattern,
        );
        if i == 0 {
            layer.bottom_surface = bottom_paths;
        } else {
            layer.bridge = bottom_paths;
        }
        layer.solid_infill = infill::solid(&rest, spacing, i);
    });

    assign_sparse_infill(layers, &sparse_all, settings, mesh);
}

fn assign_sparse_infill(
    layers: &mut [Layer],
    sparse: &[Vec<Polygon>],
    settings: &SliceSettings,
    mesh: Option<&TriangleMesh>,
) {
    match settings.infill_pattern {
        InfillPattern::Lightning => {
            for (layer, paths) in layers
                .iter_mut()
                .zip(infill::generate_lightning(sparse, settings))
            {
                layer.infill = paths;
            }
        }
        InfillPattern::AdaptiveCubic | InfillPattern::SupportCubic => {
            let support_only = settings.infill_pattern == InfillPattern::SupportCubic;
            let spacing = infill::adaptive::line_spacing_mm(settings);
            let octree =
                mesh.and_then(|mesh| infill::adaptive::Octree::build(mesh, spacing, support_only));
            layers.par_iter_mut().enumerate().for_each(|(i, layer)| {
                layer.infill = octree
                    .as_ref()
                    .map(|octree| infill::adaptive::fill(&sparse[i], octree, layer.z_mm))
                    .unwrap_or_default();
            });
        }
        _ => {
            layers.par_iter_mut().enumerate().for_each(|(i, layer)| {
                layer.infill = infill::generate(&sparse[i], settings, i, layer.z_mm);
            });
        }
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
