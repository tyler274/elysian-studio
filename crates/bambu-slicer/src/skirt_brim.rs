//! Skirt and outer brim (`PrintStep::SkirtBrim`).

use bambu_config::SliceSettings;
use bambu_geom::{difference_polygons, offset_polygons, Polygon, Polyline};

use crate::clip::subtract_polylines;

/// Closed concentric loops, innermost at `start_mm`, `loops` rings at `spacing_mm`.
/// Returned outermost-first (print order).
pub fn concentric_loops(
    contours: &[Polygon],
    start_mm: f64,
    loops: u32,
    spacing_mm: f64,
) -> Vec<Polyline> {
    if contours.is_empty() || loops == 0 || spacing_mm <= 0.0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    for i in 0..loops {
        let offset = start_mm + spacing_mm * (i as f64 + 0.5);
        let rings = offset_polygons(contours, offset);
        out.extend(rings.into_iter().filter(|r| r.len() >= 3));
    }
    out.reverse();
    out
}

pub fn brim(contours: &[Polygon], settings: &SliceSettings) -> Vec<Polyline> {
    if !settings.has_outer_brim() {
        return Vec::new();
    }
    let w = settings.line_width_for(bambu_config::FlowRole::ExternalPerimeter, true);
    let loops = (settings.brim_width_mm / w).round().max(1.0) as u32;
    concentric_loops(contours, settings.brim_object_gap_mm.max(0.0), loops, w)
}

pub fn skirt(footprint: &[Polygon], settings: &SliceSettings) -> Vec<Polyline> {
    if !settings.has_skirt() {
        return Vec::new();
    }
    let brim_outer = if settings.draft_shield != bambu_config::DraftShield::Disabled {
        0.0
    } else if settings.has_outer_brim() {
        settings.brim_object_gap_mm.max(0.0) + settings.brim_width_mm
    } else {
        0.0
    };
    let start = brim_outer + settings.skirt_distance_mm;
    concentric_loops(
        footprint,
        start,
        settings.skirt_loops,
        settings.line_width_for(bambu_config::FlowRole::ExternalPerimeter, true),
    )
}

fn skirt_spacing_mm(settings: &SliceSettings) -> f64 {
    settings.line_width_for(bambu_config::FlowRole::ExternalPerimeter, true)
}

/// Skirt extrusion band: outermost loop grown by half spacing minus innermost
/// shrunk by half spacing (C++ `Brim.cpp` `skirt_outers` / `skirt_inners`).
fn skirt_annulus(skirt: &[Polyline], spacing_mm: f64) -> Vec<Polygon> {
    if skirt.is_empty() || spacing_mm <= 0.0 {
        return Vec::new();
    }
    let half = spacing_mm / 2.0;
    let inners = offset_polygons(std::slice::from_ref(skirt.last().unwrap()), -half);
    let outers = offset_polygons(std::slice::from_ref(&skirt[0]), half);
    difference_polygons(&outers, &inners)
}

