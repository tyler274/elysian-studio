//! Classic offset perimeters (`PerimeterGenerator::process_classic`) and
//! Arachne-lite leftover centerlines (`wall_generator: arachne`).
//!
//! Full C++ Arachne (`SkeletalTrapezoidation` + variable bead width) is a later
//! phase. This pass keeps constant extrusion width: fit as many full-width
//! onions as possible, then drop a centerline into leftover thinner than one
//! wall so features classic drops still print.
//!
//! `top_one_wall_type` / legacy `only_one_wall_top`: the topmost layer (and,
//! for `AllTop`, terraces not covered by the layer above) keep a single outer
//! wall so top infill can fill the rest. Extra inner walls continue only under
//! the layer above (C++ `generate_one_wall_by_top_most` / `Alltop`).

use bambu_config::{SliceSettings, TopOneWallType, WallGenerator};
use bambu_geom::{
    difference_polygons, intersect_polygons, offset_polygons, union_polygons, Polygon, Polyline,
};

use crate::seams;

const COVER_MM: f64 = 0.15;

pub struct PerimeterResult {
    pub outer: Vec<Polyline>,
    pub inner: Vec<Polyline>,
    pub infill_region: Vec<Polygon>,
    pub seam_hint: Option<bambu_geom::Point>,
}

pub fn generate(
    contours: &[Polygon],
    settings: &SliceSettings,
    seam_hint: Option<bambu_geom::Point>,
    upper: Option<&[Polygon]>,
) -> PerimeterResult {
    match settings.wall_generator {
        WallGenerator::Classic => classic_perimeters(contours, settings, seam_hint, upper),
        WallGenerator::Arachne => arachne_perimeters(contours, settings, seam_hint, upper),
    }
}

fn classic_perimeters(
    contours: &[Polygon],
    settings: &SliceSettings,
    seam_hint: Option<bambu_geom::Point>,
    upper: Option<&[Polygon]>,
) -> PerimeterResult {
    let w = settings.line_width_mm;
    let loops = settings.wall_loops.max(1);
    let upper = upper.filter(|u| !u.is_empty());
    let one_wall_layer =
        loops > 1 && settings.top_one_wall != TopOneWallType::None && upper.is_none();

    let mut hint = seam_hint;
    let (outer, mut inner) = if one_wall_layer {
        let (outer, hint_out) = onion_rings(contours, 1, w, settings, hint);
        hint = hint_out;
        (outer, Vec::new())
    } else {
        onion_split(contours, loops, w, settings, hint, &mut hint)
    };

    let wall_n = if one_wall_layer { 1 } else { loops };
    let mut infill_region = offset_polygons(contours, -w * (wall_n as f64 + 0.5));

    if !one_wall_layer && loops > 1 && settings.top_one_wall == TopOneWallType::AllTop {
        if let Some(upper) = upper {
            apply_all_top(
                contours,
                upper,
                loops,
                w,
                settings,
                &mut inner,
                &mut infill_region,
                &mut hint,
                false,
            );
        }
    }

    PerimeterResult {
        outer,
        inner,
        infill_region,
        seam_hint: hint,
    }
}

