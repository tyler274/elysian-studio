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
    let w = settings.line_width_mm;
    let loops = (settings.brim_width_mm / w).round().max(1.0) as u32;
    concentric_loops(contours, 0.0, loops, w)
}

pub fn skirt(footprint: &[Polygon], settings: &SliceSettings) -> Vec<Polyline> {
    if settings.skirt_loops == 0 {
        return Vec::new();
    }
    let start = settings.brim_width_mm + settings.skirt_distance_mm;
    concentric_loops(
        footprint,
        start,
        settings.skirt_loops,
        settings.line_width_mm,
    )
}
