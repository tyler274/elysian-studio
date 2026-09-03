//! Honeycomb infill (Slic3r `FillHoneycomb` zigzag columns).

use bambu_geom::{unscale, Point, Polygon, Polyline};

use super::{bbox, clip_to_region};

pub fn fill(region: &[Polygon], spacing_mm: f64, density: f64) -> Vec<Polyline> {
    let Some((min, max)) = bbox(region) else {
        return Vec::new();
    };
    let density = density.max(0.05);
    let distance = spacing_mm / density;
    let hex_side = distance / (3.0_f64.sqrt() / 2.0);
    let y_short = distance * 3.0_f64.sqrt() / 3.0;

    let min_x = unscale(min.x);
    let max_x = unscale(max.x);
    let min_y = unscale(min.y);
    let max_y = unscale(max.y);

    let mut paths = Vec::new();
    let mut x = min_x;
    let mut column = 0u32;
    while x <= max_x + distance {
        let mut pts = Vec::new();
        let ax0 = x;
        let ax1 = x + distance * 0.5;
        let mut y = min_y;
        while y <= max_y + hex_side {
            if column.is_multiple_of(2) {
                pts.push(Point::from_mm(ax1, y));
                pts.push(Point::from_mm(ax0, y + y_short));
                pts.push(Point::from_mm(ax0, y + y_short + hex_side));
                pts.push(Point::from_mm(ax1, y + y_short * 2.0 + hex_side));
            } else {
                pts.push(Point::from_mm(ax0, y));
                pts.push(Point::from_mm(ax1, y + y_short));
                pts.push(Point::from_mm(ax1, y + y_short + hex_side));
                pts.push(Point::from_mm(ax0, y + y_short * 2.0 + hex_side));
            }
            y += y_short * 2.0 + hex_side;
        }
        if pts.len() >= 2 {
            paths.push(pts);
        }
        x += distance;
        column += 1;
    }
    clip_to_region(paths, region)
}