/// C++ `make_brim`: when draft shield is on and `skirt_distance < brim_width`,
/// difference brim loops against the skirt annulus so the shield can print.
pub fn trim_brim_for_draft_shield(
    brim: Vec<Polyline>,
    skirt: &[Polyline],
    settings: &SliceSettings,
) -> Vec<Polyline> {
    if brim.is_empty()
        || skirt.is_empty()
        || settings.draft_shield == bambu_config::DraftShield::Disabled
        || settings.skirt_distance_mm >= settings.brim_width_mm
    {
        return brim;
    }
    let annulus = skirt_annulus(skirt, skirt_spacing_mm(settings));
    if annulus.is_empty() {
        return brim;
    }
    subtract_polylines(&brim, &annulus)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bambu_config::SliceSettings;
    use bambu_geom::Point;

    fn square(size_mm: f64) -> Polygon {
        vec![
            Point::from_mm(0.0, 0.0),
            Point::from_mm(size_mm, 0.0),
            Point::from_mm(size_mm, size_mm),
            Point::from_mm(0.0, size_mm),
        ]
    }

    fn min_x(path: &Polyline) -> f64 {
        path.iter()
            .map(|p| p.to_mm().0)
            .fold(f64::INFINITY, f64::min)
    }

    #[test]
    fn brim_object_gap_offsets_innermost_loop() {
        let contour = square(20.0);
        let mut settings = SliceSettings::default();
        settings.line_width_mm = 0.42;
        settings.brim_width_mm = 1.26;
        let flush = brim(&[contour.clone()], &settings);
        assert_eq!(flush.len(), 3);
        settings.brim_object_gap_mm = 0.5;
        let gapped = brim(&[contour.clone()], &settings);
        assert_eq!(gapped.len(), 3);
        let flush_inner = min_x(flush.last().unwrap());
        let gapped_inner = min_x(gapped.last().unwrap());
        assert!(
            (gapped_inner - (flush_inner - 0.5)).abs() < 0.05,
            "innermost brim should move out by the gap: flush={flush_inner} gapped={gapped_inner}"
        );
        settings.brim_type = bambu_config::BrimType::NoBrim;
        assert!(brim(&[contour], &settings).is_empty());
    }

    #[test]
    fn skirt_clears_gapped_brim() {
        let contour = square(20.0);
        let mut settings = SliceSettings::default();
        settings.line_width_mm = 0.42;
        settings.brim_width_mm = 1.26;
        settings.brim_object_gap_mm = 0.5;
        settings.skirt_loops = 1;
        settings.skirt_distance_mm = 2.0;
        let brim_paths = brim(&[contour.clone()], &settings);
        let skirt_paths = skirt(&[contour], &settings);
        let brim_outer = brim_paths.iter().map(min_x).fold(f64::INFINITY, f64::min);
        let skirt_inner = min_x(skirt_paths.last().unwrap());
        assert!(
            skirt_inner < brim_outer - 1.5,
            "skirt should sit outside the gapped brim: brim_outer={brim_outer} skirt_inner={skirt_inner}"
        );
    }

    #[test]
    fn skirt_height_zero_emits_no_loops() {
        let contour = square(20.0);
        let mut settings = SliceSettings::default();
        settings.skirt_loops = 2;
        settings.skirt_height = 0;
        assert!(skirt(&[contour], &settings).is_empty());
    }

    #[test]
    fn draft_shield_limited_height_zero_still_emits() {
        let contour = square(20.0);
        let mut settings = SliceSettings::default();
        settings.skirt_loops = 2;
        settings.skirt_height = 0;
        settings.draft_shield = bambu_config::DraftShield::Limited;
        assert_eq!(skirt(&[contour], &settings).len(), 2);
    }

    #[test]
    fn draft_shield_ignores_brim_when_placing_skirt() {
        let contour = square(20.0);
        let mut settings = SliceSettings::default();
        settings.line_width_mm = 0.42;
        settings.brim_width_mm = 5.0;
        settings.brim_object_gap_mm = 0.5;
        settings.skirt_loops = 1;
        settings.skirt_distance_mm = 2.0;
        let with_brim = skirt(&[contour.clone()], &settings);
        settings.draft_shield = bambu_config::DraftShield::Enabled;
        let shielded = skirt(&[contour], &settings);
        let brim_clearance = min_x(with_brim.last().unwrap());
        let shield_inner = min_x(shielded.last().unwrap());
        assert!(
            shield_inner > brim_clearance + 3.0,
            "draft shield sits skirt_distance from the object, not outside brim: shield={shield_inner} brim_clearance={brim_clearance}"
        );
    }

    fn path_length_mm(paths: &[Polyline]) -> f64 {
        paths
            .iter()
            .map(|path| {
                path.windows(2)
                    .map(|w| {
                        let (ax, ay) = w[0].to_mm();
                        let (bx, by) = w[1].to_mm();
                        ((bx - ax).hypot(by - ay)).abs()
                    })
                    .sum::<f64>()
            })
            .sum()
    }

    fn shield_brim_settings() -> (Polygon, SliceSettings) {
        let mut settings = SliceSettings::default();
        settings.line_width_mm = 0.5;
        settings.initial_layer_line_width_mm = 0.0;
        settings.brim_width_mm = 5.0;
        settings.brim_object_gap_mm = 0.0;
        settings.skirt_loops = 1;
        settings.skirt_height = 1;
        settings.skirt_distance_mm = 2.0;
        settings.draft_shield = bambu_config::DraftShield::Enabled;
        (square(20.0), settings)
    }

    #[test]
    fn draft_shield_drops_brim_rings_that_overlap_skirt() {
        let (contour, settings) = shield_brim_settings();
        let raw = brim(&[contour.clone()], &settings);
        let skirt_paths = skirt(&[contour], &settings);
        let trimmed = trim_brim_for_draft_shield(raw.clone(), &skirt_paths, &settings);
        let annulus = skirt_annulus(&skirt_paths, skirt_spacing_mm(&settings));
        assert!(!annulus.is_empty());
        assert!(
            path_length_mm(&trimmed) < path_length_mm(&raw) * 0.95,
            "trimmed={} raw={}",
            path_length_mm(&trimmed),
            path_length_mm(&raw)
        );
        assert!(!trimmed.is_empty());
        let leftover_in_annulus = crate::clip::clip_polylines(&trimmed, &annulus);
        assert!(
            leftover_in_annulus.is_empty(),
            "brim should not occupy the skirt band, leftover={}",
            leftover_in_annulus.len()
        );
    }

    #[test]
    fn draft_shield_keeps_brim_when_skirt_clears_it() {
        let (contour, mut settings) = shield_brim_settings();
        settings.brim_width_mm = 1.0;
        let raw = brim(&[contour.clone()], &settings);
        let skirt_paths = skirt(&[contour], &settings);
        let trimmed = trim_brim_for_draft_shield(raw.clone(), &skirt_paths, &settings);
        assert_eq!(trimmed.len(), raw.len());
        assert!((path_length_mm(&trimmed) - path_length_mm(&raw)).abs() < 1e-9);
    }

    #[test]
    fn no_draft_shield_leaves_brim_intact() {
        let (contour, mut settings) = shield_brim_settings();
        settings.draft_shield = bambu_config::DraftShield::Disabled;
        let raw = brim(&[contour.clone()], &settings);
        let skirt_paths = skirt(&[contour], &settings);
        let trimmed = trim_brim_for_draft_shield(raw.clone(), &skirt_paths, &settings);
        assert_eq!(trimmed.len(), raw.len());
    }
}
