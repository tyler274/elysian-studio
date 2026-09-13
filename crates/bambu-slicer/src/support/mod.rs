//! Supports (`PrintObjectStep::SupportMaterial`).
//!
//! Overhangs are `layer[i] minus an expansion of layer[i-1]` using
//! `tan(threshold)` as the per-layer XY reach. Classic fills the downward
//! union with the base pattern (`support_base_pattern`; BBL `default` is
//! rectilinear). Tree (`tree(auto)`) drops slim branch disks to the
//! bed, steering around the part — Bambu's default when supports are on.
//! Short two-sided bridges can be dropped (`max_bridge_length` / `bridge_no_support`).
//! Dust-sized overhangs can be dropped (`support_remove_small_overhang`).
//! Contacts can grow or shrink in XY (`support_expansion`).
//! Tree auto can keep only cantilevers (`support_critical_regions_only`).
//! Top contact can use loops (`support_interface_loop_pattern`) or a chosen
//! hatch (`support_interface_pattern`; BBL `auto` stays 0° rectilinear).
//! Classic in-model floors honor `support_interface_bottom_layers` (`-1` copies
//! top; C++ default 0). Tree hardcodes 0. Bottom hatch uses
//! `support_bottom_interface_spacing`.
//! Top/bottom Z gaps (`support_top_z_distance` / `support_bottom_z_distance`)
//! skip whole object layers under overhangs and above in-model landings.
//! Tree trunks honor `support_base_pattern` (honeycomb / grid fill the disks;
//! BBL `default` stays hollow) and `tree_support_wall_count` (`-1` auto outlines).
//! `support_angle` rotates hatch. Solid interfaces can be ironed
//! (`enable_support_ironing` with `support_interface_spacing` 0).

mod tree;

use bambu_config::{
    FlowRole, IroningPattern, SliceSettings, SupportBasePattern, SupportInterfacePattern,
    SupportType, LOOP_CLIPPING_OVER_NOZZLE,
};
use bambu_geom::{
    difference_polygons, intersect_polygons, offset_polygons, union_polygons, Polygon,
};
use rayon::prelude::*;

use crate::infill;
use crate::GpuAssist;
use crate::Layer;

pub fn apply(layers: &mut [Layer], settings: &SliceSettings, assist: Option<&GpuAssist>) {
    if !settings.enable_support || layers.len() < 2 {
        return;
    }
    let mut overhangs = detect_overhangs(layers, settings);
    if settings.support_on_build_plate_only {
        trim_overhangs_above_model(&mut overhangs, layers);
    }
    trim_small_overhangs(&mut overhangs, layers, settings);
    trim_non_critical_overhangs(&mut overhangs, layers, settings);
    apply_enforcer_blocker(&mut overhangs, layers);
    trim_bridged_overhangs(&mut overhangs, layers, settings);
    expand_overhangs(&mut overhangs, layers, settings);
    coalesce_independent_support(&mut overhangs, settings);
    match settings.support_type {
        SupportType::Classic => apply_classic(layers, settings, &overhangs),
        SupportType::Tree => tree::apply(layers, settings, &overhangs, assist),
    }
    iron_support_interface(layers, settings);
}

