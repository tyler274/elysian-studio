//! Skirt and outer brim (`PrintStep::SkirtBrim`).

use bambu_config::SliceSettings;
use bambu_geom::{offset_polygons, Polygon, Polyline};

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
    if settings.brim_width_mm <= 0.0 {
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
    } else if settings.brim_width_mm > 0.0 {
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
        let gapped = brim(&[contour], &settings);
        assert_eq!(gapped.len(), 3);
        let flush_inner = min_x(flush.last().unwrap());
        let gapped_inner = min_x(gapped.last().unwrap());
        assert!(
            (gapped_inner - (flush_inner - 0.5)).abs() < 0.05,
            "innermost brim should move out by the gap: flush={flush_inner} gapped={gapped_inner}"
        );
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
}
