//! Classic offset perimeters (`PerimeterGenerator::process_classic`) and
//! Arachne-lite leftover centerlines (`wall_generator: arachne`).
//!
//! Full C++ Arachne (`SkeletalTrapezoidation` + variable bead width) is a later
//! phase. This pass keeps constant extrusion width: fit as many full-width
//! onions as possible, then drop a centerline into leftover thinner than one
//! wall so features classic drops still print.
//!
//! Classic leftover between onions uses C++ gap collapse (`opening_ex` minus
//! too-wide `offset2_ex`) and an open midline for thin corridors. Variable-width
//! Voronoi medial axis is later.
//!
//! `top_one_wall_type` / legacy `only_one_wall_top`: the topmost layer (and,
//! for `AllTop`, terraces not covered by the layer above) keep a single outer
//! wall so top infill can fill the rest. Extra inner walls continue only under
//! the layer above (C++ `generate_one_wall_by_top_most` / `Alltop`).
//! `only_one_wall_first_layer` does the same on object layer 0.

use bambu_config::{FlowRole, SliceSettings, TopOneWallType, WallGenerator};
use bambu_geom::{
    difference_polygons, intersect_polygons, offset_polygons, union_polygons, Polygon, Polyline,
};

use crate::seams;

const COVER_MM: f64 = 0.15;

/// Outer vs inner wall extrusion width (C++ `ext_perimeter_flow` / `perimeter_flow`).
#[derive(Clone, Copy)]
struct WallSpacing {
    outer: f64,
    inner: f64,
}

impl WallSpacing {
    fn from_settings(settings: &SliceSettings, first_layer: bool) -> Self {
        Self {
            outer: settings.line_width_for(FlowRole::ExternalPerimeter, first_layer),
            inner: settings.line_width_for(FlowRole::Perimeter, first_layer),
        }
    }

    fn loop_offset(self, i: u32) -> f64 {
        // Width stack matches C++ `precise_outer_wall` ON + InnerOuter.
        // `precise_outer_wall` false uses `Flow::spacing`; that is a no-op
        // while spacing equals width, so BBL default gaps stay here.
        if i == 0 {
            self.outer * 0.5
        } else {
            self.outer + self.inner * (f64::from(i) - 0.5)
        }
    }

    fn stack_inset(self, n: u32) -> f64 {
        if n <= 1 {
            self.outer * 1.5
        } else {
            self.outer + self.inner * (f64::from(n) - 0.5)
        }
    }
}

/// C++ `generate_one_wall_by_top_most` / `only_one_wall_first_layer`.
fn use_single_wall(settings: &SliceSettings, loops: u32, topmost: bool, first_layer: bool) -> bool {
    loops > 1
        && ((settings.top_one_wall != TopOneWallType::None && topmost)
            || (settings.only_one_wall_first_layer && first_layer))
}

pub struct PerimeterResult {
    pub outer: Vec<Polyline>,
    pub inner: Vec<Polyline>,
    pub infill_region: Vec<Polygon>,
    pub gap_infill: Vec<Polyline>,
    pub seam_hint: Option<bambu_geom::Point>,
}

pub fn generate(
    contours: &[Polygon],
    settings: &SliceSettings,
    seam_hint: Option<bambu_geom::Point>,
    upper: Option<&[Polygon]>,
    layer_idx: usize,
    lower: Option<&[Polygon]>,
) -> PerimeterResult {
    let mut result = match settings.wall_generator {
        WallGenerator::Classic => {
            classic_perimeters(contours, settings, seam_hint, upper, layer_idx)
        }
        WallGenerator::Arachne => {
            arachne_perimeters(contours, settings, seam_hint, upper, layer_idx)
        }
    };
    if settings.seam_placement_away_from_overhangs {
        if let Some(supported) = lower.filter(|polys| !polys.is_empty()) {
            if let Some(hint) = seams::retime_away_from_overhangs(
                &mut result.outer,
                &mut result.inner,
                settings.seam,
                supported,
            ) {
                result.seam_hint = Some(hint);
            }
        }
    }
    result
}

