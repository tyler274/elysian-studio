//! Classic offset perimeters (`PerimeterGenerator::process_classic`).

use bambu_config::SliceSettings;
use bambu_geom::{offset_polygons, Polygon, Polyline};

use crate::seams;

pub struct PerimeterResult {
    pub outer: Vec<Polyline>,
    pub inner: Vec<Polyline>,
    pub infill_region: Vec<Polygon>,
    pub seam_hint: Option<bambu_geom::Point>,
}

pub fn classic_perimeters(
    contours: &[Polygon],
    settings: &SliceSettings,
    seam_hint: Option<bambu_geom::Point>,
) -> PerimeterResult {
    let w = settings.line_width_mm;
    let loops = settings.wall_loops.max(1);
    let mut outer = Vec::new();
    let mut inner = Vec::new();
    let mut hint = seam_hint;

    for i in 0..loops {
        let inset = w * (i as f64 + 0.5);
        let mut rings = offset_polygons(contours, -inset);
        rings.retain(|r| r.len() >= 3);
        if rings.is_empty() {
            break;
        }
        for ring in &mut rings {
            seams::apply_seam(ring, settings.seam, hint);
            hint = ring.first().copied();
        }
        if i == 0 {
            outer.extend(rings);
        } else {
            inner.extend(rings);
        }
    }

    let infill_inset = w * (loops as f64 + 0.5);
    let infill_region = offset_polygons(contours, -infill_inset);

    PerimeterResult {
        outer,
        inner,
        infill_region,
        seam_hint: hint,
    }
}
