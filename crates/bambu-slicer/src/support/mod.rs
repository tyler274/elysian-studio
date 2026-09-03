//! Supports (`PrintObjectStep::SupportMaterial`).
//!
//! Overhangs are `layer[i] minus an expansion of layer[i-1]` using
//! `tan(threshold)` as the per-layer XY reach. Classic fills the downward
//! union with grid infill. Tree (`tree(auto)`) drops slim branch disks to the
//! bed, steering around the part — Bambu's default when supports are on.

mod tree;

use bambu_config::{SliceSettings, SupportType};
use bambu_geom::{
    difference_polygons, intersect_polygons, offset_polygons, union_polygons, Polygon,
};
use rayon::prelude::*;

use crate::infill;
use crate::Layer;

pub fn apply(layers: &mut [Layer], settings: &SliceSettings) {
    if !settings.enable_support || layers.len() < 2 {
        return;
    }
    let mut overhangs = detect_overhangs(layers, settings);
    apply_enforcer_blocker(&mut overhangs, layers);
    match settings.support_type {
        SupportType::Classic => apply_classic(layers, settings, &overhangs),
        SupportType::Tree => tree::apply(layers, settings, &overhangs),
    }
}

fn detect_overhangs(layers: &[Layer], settings: &SliceSettings) -> Vec<Vec<Polygon>> {
    let n = layers.len();
    let tan_th = settings.support_threshold_angle_deg.to_radians().tan();
    let mut overhangs: Vec<Vec<Polygon>> = vec![Vec::new(); n];
    overhangs.par_iter_mut().enumerate().for_each(|(i, slot)| {
        if i == 0 {
            return;
        }
        let dz = (layers[i].print_z_mm - layers[i - 1].print_z_mm).max(1e-6);
        let expansion = dz * tan_th;
        let supported = offset_polygons(&layers[i - 1].contours, expansion);
        *slot = difference_polygons(&layers[i].contours, &supported);
    });
    overhangs
}

/// C++ `SupportAnnotations`: enforcers are 90° contacts (`intersection(lslices, enforcer)
/// minus lower layer`); blockers trim overhangs.
fn apply_enforcer_blocker(overhangs: &mut [Vec<Polygon>], layers: &[Layer]) {
    for i in 0..layers.len() {
        if i > 0 && !layers[i].support_enforcer.is_empty() {
            let forced = intersect_polygons(&layers[i].contours, &layers[i].support_enforcer);
            let below = offset_polygons(&layers[i - 1].contours, 0.05);
            let forced = difference_polygons(&forced, &below);
            if !forced.is_empty() {
                let mut acc = overhangs[i].clone();
                acc.extend(forced);
                overhangs[i] = union_polygons(&acc);
            }
        }
        if !layers[i].support_blocker.is_empty() {
            overhangs[i] = difference_polygons(&overhangs[i], &layers[i].support_blocker);
        }
    }
}

fn apply_classic(layers: &mut [Layer], settings: &SliceSettings, overhangs: &[Vec<Polygon>]) {
    let n = layers.len();
    let xy = settings.support_xy_distance_mm;
    let mut column: Vec<Polygon> = Vec::new();
    let mut regions: Vec<Vec<Polygon>> = vec![Vec::new(); n];
    for i in (0..n).rev() {
        let mut acc = column;
        acc.extend(overhangs[i].iter().cloned());
        column = union_polygons(&acc);
        let forbidden = offset_polygons(&layers[i].contours, xy);
        regions[i] = difference_polygons(&column, &forbidden);
    }

    let interface_n = settings.support_interface_layers.max(1);
    let inset = settings.line_width_mm * 0.5;
    let support_spacing = settings.support_spacing_mm();
    let interface_spacing = settings.line_width_mm * 1.1;
    layers.par_iter_mut().enumerate().for_each(|(i, layer)| {
        if regions[i].is_empty() {
            return;
        }
        let fill_region = offset_polygons(&regions[i], -inset);
        layer.support_region = regions[i].clone();
        if fill_region.is_empty() {
            return;
        }
        let is_interface = (1..=interface_n).any(|d| {
            let j = i + d as usize;
            j < n && !overhangs[j].is_empty()
        });
        if is_interface {
            layer.support_interface = infill::rectilinear(&fill_region, interface_spacing, i);
        } else {
            layer.support = infill::rectilinear(&fill_region, support_spacing, i);
        }
    });
}

pub fn first_layer_footprint(layer: &Layer) -> Vec<Polygon> {
    let mut acc = layer.contours.clone();
    acc.extend(layer.support_region.iter().cloned());
    union_polygons(&acc)
}