fn classic_perimeters(
    contours: &[Polygon],
    settings: &SliceSettings,
    seam_hint: Option<bambu_geom::Point>,
    upper: Option<&[Polygon]>,
    layer_idx: usize,
) -> PerimeterResult {
    let first_layer = layer_idx == 0;
    let walls = WallSpacing::from_settings(settings, first_layer);
    let loops = settings.wall_loops_for_layer(layer_idx);
    let upper = upper.filter(|u| !u.is_empty());
    let one_wall_layer = use_single_wall(settings, loops, upper.is_none(), first_layer);

    let mut hint = seam_hint;
    let (classic_outer, mut inner) = if one_wall_layer {
        let (outer, hint_out) = onion_rings(contours, 1, walls.outer, settings, hint);
        hint = hint_out;
        (outer, Vec::new())
    } else {
        onion_split(contours, loops, walls, settings, hint, &mut hint)
    };

    let wall_n = if one_wall_layer { 1 } else { loops };
    let mut thin_walls = Vec::new();
    let mut thin_core: Option<Vec<Polygon>> = None;
    let mut outer = classic_outer;
    if settings.detect_thin_wall {
        let (core, thin) = peel_thin_walls(contours, walls.outer, settings.nozzle_diameter_mm);
        thin_walls = thin;
        if core.is_empty() {
            outer = Vec::new();
            inner.clear();
        } else {
            outer = seam_rings(core.clone(), settings, &mut hint);
        }
        thin_core = Some(core);
    }

    let mut infill_region = if thin_core.as_ref().is_some_and(|c| c.is_empty()) {
        Vec::new()
    } else {
        offset_polygons(contours, -walls.stack_inset(wall_n))
    };
    let mut gap_infill = Vec::new();

    if !one_wall_layer && loops > 1 && settings.top_one_wall == TopOneWallType::AllTop {
        if let Some(upper) = upper {
            apply_all_top(
                contours,
                upper,
                loops,
                walls,
                settings,
                &mut inner,
                &mut infill_region,
                &mut hint,
                false,
            );
        }
    }

    if settings.gap_infill_speed_mm_s > 0.0 {
        let (gap_src, gap_loops): (&[Polygon], u32) = match thin_core.as_deref() {
            Some([]) => (&[], 0),
            Some(core) => (core, wall_n.saturating_sub(1)),
            None => (contours, wall_n),
        };
        let areas = if gap_loops == 0 {
            Vec::new()
        } else {
            collect_gap_areas(gap_src, gap_loops, walls.inner)
        };
        gap_infill = centerline_gaps(&areas, walls.inner, settings, &mut hint);
        if !areas.is_empty() && !infill_region.is_empty() {
            let covered = offset_polygons(&areas, walls.inner * 0.5);
            infill_region = difference_polygons(&infill_region, &covered);
        }
    }

    if !thin_walls.is_empty() {
        thin_walls.append(&mut outer);
        outer = thin_walls;
    }

    PerimeterResult {
        outer,
        inner,
        infill_region: apply_infill_wall_overlap(infill_region, settings, first_layer),
        gap_infill,
        seam_hint: hint,
    }
}