fn arachne_perimeters(
    contours: &[Polygon],
    settings: &SliceSettings,
    seam_hint: Option<bambu_geom::Point>,
    upper: Option<&[Polygon]>,
) -> PerimeterResult {
    let w = settings.line_width_mm;
    let loops = settings.wall_loops.max(1);
    let upper = upper.filter(|u| !u.is_empty());
    let one_wall_layer =
        loops > 1 && settings.top_one_wall != TopOneWallType::None && upper.is_none();

    let mut hint = seam_hint;
    let target = if one_wall_layer { 1 } else { loops };
    let (outer, mut inner) = arachne_split(contours, target, w, settings, &mut hint);

    let mut infill_region = offset_polygons(contours, -w * (target as f64 + 0.5));

    if !one_wall_layer && loops > 1 && settings.top_one_wall == TopOneWallType::AllTop {
        if let Some(upper) = upper {
            apply_all_top(
                contours,
                upper,
                loops,
                w,
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
        infill_region,
        seam_hint: hint,
    }
}

#[allow(clippy::too_many_arguments)]
fn apply_all_top(
    contours: &[Polygon],
    upper: &[Polygon],
    loops: u32,
    w: f64,
    settings: &SliceSettings,
    inner: &mut Vec<Polyline>,
    infill_region: &mut Vec<Polygon>,
    hint: &mut Option<bambu_geom::Point>,
    arachne: bool,
) {
    let cover = cover_upper(upper);
    let remaining = offset_polygons(contours, -w);
    let not_top = intersect_polygons(&remaining, &cover);
    let after_one = offset_polygons(contours, -w * 1.5);
    let top = difference_polygons(&after_one, &cover);
    if not_top.is_empty() {
        inner.clear();
        *infill_region = after_one;
        return;
    }
    let extra = loops - 1;
    if arachne {
        let (more_outer, more_inner) = arachne_split(&not_top, extra, w, settings, hint);
        *inner = more_outer;
        inner.extend(more_inner);
    } else {
        let (more, hint_out) = onion_rings(&not_top, extra, w, settings, *hint);
        *hint = hint_out;
        *inner = more;
    }
    *infill_region = offset_polygons(&not_top, -w * (extra as f64 + 0.5));
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
    w: f64,
    settings: &SliceSettings,
    mut hint: Option<bambu_geom::Point>,
    hint_out: &mut Option<bambu_geom::Point>,
) -> (Vec<Polyline>, Vec<Polyline>) {
    let mut outer = Vec::new();
    let mut inner = Vec::new();
    for i in 0..loops {
        let rings = offset_loops(contours, w * (i as f64 + 0.5), settings, &mut hint);
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
    w: f64,
    settings: &SliceSettings,
    hint: &mut Option<bambu_geom::Point>,
) -> (Vec<Polyline>, Vec<Polyline>) {
    let mut outer = Vec::new();
    let mut inner = Vec::new();
    let mut fitted = 0u32;
    for i in 0..loops {
        let rings = offset_loops(contours, w * (i as f64 + 0.5), settings, hint);
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
        if let Some(thin) = leftover_centerline(contours, fitted, w, settings, hint) {
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
    w: f64,
    settings: &SliceSettings,
    hint: &mut Option<bambu_geom::Point>,
) -> Option<Vec<Polyline>> {
    let min_feat = settings.min_feature_size_mm();
    let min_bead = settings.min_bead_width_mm();
    let (lo, hi) = if fitted == 0 {
        (min_feat * 0.5, w * 0.5)
    } else {
        let eps = (min_feat * 0.5).max(min_bead * 0.01).max(1e-4);
        (w * (fitted as f64 - 0.5) + eps, w * f64::from(fitted))
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

fn seam_rings(
    mut rings: Vec<Polygon>,
    settings: &SliceSettings,
    hint: &mut Option<bambu_geom::Point>,
) -> Vec<Polyline> {
    rings.retain(|r| r.len() >= 3);
    for ring in &mut rings {
        seams::apply_seam(ring, settings.seam, *hint);
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

    #[test]
    fn arachne_matches_classic_on_thick_square() {
        let contours = vec![rect(20.0, 20.0)];
        let mut classic = SliceSettings::default();
        classic.wall_loops = 2;
        classic.wall_generator = WallGenerator::Classic;
        let mut arachne = classic.clone();
        arachne.wall_generator = WallGenerator::Arachne;
        let a = generate(&contours, &classic, None, None);
        let b = generate(&contours, &arachne, None, None);
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
        let a = generate(&contours, &classic, None, None);
        let b = generate(&contours, &arachne, None, None);
        assert_eq!(a.outer.len(), 1);
        assert!(a.inner.is_empty(), "classic cannot fit a second wall");
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
    fn arachne_skips_features_thinner_than_min_feature() {
        let contours = vec![rect(0.08, 10.0)];
        let mut settings = SliceSettings::default();
        settings.line_width_mm = 0.42;
        settings.wall_loops = 2;
        settings.min_feature_size = 0.25;
        settings.nozzle_diameter_mm = 0.4;
        settings.wall_generator = WallGenerator::Arachne;
        let peri = generate(&contours, &settings, None, None);
        assert!(peri.outer.is_empty());
        assert!(peri.inner.is_empty());
    }
}
