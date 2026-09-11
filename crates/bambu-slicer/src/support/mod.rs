//! Supports (`PrintObjectStep::SupportMaterial`).
//!
//! Overhangs are `layer[i] minus an expansion of layer[i-1]` using
//! `tan(threshold)` as the per-layer XY reach. Classic fills the downward
//! union with the base pattern (`support_base_pattern`; BBL `default` is
//! rectilinear). Tree (`tree(auto)`) drops slim branch disks to the
//! bed, steering around the part — Bambu's default when supports are on.
//! Short two-sided bridges can be dropped (`max_bridge_length` / `bridge_no_support`).
//! Dust-sized overhangs can be dropped (`support_remove_small_overhang`).
//! Top contact can use loops (`support_interface_loop_pattern`).

mod tree;

use bambu_config::{FlowRole, SliceSettings, SupportBasePattern, SupportType};
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
    if settings.support_on_build_plate_only {
        trim_overhangs_above_model(&mut overhangs, layers);
    }
    trim_small_overhangs(&mut overhangs, layers, settings);
    apply_enforcer_blocker(&mut overhangs, layers);
    trim_bridged_overhangs(&mut overhangs, layers, settings);
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

/// C++ `PrintObjectSupportMaterial::buildplate_covered`: union of slices
/// below the current layer, then `diff` overhangs so support cannot rest on
/// the model.
fn trim_overhangs_above_model(overhangs: &mut [Vec<Polygon>], layers: &[Layer]) {
    let mut covered: Vec<Polygon> = Vec::new();
    for i in 0..layers.len() {
        if i > 0 && !covered.is_empty() {
            overhangs[i] = difference_polygons(&overhangs[i], &covered);
        }
        let grown = offset_polygons(&layers[i].contours, 0.01);
        let mut acc = covered;
        acc.extend(grown);
        covered = union_polygons(&acc);
    }
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

/// C++ `dist_max > scale_(3)` in `detect_overhangs` cantilever tagging.
const CANTILEVER_MM: f64 = 3.0;

/// C++ classic `SupportMaterial` small-overhang filter: morphological close by
/// `line_width`, then drop if a bbox axis is under `2 * line_width`. Keep
/// cantilevers. Runs before enforcers so painted support can restore a nub.
fn trim_small_overhangs(
    overhangs: &mut [Vec<Polygon>],
    layers: &[Layer],
    settings: &SliceSettings,
) {
    if !settings.support_remove_small_overhang {
        return;
    }
    let fw = settings.line_width_mm;
    if fw <= 0.0 {
        return;
    }
    for i in 1..overhangs.len() {
        if overhangs[i].is_empty() {
            continue;
        }
        let lower = layers[i - 1].contours.as_slice();
        overhangs[i]
            .retain(|poly| is_cantilever_overhang(poly, lower) || !is_small_overhang(poly, fw));
    }
}

fn is_cantilever_overhang(poly: &Polygon, lower: &[Polygon]) -> bool {
    let far = offset_polygons(lower, CANTILEVER_MM);
    !difference_polygons(std::slice::from_ref(poly), &far).is_empty()
}

fn is_small_overhang(poly: &Polygon, fw: f64) -> bool {
    let closed = offset_polygons(&offset_polygons(std::slice::from_ref(poly), fw), -fw);
    if closed.is_empty() {
        return true;
    }
    let Some((min, max)) = infill::bbox(&closed) else {
        return true;
    };
    let (x0, y0) = min.to_mm();
    let (x1, y1) = max.to_mm();
    (x1 - x0) < 2.0 * fw || (y1 - y0) < 2.0 * fw
}

/// C++ `PrintObject::remove_bridges_from_contacts` with `break_bridge = false`
/// (tree) plus classic `bridge_no_support`.
///
/// TreeSupport treats `max_bridge_length > 0` as "consider bridges"; BBL `"0"`
/// keeps every overhang. Classic Studio only drops bridges when
/// `bridge_no_support` is on (any span). Two-sided overhangs whose bbox is
/// shorter than the limit in both axes are subtracted from the contact set.
fn trim_bridged_overhangs(
    overhangs: &mut [Vec<Polygon>],
    layers: &[Layer],
    settings: &SliceSettings,
) {
    let max_len = if settings.bridge_no_support {
        f64::INFINITY
    } else if settings.max_bridge_length_mm > 0.0 {
        settings.max_bridge_length_mm
    } else {
        return;
    };
    let fw = settings.line_width_for(FlowRole::ExternalPerimeter, false);
    for i in 1..overhangs.len() {
        if overhangs[i].is_empty() {
            continue;
        }
        let grown = offset_polygons(&layers[i - 1].contours, fw * 0.5);
        overhangs[i].retain(|poly| !is_short_bridge(poly, &grown, max_len));
    }
}

fn is_short_bridge(poly: &Polygon, grown_lower: &[Polygon], max_len_mm: f64) -> bool {
    let contact = union_polygons(&intersect_polygons(std::slice::from_ref(poly), grown_lower));
    if contact.len() < 2 {
        return false;
    }
    if !max_len_mm.is_finite() {
        return true;
    }
    let Some((min, max)) = infill::bbox(std::slice::from_ref(poly)) else {
        return false;
    };
    let (x0, y0) = min.to_mm();
    let (x1, y1) = max.to_mm();
    (x1 - x0) < max_len_mm && (y1 - y0) < max_len_mm
}

/// C++ `SupportParameters::base_fill_pattern` on classic columns.
fn fill_support_base(
    region: &[Polygon],
    spacing: f64,
    layer_idx: usize,
    settings: &SliceSettings,
) -> Vec<bambu_geom::Polyline> {
    if !spacing.is_finite() || spacing <= 1e-6 {
        return Vec::new();
    }
    match settings.support_base_pattern.classic_fill() {
        SupportBasePattern::None => Vec::new(),
        SupportBasePattern::Honeycomb => infill::honeycomb::fill(
            region,
            spacing,
            settings.support_density.max(1e-6),
            layer_idx,
            0.0,
        ),
        SupportBasePattern::RectilinearGrid => {
            let angle = if layer_idx.is_multiple_of(2) {
                0.0
            } else {
                90.0
            };
            infill::rectilinear(region, spacing, layer_idx, angle)
        }
        _ => infill::rectilinear(region, spacing, layer_idx, 0.0),
    }
}

/// C++ `LoopInterfaceProcessor` with `n_contact_loops = 1`: loops on the top
/// contact instead of hatch. Studio notches circles into a contour; concentric
/// rings cover the same contact island.
pub(super) fn fill_support_interface(
    region: &[Polygon],
    spacing: f64,
    layer_idx: usize,
    settings: &SliceSettings,
    loops: bool,
) -> Vec<bambu_geom::Polyline> {
    if loops {
        infill::concentric(
            region,
            spacing,
            settings.nozzle_diameter_mm * bambu_config::LOOP_CLIPPING_OVER_NOZZLE,
        )
    } else {
        infill::rectilinear(region, spacing, layer_idx, 0.0)
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
    let support_w = settings.line_width_for(bambu_config::FlowRole::SupportMaterial, false);
    let inset = support_w * 0.5;
    let support_spacing = settings.support_spacing_mm();
    let interface_spacing = support_w * 1.1;
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
            let is_contact = i + 1 < n && !overhangs[i + 1].is_empty();
            layer.support_interface = fill_support_interface(
                &fill_region,
                interface_spacing,
                i,
                settings,
                is_contact && settings.support_interface_loop_pattern,
            );
        } else {
            layer.support = fill_support_base(&fill_region, support_spacing, i, settings);
        }
    });
}