fn arachne_perimeters(
    contours: &[Polygon],
    settings: &SliceSettings,
    seam_hint: Option<bambu_geom::Point>,
    upper: Option<&[Polygon]>,
    layer_idx: usize,
) -> PerimeterResult {
    let first_layer = layer_idx == 0;
    let walls = WallSpacing::from_settings(settings, first_layer);
    let loops = settings.wall_loops_for_layer(layer_idx);
    let upper = upper.filter(|u| !u.is_empty());
    let one_wall_layer = use_single_wall(settings, loops, upper.is_none(), first_layer);
    let mut hint = seam_hint;
    let target = if one_wall_layer { 1 } else { loops };
    let (outer, mut inner) = arachne_split(contours, target, walls, settings, &mut hint);

    let mut infill_region = offset_polygons(contours, -walls.stack_inset(target));

    if !one_wall_layer && loops > 1 && settings.top_one_wall == TopOneWallType::AllTop {
        if let Some(upper) = upper {
            apply_all_top(
                contours,
                upper,
                loops,
                walls,
                settings,
                &mut inner,
                &mut infill_region,
                &mut hint,
                true,
            );
        }
    }

    PerimeterResult {
        outer,
        inner,
        infill_region: apply_infill_wall_overlap(infill_region, settings, first_layer),
        gap_infill: Vec::new(),
        seam_hint: hint,
    }
}

/// C++ `infill_wall_overlap`: enlarge the fill contour toward the last wall.
/// Percent is applied to sparse infill line width (BBL 15% ≈ 0.063 mm at 0.42 mm).
fn apply_infill_wall_overlap(
    infill: Vec<Polygon>,
    settings: &SliceSettings,
    first_layer: bool,
) -> Vec<Polygon> {
    let grow =
        settings.infill_wall_overlap * settings.line_width_for(FlowRole::SparseInfill, first_layer);
    if grow <= 1e-9 || infill.is_empty() {
        infill
    } else {
        offset_polygons(&infill, grow)
    }
}

#[allow(clippy::too_many_arguments)]
fn apply_all_top(
    contours: &[Polygon],
    upper: &[Polygon],
    loops: u32,
    walls: WallSpacing,
    settings: &SliceSettings,
    inner: &mut Vec<Polyline>,
    infill_region: &mut Vec<Polygon>,
    hint: &mut Option<bambu_geom::Point>,
    arachne: bool,
) {
    let cover = cover_upper(upper);
    let remaining = offset_polygons(contours, -walls.outer);
    let not_top = intersect_polygons(&remaining, &cover);
    let after_one = offset_polygons(contours, -walls.stack_inset(1));
    let top = difference_polygons(&after_one, &cover);
    if not_top.is_empty() {
        inner.clear();
        *infill_region = after_one;
        return;
    }
    let extra = loops - 1;
    let extra_walls = WallSpacing {
        outer: walls.inner,
        inner: walls.inner,
    };
    if arachne {
        let (more_outer, more_inner) = arachne_split(&not_top, extra, extra_walls, settings, hint);
        *inner = more_outer;
        inner.extend(more_inner);
    } else {
        let (more, hint_out) = onion_rings(&not_top, extra, walls.inner, settings, *hint);
        *hint = hint_out;
        *inner = more;
    }
    *infill_region = offset_polygons(&not_top, -extra_walls.stack_inset(extra));
    if !top.is_empty() {
        infill_region.extend(top);
        *infill_region = union_polygons(infill_region);
    }
}

fn cover_upper(upper: &[Polygon]) -> Vec<Polygon> {
    let grown = offset_polygons(upper, COVER_MM);
    if grown.is_empty() {
        union_polygons(upper)
    } else {
        union_polygons(&grown)
    }
}

fn onion_split(
    contours: &[Polygon],
    loops: u32,
    walls: WallSpacing,
    settings: &SliceSettings,
    mut hint: Option<bambu_geom::Point>,
    hint_out: &mut Option<bambu_geom::Point>,
) -> (Vec<Polyline>, Vec<Polyline>) {
    let mut outer = Vec::new();
    let mut inner = Vec::new();
    for i in 0..loops {
        let rings = offset_loops(contours, walls.loop_offset(i), settings, &mut hint);
        if i == 0 {
            outer.extend(rings);
        } else {
            inner.extend(rings);
        }
    }
    *hint_out = hint;
    (outer, inner)
}

