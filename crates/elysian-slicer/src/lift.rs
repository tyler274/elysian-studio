//! C++ `PrintObject::detect_overhangs_for_lift`.

use bambu_geom::{difference_polygons, offset_polygons, Polygon};
use rayon::prelude::*;

use crate::Layer;

/// C++ `g_min_overhang_percent_for_lift`.
const MIN_OVERHANG_PERCENT: f64 = 0.3;
/// C++ `offset2_ex` delta as a fraction of line width.
const OPENING_FRAC: f64 = 0.1;

fn offset2(polygons: &[Polygon], line_width_mm: f64) -> Vec<Polygon> {
    let delta = OPENING_FRAC * line_width_mm;
    offset_polygons(&offset_polygons(polygons, -delta), delta)
}

/// Fill `Layer::lift_overhangs` from each slice vs the layer below.
///
/// C++ starts at `raft_layers + 1` in the object-layer list (skip raft and the
/// first object slice). Raft is prepended here, so skip the leading wall-less
/// slabs plus the first object layer.
pub(crate) fn detect_overhangs_for_lift(layers: &mut [Layer], line_width_mm: f64) {
    if layers.len() < 2 || line_width_mm <= 0.0 {
        return;
    }
    let first_object = layers
        .iter()
        .position(|l| !l.outer_walls.is_empty() || !l.inner_walls.is_empty())
        .unwrap_or(0);
    let start = first_object + 1;
    if start >= layers.len() {
        return;
    }
    let min_overlap = line_width_mm * MIN_OVERHANG_PERCENT;
    let contours: Vec<Vec<Polygon>> = layers.iter().map(|l| l.contours.clone()).collect();
    let support: Vec<Vec<Polygon>> = layers.iter().map(|l| l.support_region.clone()).collect();
    layers.par_iter_mut().enumerate().for_each(|(i, layer)| {
        if i < start {
            return;
        }
        let grown = offset_polygons(&contours[i - 1], min_overlap);
        let overhangs = difference_polygons(&contours[i], &grown);
        let mut lift = offset2(&overhangs, line_width_mm);
        if !support[i].is_empty() {
            lift.extend(offset2(&support[i], line_width_mm));
        }
        layer.lift_overhangs = lift;
    });
}