/// C++ `_make_skirt` hull of object + support contours up to `skirt_height`.
pub fn layers_footprint(layers: &[Layer]) -> Vec<Polygon> {
    let mut acc = Vec::new();
    for layer in layers {
        acc.extend(layer.contours.iter().cloned());
        acc.extend(layer.support_region.iter().cloned());
    }
    union_polygons(&acc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bambu_geom::Point;

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Polygon {
        vec![
            Point::from_mm(x0, y0),
            Point::from_mm(x1, y0),
            Point::from_mm(x1, y1),
            Point::from_mm(x0, y1),
        ]
    }

    #[test]
    fn two_sided_gap_is_a_short_bridge() {
        let lower = vec![rect(0.0, 0.0, 8.0, 8.0), rect(14.0, 0.0, 22.0, 8.0)];
        let grown = offset_polygons(&lower, 0.21);
        let gap = rect(8.0, 0.0, 14.0, 8.0);
        assert!(
            is_short_bridge(&gap, &grown, 10.0),
            "6×8 mm span under C++ max_bridge_length 10"
        );
        assert!(
            !is_short_bridge(&gap, &grown, 5.0),
            "bbox 6×8 is not shorter than 5 mm in both axes"
        );
        assert!(is_short_bridge(&gap, &grown, f64::INFINITY));
    }

    #[test]
    fn cantilever_wing_is_not_a_bridge() {
        let lower = vec![rect(8.0, 8.0, 16.0, 16.0)];
        let grown = offset_polygons(&lower, 0.21);
        let wing = rect(0.0, 0.0, 24.0, 8.0);
        assert!(
            !is_short_bridge(&wing, &grown, 10.0),
            "one-sided table wing must still get support"
        );
        assert!(!is_short_bridge(&wing, &grown, f64::INFINITY));
    }

    #[test]
    fn thin_island_is_a_small_overhang() {
        let sliver = rect(0.0, 0.0, 0.5, 8.0);
        assert!(is_small_overhang(&sliver, 0.42));
        let island = rect(0.0, 0.0, 8.0, 8.0);
        assert!(!is_small_overhang(&island, 0.42));
    }

    #[test]
    fn cantilever_is_not_removed_as_small() {
        let lower = vec![rect(8.0, 8.0, 16.0, 16.0)];
        let wing = rect(0.0, 0.0, 24.0, 8.0);
        assert!(is_cantilever_overhang(&wing, &lower));
        let nub = rect(8.0, 7.5, 16.0, 8.0);
        assert!(!is_cantilever_overhang(&nub, &lower));
    }

    #[test]
    fn interface_loops_span_both_axes() {
        let region = vec![rect(0.0, 0.0, 20.0, 20.0)];
        let settings = SliceSettings::default();
        let loops = fill_support_interface(&region, 0.5, 0, &settings, true);
        assert!(
            loops.iter().any(|path| path.len() >= 4),
            "concentric contact should emit a ring, got {} paths",
            loops.len()
        );
        let hatch = fill_support_interface(&region, 0.5, 0, &settings, false);
        assert!(
            hatch.iter().any(|path| path.len() == 2),
            "default hatch should be scanline segments"
        );
    }

    fn path_spans_both_axes(paths: &[bambu_geom::Polyline]) -> bool {
        paths.iter().any(|path| {
            if path.len() < 3 {
                return false;
            }
            let mut min_x = f64::MAX;
            let mut max_x = f64::MIN;
            let mut min_y = f64::MAX;
            let mut max_y = f64::MIN;
            for p in path {
                let (x, y) = p.to_mm();
                min_x = min_x.min(x);
                max_x = max_x.max(x);
                min_y = min_y.min(y);
                max_y = max_y.max(y);
            }
            max_x - min_x > 1.0 && max_y - min_y > 1.0
        })
    }

    #[test]
    fn honeycomb_base_spans_both_axes() {
        let region = vec![rect(0.0, 0.0, 20.0, 20.0)];
        let mut settings = SliceSettings::default();
        let hatch = fill_support_base(&region, settings.support_spacing_mm(), 3, &settings);
        assert!(
            !path_spans_both_axes(&hatch),
            "BBL default rectilinear should stay scanlines"
        );
        settings.support_base_pattern = SupportBasePattern::Honeycomb;
        let hex = fill_support_base(&region, settings.support_spacing_mm(), 3, &settings);
        assert!(
            path_spans_both_axes(&hex),
            "C++ honeycomb support should zigzag in X and Y"
        );
        settings.support_base_pattern = SupportBasePattern::None;
        assert!(fill_support_base(&region, settings.support_spacing_mm(), 3, &settings).is_empty());
    }
}