fn onion_rings(
    contours: &[Polygon],
    loops: u32,
    w: f64,
    settings: &SliceSettings,
    mut hint: Option<bambu_geom::Point>,
) -> (Vec<Polyline>, Option<bambu_geom::Point>) {
    let mut out = Vec::new();
    for i in 0..loops {
        out.extend(offset_loops(
            contours,
            w * (i as f64 + 0.5),
            settings,
            &mut hint,
        ));
    }
    (out, hint)
}

/// Fit full-width onions, then a leftover centerline if `loops` were not filled.
fn arachne_split(
    contours: &[Polygon],
    loops: u32,
    walls: WallSpacing,
    settings: &SliceSettings,
    hint: &mut Option<bambu_geom::Point>,
) -> (Vec<Polyline>, Vec<Polyline>) {
    let mut outer = Vec::new();
    let mut inner = Vec::new();
    let mut fitted = 0u32;
    for i in 0..loops {
        let rings = offset_loops(contours, walls.loop_offset(i), settings, hint);
        if rings.is_empty() {
            break;
        }
        if i == 0 {
            outer.extend(rings);
        } else {
            inner.extend(rings);
        }
        fitted += 1;
    }
    if fitted < loops {
        if let Some(thin) = leftover_centerline(contours, fitted, walls, settings, hint) {
            if fitted == 0 {
                outer.extend(thin);
            } else {
                inner.extend(thin);
            }
        }
    }
    (outer, inner)
}

fn leftover_centerline(
    contours: &[Polygon],
    fitted: u32,
    walls: WallSpacing,
    settings: &SliceSettings,
    hint: &mut Option<bambu_geom::Point>,
) -> Option<Vec<Polyline>> {
    let min_feat = settings.min_feature_size_mm();
    let min_bead = settings.min_bead_width_mm();
    let (lo, hi) = if fitted == 0 {
        (min_feat * 0.5, walls.outer * 0.5)
    } else {
        let last = walls.loop_offset(fitted - 1);
        let eps = (min_feat * 0.5).max(min_bead * 0.01).max(1e-4);
        (last + eps, last + 0.5 * walls.inner)
    };
    if hi <= lo + 1e-6 {
        return None;
    }
    let rings = deepest_inset(contours, lo, hi)?;
    Some(seam_rings(rings, settings, hint))
}

fn deepest_inset(contours: &[Polygon], lo: f64, hi: f64) -> Option<Vec<Polygon>> {
    let mut best = offset_keep(contours, lo)?;
    let mut a = lo;
    let mut b = hi;
    for _ in 0..16 {
        let mid = 0.5 * (a + b);
        match offset_keep(contours, mid) {
            Some(o) => {
                best = o;
                a = mid;
            }
            None => b = mid,
        }
    }
    Some(best)
}

fn offset_keep(contours: &[Polygon], inset_mm: f64) -> Option<Vec<Polygon>> {
    let mut rings = offset_polygons(contours, -inset_mm);
    rings.retain(|r| r.len() >= 3);
    if rings.is_empty() {
        None
    } else {
        Some(rings)
    }
}

/// C++ `INSET_OVERLAP_TOLERANCE` in `libslic3r.h`.
const INSET_OVERLAP_TOLERANCE: f64 = 0.4;

/// C++ `detect_thin_wall` first loop: `offset2_ex` drops islands that cannot
/// hold two line widths, then a medial-axis stand-in of the leftover.
fn peel_thin_walls(
    contours: &[Polygon],
    outer_w: f64,
    nozzle_mm: f64,
) -> (Vec<Polygon>, Vec<Polyline>) {
    let core = offset_polygons(&offset_polygons(contours, -outer_w), outer_w * 0.5);
    let grown = offset_polygons(&core, outer_w * 0.5);
    let leftover = difference_polygons(contours, &grown);
    let min_w = (nozzle_mm / 3.0).max(1e-4);
    // C++ `opening_ex` then `medial_axis` — do not subtract too-wide (gap-fill).
    let opened = offset_polygons(&offset_polygons(&leftover, -min_w * 0.5), min_w * 0.5);
    let mut paths = Vec::new();
    for poly in opened {
        if let Some(path) = crate::gap_fill::open_centerline(&poly) {
            paths.push(path);
        }
    }
    (core, paths)
}

