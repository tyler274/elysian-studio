//! Classic downward-projected supports (`PrintObjectStep::SupportMaterial`).
//!
//! Tree supports are not implemented yet. Overhangs are `layer[i] minus an
//! expansion of layer[i-1]` using `tan(threshold)` as the per-layer XY reach,
//! then unioned downward and subtracted from the part plus XY gap.

use bambu_config::SliceSettings;
use bambu_geom::{difference_polygons, offset_polygons, union_polygons, Polygon};

use crate::infill;
use crate::Layer;

pub fn apply_classic(layers: &mut [Layer], settings: &SliceSettings) {
    if !settings.enable_support || layers.len() < 2 {
        return;
    }

    let n = layers.len();
    let mut overhangs: Vec<Vec<Polygon>> = vec![Vec::new(); n];
    for i in 1..n {
        let dz = (layers[i].z_mm - layers[i - 1].z_mm).max(1e-6);
        let expansion = dz * settings.support_threshold_angle_deg.to_radians().tan();
        let supported = offset_polygons(&layers[i - 1].contours, expansion);
        overhangs[i] = difference_polygons(&layers[i].contours, &supported);
    }

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
    for i in 0..n {
        if regions[i].is_empty() {
            continue;
        }
        let fill_region = offset_polygons(&regions[i], -inset);
        if fill_region.is_empty() {
            continue;
        }
        let is_interface = (1..=interface_n).any(|d| {
            let j = i + d as usize;
            j < n && !overhangs[j].is_empty()
        });
        layers[i].support_region = std::mem::take(&mut regions[i]);
        if is_interface {
            layers[i].support_interface =
                infill::rectilinear(&fill_region, settings.line_width_mm * 1.1, i);
        } else {
            layers[i].support = infill::rectilinear(&fill_region, settings.support_spacing_mm(), i);
        }
    }
}

pub fn first_layer_footprint(layer: &Layer) -> Vec<Polygon> {
    let mut acc = layer.contours.clone();
    acc.extend(layer.support_region.iter().cloned());
    union_polygons(&acc)
}
