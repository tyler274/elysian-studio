//! Honeycomb infill (Slic3r `FillHoneycomb` zigzag columns).
//!
//! `spacing_mm` is already `line_width / density` from `infill_spacing_mm`.
//! C++ uses `this->spacing` (line width) and `params.density` to form the same
//! `distance = line_width / density`.

use bambu_geom::{unscale, Point, Polygon, Polyline};

use super::{bbox, clip_to_region};

pub fn fill(
    region: &[Polygon],
    spacing_mm: f64,
    density: f64,
    layer_index: usize,
) -> Vec<Polyline> {
    let Some((min, max)) = bbox(region) else {
        return Vec::new();
    };
    if !spacing_mm.is_finite() || spacing_mm <= 1e-6 {
        return Vec::new();
    }
    let density = density.clamp(1e-6, 1.0);
    // C++ `m.distance = min_spacing / density` with `min_spacing = this->spacing`.
    let distance = spacing_mm;
    let min_spacing = spacing_mm * density;
    let hex_side = distance / (3.0_f64.sqrt() / 2.0);
    let hex_width = distance * 2.0;
    let hex_height = hex_side * 2.0;
    let pattern_height = hex_height + hex_side;
    let y_short = distance * 3.0_f64.sqrt() / 3.0;
    let x_offset = min_spacing / 2.0;
    let y_offset = x_offset * 3.0_f64.sqrt() / 3.0;
    let hex_cx = hex_width / 2.0;
    let hex_cy = hex_side;
    // C++ `FillHoneycomb::_layer_angle`: π/3 × (layer % 3).
    let angle = std::f64::consts::FRAC_PI_3 * (layer_index % 3) as f64;

    let mut min_x = unscale(min.x);
    let mut min_y = unscale(min.y);
    let mut max_x = unscale(max.x);
    let mut max_y = unscale(max.y);
    rotate_bounds(
        &mut min_x, &mut min_y, &mut max_x, &mut max_y, angle, hex_cx, hex_cy,
    );
    min_x = align_to_grid(min_x, hex_width);
    min_y = align_to_grid(min_y, pattern_height);

    let y_step = y_short + hex_side + y_short + hex_side;
    let mut paths = Vec::new();
    let mut x = min_x;
    while x <= max_x {
        let mut pts = Vec::new();
        let mut ax0 = x + x_offset;
        let mut ax1 = x + distance - x_offset;
        for _ in 0..2 {
            pts.reverse();
            let mut y = min_y;
            while y <= max_y {
                pts.push(Point::from_mm(ax1, y + y_offset));
                pts.push(Point::from_mm(ax0, y + y_short - y_offset));
                pts.push(Point::from_mm(ax0, y + y_short + hex_side + y_offset));
                pts.push(Point::from_mm(
                    ax1,
                    y + y_short + hex_side + y_short - y_offset,
                ));
                pts.push(Point::from_mm(
                    ax1,
                    y + y_short + hex_side + y_short + hex_side + y_offset,
                ));
                y += y_step;
            }
            ax0 += distance;
            ax1 += distance;
            std::mem::swap(&mut ax0, &mut ax1);
            x += distance;
        }
        if pts.len() >= 2 {
            for p in &mut pts {
                let (px, py) = p.to_mm();
                let (rx, ry) = rotate_point(px, py, -angle, hex_cx, hex_cy);
                *p = Point::from_mm(rx, ry);
            }
            paths.push(pts);
        }
    }
    clip_to_region(paths, region)
}

fn rotate_point(x: f64, y: f64, angle: f64, cx: f64, cy: f64) -> (f64, f64) {
    let (s, c) = angle.sin_cos();
    let dx = x - cx;
    let dy = y - cy;
    (cx + dx * c - dy * s, cy + dx * s + dy * c)
}

fn rotate_bounds(
    min_x: &mut f64,
    min_y: &mut f64,
    max_x: &mut f64,
    max_y: &mut f64,
    angle: f64,
    cx: f64,
    cy: f64,
) {
    if angle.abs() < 1e-12 {
        return;
    }
    let corners = [
        (*min_x, *min_y),
        (*max_x, *min_y),
        (*max_x, *max_y),
        (*min_x, *max_y),
    ];
    let mut nmin_x = f64::INFINITY;
    let mut nmin_y = f64::INFINITY;
    let mut nmax_x = f64::NEG_INFINITY;
    let mut nmax_y = f64::NEG_INFINITY;
    for (x, y) in corners {
        let (rx, ry) = rotate_point(x, y, angle, cx, cy);
        nmin_x = nmin_x.min(rx);
        nmin_y = nmin_y.min(ry);
        nmax_x = nmax_x.max(rx);
        nmax_y = nmax_y.max(ry);
    }
    *min_x = nmin_x;
    *min_y = nmin_y;
    *max_x = nmax_x;
    *max_y = nmax_y;
}

/// C++ `align_to_grid`: never larger than `coord` (round toward −∞).
fn align_to_grid(coord: f64, spacing: f64) -> f64 {
    if spacing <= 1e-12 {
        return coord;
    }
    (coord / spacing).floor() * spacing
}

#[cfg(test)]
mod tests {
    use super::*;
    use bambu_geom::Point;

    fn square(size_mm: f64) -> Vec<Polygon> {
        vec![vec![
            Point::from_mm(0.0, 0.0),
            Point::from_mm(size_mm, 0.0),
            Point::from_mm(size_mm, size_mm),
            Point::from_mm(0.0, size_mm),
        ]]
    }

    #[test]
    fn five_percent_fills_a_40mm_square() {
        // 0.45 mm line at 5% → `infill_spacing_mm` = 9 mm.
        let paths = fill(&square(40.0), 9.0, 0.05, 0);
        assert!(
            paths.iter().any(|p| p.len() >= 2),
            "5% honeycomb should clip into a 40 mm square, got {} paths",
            paths.len()
        );
    }

    #[test]
    fn fifteen_percent_fills_a_20mm_cube_footprint() {
        let paths = fill(&square(20.0), 0.42 / 0.15, 0.15, 1);
        assert!(!paths.is_empty());
    }
}