/// C++ `independent_support_layer_height`: merge every other object layer when
/// `2 * layer_height` still fits `max_layer_height` and the wipe tower is off.
fn coalesce_independent_support(overhangs: &mut [Vec<Polygon>], settings: &SliceSettings) {
    if !settings.independent_support_layer_height || settings.has_wipe_tower() {
        return;
    }
    if 2.0 * settings.layer_height_mm > settings.max_layer_height_mm + 1e-9 {
        return;
    }
    let n = overhangs.len();
    for i in (1..n).step_by(2) {
        let extra = if i + 1 < n {
            overhangs[i + 1].clone()
        } else {
            Vec::new()
        };
        if !extra.is_empty() {
            overhangs[i].extend(extra);
            overhangs[i] = union_polygons(&overhangs[i]);
        }
        if i + 1 < n {
            overhangs[i + 1].clear();
        }
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

/// C++ TreeSupport `support_critical_regions_only` on auto types: drop
/// overhangs that are not cantilevers. Enforcers run after so painted support
/// can restore a region. Classic columns ignore the flag.
fn trim_non_critical_overhangs(
    overhangs: &mut [Vec<Polygon>],
    layers: &[Layer],
    settings: &SliceSettings,
) {
    if !settings.support_critical_regions_only || settings.support_type != SupportType::Tree {
        return;
    }
    for i in 1..overhangs.len() {
        if overhangs[i].is_empty() {
            continue;
        }
        let lower = layers[i - 1].contours.as_slice();
        overhangs[i].retain(|poly| is_cantilever_overhang(poly, lower));
    }
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

/// C++ `support_expansion` on classic contacts: grow against the lower slice
/// so the extra cannot jump through the model; shrink is a plain offset.
fn expand_overhangs(overhangs: &mut [Vec<Polygon>], layers: &[Layer], settings: &SliceSettings) {
    let delta = settings.support_expansion_mm;
    if delta.abs() < 1e-9 {
        return;
    }
    for i in 1..overhangs.len() {
        if overhangs[i].is_empty() {
            continue;
        }
        overhangs[i] = expand_overhang_delta(&overhangs[i], &layers[i - 1].contours, delta);
    }
}

fn expand_overhang_delta(overhang: &[Polygon], lower: &[Polygon], delta_mm: f64) -> Vec<Polygon> {
    if delta_mm > 0.0 {
        difference_polygons(&offset_polygons(overhang, delta_mm), lower)
    } else {
        offset_polygons(overhang, delta_mm)
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
pub(super) fn fill_support_base(
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
            settings.support_angle_deg,
        ),
        SupportBasePattern::RectilinearGrid => {
            let angle = if layer_idx.is_multiple_of(2) {
                settings.support_angle_deg
            } else {
                settings.support_angle_deg + 90.0
            };
            infill::rectilinear(region, spacing, layer_idx, angle)
        }
        _ => infill::rectilinear(region, spacing, layer_idx, settings.support_angle_deg),
    }
}

/// C++ `LoopInterfaceProcessor` with `n_contact_loops = 1`: loops on the top
/// contact instead of hatch. Studio notches circles into a contour; concentric
/// rings cover the same contact island. Hatch follows
/// `support_interface_pattern` (`auto` / `rectilinear` stay 0° scanlines).
pub(super) fn fill_support_interface(
    region: &[Polygon],
    spacing: f64,
    layer_idx: usize,
    settings: &SliceSettings,
    loops: bool,
) -> Vec<bambu_geom::Polyline> {
    if loops || settings.support_interface_pattern == SupportInterfacePattern::Concentric {
        infill::concentric(
            region,
            spacing,
            settings.nozzle_diameter_mm * bambu_config::LOOP_CLIPPING_OVER_NOZZLE,
        )
    } else if settings.support_interface_pattern == SupportInterfacePattern::Grid {
        let mut lines = infill::rectilinear(region, spacing, layer_idx, settings.support_angle_deg);
        lines.extend(infill::rectilinear(
            region,
            spacing,
            layer_idx,
            settings.support_angle_deg + 90.0,
        ));
        lines
    } else if settings.support_interface_pattern == SupportInterfacePattern::RectilinearInterlaced {
        // C++ `support_interface_angle`: 90° ± 45° when `support_angle` is 0.
        let angle = if layer_idx.is_multiple_of(2) {
            settings.support_angle_deg + 135.0
        } else {
            settings.support_angle_deg + 45.0
        };
        infill::rectilinear(region, spacing, layer_idx, angle)
    } else {
        infill::rectilinear(region, spacing, layer_idx, settings.support_angle_deg)
    }
}

pub(super) fn overhang_in_range(
    overhangs: &[Vec<Polygon>],
    layer: usize,
    first: usize,
    last: usize,
) -> bool {
    (first..=last).any(|delta| {
        overhangs
            .get(layer.saturating_add(delta))
            .is_some_and(|overhang| !overhang.is_empty())
    })
}

fn mark_on_model_landings(
    regions: &[Vec<Polygon>],
    layers: &[Layer],
    settings: &SliceSettings,
) -> Vec<bool> {
    let mut landing = vec![false; regions.len()];
    for (i, region) in regions.iter().enumerate().skip(1) {
        if region.is_empty() || !regions[i - 1].is_empty() {
            continue;
        }
        let grown = offset_polygons(region, settings.support_xy_gap_mm(i) + 0.05);
        landing[i] = !intersect_polygons(&grown, &layers[i - 1].contours).is_empty();
    }
    landing
}

fn apply_classic(layers: &mut [Layer], settings: &SliceSettings, overhangs: &[Vec<Polygon>]) {
    let n = layers.len();
    let mut column: Vec<Polygon> = Vec::new();
    let mut regions: Vec<Vec<Polygon>> = vec![Vec::new(); n];
    for i in (0..n).rev() {
        let mut acc = column;
        acc.extend(overhangs[i].iter().cloned());
        column = union_polygons(&acc);
        let forbidden = offset_polygons(&layers[i].contours, settings.support_xy_gap_mm(i));
        regions[i] = difference_polygons(&column, &forbidden);
    }

    let top_gap = settings.support_top_gap_layers();
    let bottom_gap = settings.support_bottom_gap_layers();
    if top_gap > 0 {
        for (i, region) in regions.iter_mut().enumerate() {
            if overhang_in_range(overhangs, i, 1, top_gap) {
                region.clear();
            }
        }
    }

    let mut landing = mark_on_model_landings(&regions, layers, settings);
    if bottom_gap > 0 {
        let mut skip = vec![false; n];
        for (i, is_landing) in landing.iter().enumerate() {
            if !is_landing {
                continue;
            }
            for delta in 0..bottom_gap {
                if let Some(slot) = skip.get_mut(i + delta) {
                    *slot = true;
                }
            }
        }
        for (region, skip_layer) in regions.iter_mut().zip(skip) {
            if skip_layer {
                region.clear();
            }
        }
        landing = mark_on_model_landings(&regions, layers, settings);
    }

    let interface_n = settings.support_interface_layers.max(1);
    let bottom_n = settings.resolved_support_interface_bottom_layers();
    let support_w = settings.line_width_for(bambu_config::FlowRole::SupportMaterial, false);
    let inset = support_w * 0.5;
    let support_spacing = settings.support_spacing_mm();
    let interface_spacing = settings.support_interface_hatch_spacing_mm();
    let bottom_spacing = settings.support_bottom_interface_hatch_spacing_mm();
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
            let j = i + top_gap + d as usize;
            j < n && !overhangs[j].is_empty()
        });
        let is_bottom =
            bottom_n > 0 && (0..bottom_n).any(|d| i >= d as usize && landing[i - d as usize]);
        if is_interface {
            let is_contact = i + top_gap + 1 < n && !overhangs[i + top_gap + 1].is_empty();
            layer.support_interface = fill_support_interface(
                &fill_region,
                interface_spacing,
                i,
                settings,
                is_contact && settings.support_interface_loop_pattern,
            );
        } else if is_bottom {
            layer.support_interface =
                fill_support_interface(&fill_region, bottom_spacing, i, settings, false);
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

/// C++ support-interface ironing: solid hatch (`support_interface_spacing` 0)
/// that is not grid. Paths are `erSupportIroning`, not part ironing.
fn iron_support_interface(layers: &mut [Layer], settings: &SliceSettings) {
    if !settings.enable_support_ironing {
        return;
    }
    if settings.support_interface_spacing_mm.abs() > 1e-9 {
        return;
    }
    if settings.support_interface_pattern == SupportInterfacePattern::Grid {
        return;
    }
    let inset = settings.support_ironing_inset_mm.max(0.0);
    let spacing = settings.support_ironing_spacing_mm.max(0.02);
    let clip = settings.nozzle_diameter_mm * LOOP_CLIPPING_OVER_NOZZLE;
    // C++ `support_ironing_direction` (degrees), not `support_angle`.
    let angle = settings.support_ironing_direction_deg;
    layers.par_iter_mut().enumerate().for_each(|(i, layer)| {
        if layer.support_interface.is_empty() {
            return;
        }
        let area = if inset > 1e-9 {
            let inner = offset_polygons(&layer.support_region, -inset);
            if inner.is_empty() {
                layer.support_region.clone()
            } else {
                inner
            }
        } else {
            layer.support_region.clone()
        };
        if area.is_empty() {
            return;
        }
        let paths = match settings.support_ironing_pattern {
            IroningPattern::Concentric => infill::concentric(&area, spacing, clip),
            IroningPattern::Rectilinear => infill::solid_monotonic(&area, spacing, i, angle),
        };
        layer.support_ironing.extend(paths);
    });
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
        let lip = rect(0.0, 0.0, 24.0, 2.5);
        let shallow = vec![rect(2.5, 2.5, 21.5, 21.5)];
        assert!(
            !is_cantilever_overhang(&lip, &shallow),
            "a 2.5 mm lip stays inside C++ dist_max 3 mm"
        );
    }

    #[test]
    fn interface_loops_span_both_axes() {
        let region = vec![rect(0.0, 0.0, 20.0, 20.0)];
        let mut settings = SliceSettings::default();
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
        assert!(
            !path_spans_both_axes(&hatch),
            "BBL auto interface should stay scanlines"
        );
        settings.support_interface_pattern = SupportInterfacePattern::Grid;
        let grid = fill_support_interface(&region, 0.5, 0, &settings, false);
        assert!(
            grid.len() > hatch.len(),
            "C++ grid interface should add the cross hatch: grid={} auto={}",
            grid.len(),
            hatch.len()
        );
        settings.support_interface_pattern = SupportInterfacePattern::Concentric;
        let rings = fill_support_interface(&region, 0.5, 0, &settings, false);
        assert!(
            rings.iter().any(|path| path.len() >= 4),
            "C++ concentric interface should emit a ring"
        );
        settings.support_interface_pattern = SupportInterfacePattern::RectilinearInterlaced;
        let even = fill_support_interface(&region, 0.5, 0, &settings, false);
        let odd = fill_support_interface(&region, 0.5, 1, &settings, false);
        assert_ne!(
            even, odd,
            "C++ rectilinear interlaced should rotate 90° between layers"
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

    #[test]
    fn expansion_grows_away_from_lower_slice() {
        let wing = vec![rect(0.0, 0.0, 24.0, 8.0)];
        let lower = vec![rect(8.0, 8.0, 16.0, 16.0)];
        let size = |polys: &[Polygon]| {
            let (min, max) = infill::bbox(polys).expect("overhang");
            let (x0, y0) = min.to_mm();
            let (x1, y1) = max.to_mm();
            (x1 - x0, y1 - y0)
        };
        let (w0, h0) = size(&wing);
        let (wg, hg) = size(&expand_overhang_delta(&wing, &lower, 2.0));
        assert!(
            wg > w0 + 1.0 && hg > h0 + 1.0,
            "positive expansion should enlarge the contact: {w0}x{h0} -> {wg}x{hg}"
        );
        let (ws, hs) = size(&expand_overhang_delta(&wing, &lower, -1.0));
        assert!(
            ws < w0 - 0.5 && hs < h0 - 0.5,
            "negative expansion should shrink the contact: {w0}x{h0} -> {ws}x{hs}"
        );
    }
}