/// Collapsed leftover between successive onions (`PerimeterGenerator` `gaps`).
fn collect_gap_areas(contours: &[Polygon], loops: u32, w: f64) -> Vec<Polygon> {
    let min_spacing = w * (1.0 - INSET_OVERLAP_TOLERANCE);
    let mut last = contours.to_vec();
    let mut gaps = Vec::new();
    for _ in 0..loops.max(1) {
        let shrunk = offset_polygons(&last, -(w + min_spacing * 0.5));
        let offsets = offset_polygons(&shrunk, min_spacing * 0.5);
        let inner_half = offset_polygons(&last, -0.5 * w);
        let grown_next = offset_polygons(&offsets, 0.5 * w);
        gaps.extend(difference_polygons(&inner_half, &grown_next));
        last = offsets;
        if last.is_empty() {
            break;
        }
    }
    gaps
}

fn centerline_gaps(
    gaps: &[Polygon],
    w: f64,
    settings: &SliceSettings,
    hint: &mut Option<bambu_geom::Point>,
) -> Vec<Polyline> {
    let min = 0.2 * w * (1.0 - INSET_OVERLAP_TOLERANCE);
    let max = 2.0 * w;
    let keep = crate::gap_fill::collapse_gap_areas(gaps, min, max);
    if keep.is_empty() {
        return Vec::new();
    }
    let mut corridors = Vec::new();
    let mut loops = Vec::new();
    for poly in keep {
        if poly.len() < 3 {
            continue;
        }
        if crate::gap_fill::is_thin_corridor(&poly, max) {
            if let Some(path) = crate::gap_fill::open_centerline(&poly) {
                corridors.push(path);
                continue;
            }
        }
        loops.push(poly);
    }
    let mut paths = corridors;
    if !loops.is_empty() {
        if let Some(rings) = deepest_inset(&loops, 0.0, (max * 0.5).min(w)) {
            paths.extend(seam_rings(rings, settings, hint));
        }
    }
    let min_len = settings.filter_out_gap_fill_mm;
    if min_len > 0.0 {
        paths.retain(|p| polyline_len_mm(p) >= min_len);
    }
    paths
}

fn polyline_len_mm(path: &[bambu_geom::Point]) -> f64 {
    path.windows(2).map(|w| w[0].distance_mm(w[1])).sum()
}

fn seam_rings(
    mut rings: Vec<Polygon>,
    settings: &SliceSettings,
    hint: &mut Option<bambu_geom::Point>,
) -> Vec<Polyline> {
    rings.retain(|r| r.len() >= 3);
    for ring in &mut rings {
        seams::apply_seam(ring, settings.seam, *hint, None);
        *hint = ring.first().copied();
    }
    rings
}

fn offset_loops(
    contours: &[Polygon],
    inset_mm: f64,
    settings: &SliceSettings,
    hint: &mut Option<bambu_geom::Point>,
) -> Vec<Polyline> {
    let rings = offset_polygons(contours, -inset_mm);
    seam_rings(rings, settings, hint)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bambu_config::{SliceSettings, WallGenerator};
    use bambu_geom::Point;

    fn rect(width_mm: f64, height_mm: f64) -> Polygon {
        vec![
            Point::from_mm(0.0, 0.0),
            Point::from_mm(width_mm, 0.0),
            Point::from_mm(width_mm, height_mm),
            Point::from_mm(0.0, height_mm),
        ]
    }

    fn wall_len(paths: &[Polyline]) -> f64 {
        paths
            .iter()
            .flat_map(|pl| pl.windows(2))
            .map(|w| w[0].distance_mm(w[1]))
            .sum()
    }

    fn region_area(polys: &[Polygon]) -> f64 {
        polys.iter().map(crate::contour_area_mm2).sum()
    }

    #[test]
    fn infill_wall_overlap_grows_fill_region() {
        let contours = vec![rect(20.0, 20.0)];
        let mut off = SliceSettings::default();
        off.wall_loops = 2;
        off.gap_infill_speed_mm_s = 0.0;
        off.infill_wall_overlap = 0.0;
        let mut on = off.clone();
        on.infill_wall_overlap = 0.15;
        let a = generate(&contours, &off, None, None, 1, None);
        let b = generate(&contours, &on, None, None, 1, None);
        let area_off = region_area(&a.infill_region);
        let area_on = region_area(&b.infill_region);
        assert!(
            area_on > area_off + 2.0,
            "15% overlap should grow infill toward walls: off={area_off} on={area_on}"
        );
        let mut arachne = on.clone();
        arachne.wall_generator = WallGenerator::Arachne;
        let c = generate(&contours, &arachne, None, None, 1, None);
        assert!(
            (region_area(&c.infill_region) - area_on).abs() < 1.0,
            "arachne should apply the same overlap grow"
        );
    }

    #[test]
    fn infill_wall_overlap_does_not_refill_gaps() {
        let contours = vec![rect(0.7, 20.0)];
        let mut settings = SliceSettings::default();
        settings.line_width_mm = 0.42;
        settings.wall_loops = 2;
        settings.gap_infill_speed_mm_s = 45.0;
        settings.infill_wall_overlap = 0.15;
        let peri = generate(&contours, &settings, None, None, 1, None);
        assert!(!peri.gap_infill.is_empty());
        assert!(
            peri.infill_region.is_empty(),
            "overlap must not grow empty leftover into infill"
        );
    }

    #[test]
    fn arachne_matches_classic_on_thick_square() {
        let contours = vec![rect(20.0, 20.0)];
        let mut classic = SliceSettings::default();
        classic.wall_loops = 2;
        classic.wall_generator = WallGenerator::Classic;
        let mut arachne = classic.clone();
        arachne.wall_generator = WallGenerator::Arachne;
        let a = generate(&contours, &classic, None, None, 1, None);
        let b = generate(&contours, &arachne, None, None, 1, None);
        assert_eq!(a.outer.len(), 1);
        assert_eq!(b.outer.len(), 1);
        assert!(!a.inner.is_empty());
        assert_eq!(a.inner.len(), b.inner.len());
        assert_eq!(a.outer, b.outer);
        assert_eq!(a.inner, b.inner);
    }

    #[test]
    fn arachne_keeps_centerline_in_thin_leftover() {
        let w = 0.42;
        let contours = vec![rect(0.7, 20.0)];
        let mut classic = SliceSettings::default();
        classic.line_width_mm = w;
        classic.wall_loops = 2;
        classic.wall_generator = WallGenerator::Classic;
        let mut arachne = classic.clone();
        arachne.wall_generator = WallGenerator::Arachne;
        let a = generate(&contours, &classic, None, None, 1, None);
        let b = generate(&contours, &arachne, None, None, 1, None);
        assert_eq!(a.outer.len(), 1);
        assert!(a.inner.is_empty(), "classic cannot fit a second wall");
        assert!(
            !a.gap_infill.is_empty(),
            "classic leftover should be gap fill"
        );
        assert!(b.gap_infill.is_empty(), "arachne leftover is a wall");
        assert_eq!(b.outer.len(), 1);
        assert!(
            !b.inner.is_empty(),
            "arachne should place a leftover centerline"
        );
        let classic_len = wall_len(&a.outer) + wall_len(&a.inner);
        let arachne_len = wall_len(&b.outer) + wall_len(&b.inner);
        assert!(
            arachne_len > classic_len * 1.4,
            "thin leftover should add wall length: arachne={arachne_len} classic={classic_len}"
        );
    }

    #[test]
    fn classic_gap_fill_is_open_centerline() {
        let w = 0.42;
        let contours = vec![rect(0.7, 20.0)];
        let mut settings = SliceSettings::default();
        settings.line_width_mm = w;
        settings.wall_loops = 2;
        settings.wall_generator = WallGenerator::Classic;
        settings.gap_infill_speed_mm_s = 45.0;
        let peri = generate(&contours, &settings, None, None, 1, None);
        assert!(!peri.gap_infill.is_empty());
        let path = &peri.gap_infill[0];
        let len = wall_len(&peri.gap_infill);
        assert!(
            (12.0..35.0).contains(&len),
            "open midline along the leftover, not a double-back loop: len={len}"
        );
        assert!(
            path.first().unwrap().distance_mm(*path.last().unwrap()) > 8.0,
            "gap fill should be an open path"
        );
    }

    #[test]
    fn filter_out_gap_fill_drops_short_paths() {
        let contours = vec![rect(0.7, 20.0)];
        let mut settings = SliceSettings::default();
        settings.line_width_mm = 0.42;
        settings.wall_loops = 2;
        settings.gap_infill_speed_mm_s = 45.0;
        settings.filter_out_gap_fill_mm = 100.0;
        let peri = generate(&contours, &settings, None, None, 1, None);
        assert!(
            peri.gap_infill.is_empty(),
            "100 mm filter should drop the leftover"
        );
    }

    #[test]
    fn arachne_skips_features_thinner_than_min_feature() {
        let contours = vec![rect(0.08, 10.0)];
        let mut settings = SliceSettings::default();
        settings.line_width_mm = 0.42;
        settings.wall_loops = 2;
        settings.min_feature_size = 0.25;
        settings.nozzle_diameter_mm = 0.4;
        settings.wall_generator = WallGenerator::Arachne;
        let peri = generate(&contours, &settings, None, None, 1, None);
        assert!(peri.outer.is_empty());
        assert!(peri.inner.is_empty());
    }

    #[test]
    fn inner_wall_follows_inner_wall_line_width() {
        let contours = vec![rect(20.0, 20.0)];
        let mut settings = SliceSettings::default();
        settings.wall_loops = 2;
        settings.gap_infill_speed_mm_s = 0.0;
        settings.outer_wall_line_width_mm = 0.42;
        settings.inner_wall_line_width_mm = 0.42;
        let same = generate(&contours, &settings, None, None, 1, None);
        settings.inner_wall_line_width_mm = 0.60;
        let wide = generate(&contours, &settings, None, None, 1, None);
        let min_x = |paths: &[Polyline]| {
            paths
                .iter()
                .flatten()
                .map(|p| p.to_mm().0)
                .fold(f64::INFINITY, f64::min)
        };
        let same_inner = min_x(&same.inner);
        let wide_inner = min_x(&wide.inner);
        assert!(
            wide_inner > same_inner + 0.05,
            "wider inner wall should sit further inward: same={same_inner} wide={wide_inner}"
        );
        let same_outer = min_x(&same.outer);
        let wide_outer = min_x(&wide.outer);
        assert!(
            (same_outer - wide_outer).abs() < 0.02,
            "outer wall should keep outer_wall_line_width: same={same_outer} wide={wide_outer}"
        );
    }

    #[test]
    fn alternate_extra_wall_adds_an_inner_on_odd_layers() {
        let contours = vec![rect(20.0, 20.0)];
        let mut settings = SliceSettings::default();
        settings.wall_loops = 2;
        settings.gap_infill_speed_mm_s = 0.0;
        let even = generate(&contours, &settings, None, None, 0, None);
        settings.alternate_extra_wall = true;
        let odd = generate(&contours, &settings, None, None, 1, None);
        let even_on = generate(&contours, &settings, None, None, 2, None);
        assert_eq!(even.inner.len(), 1);
        assert_eq!(odd.inner.len(), 2);
        assert_eq!(even_on.inner.len(), 1);
        settings.spiral_mode = true;
        let spiral_odd = generate(&contours, &settings, None, None, 1, None);
        assert_eq!(spiral_odd.inner.len(), 1);
    }
}
