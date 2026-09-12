//! Prime tower (`PrintStep::WipeTower`).
//!
//! C++ `WipeTower` purges into a `prime_tower_width` square at `wipe_tower_x/y`
//! on tool changes. This first cut fills that square on every object layer when
//! [`SliceSettings::has_wipe_tower`] (prime tower on, more than one filament,
//! not spiral). Sparse hatch plus an outline; layer 0 adds brim.

use bambu_config::{FlowRole, SliceSettings, LOOP_CLIPPING_OVER_NOZZLE};
use bambu_geom::{clip_end, offset_polygons, Point, Polygon};
use rayon::prelude::*;

use crate::infill;
use crate::skirt_brim;
use crate::Layer;

pub fn apply(layers: &mut [Layer], settings: &SliceSettings) {
    if !settings.has_wipe_tower() || layers.is_empty() {
        return;
    }
    if settings.prime_tower_width_mm <= 1e-6 {
        return;
    }
    let square = tower_square(settings);
    if square.first().map(|p| p.len()).unwrap_or(0) < 3 {
        return;
    }
    let line = settings
        .line_width_for(FlowRole::SparseInfill, false)
        .max(0.2);
    let inner = offset_polygons(&square, -line * 0.5);
    let fill = if inner.is_empty() {
        square.clone()
    } else {
        inner
    };
    let spacing = (line * 2.2).max(line);
    let clip = settings.nozzle_diameter_mm * LOOP_CLIPPING_OVER_NOZZLE;
    let brim_w = settings.prime_tower_brim_width_mm.max(0.0);
    let brim_loops = if brim_w > 1e-9 {
        (brim_w / line).round().max(1.0) as u32
    } else {
        0
    };
    layers.par_iter_mut().enumerate().for_each(|(i, layer)| {
        let mut paths = Vec::new();
        for mut ring in fill.iter().cloned() {
            if let (Some(&first), Some(&last)) = (ring.first(), ring.last()) {
                if first != last {
                    ring.push(first);
                }
            }
            let clipped = clip_end(&ring, clip);
            if clipped.len() >= 2 {
                paths.push(clipped);
            }
        }
        paths.extend(infill::rectilinear(&fill, spacing, i, 0.0));
        if i == 0 && brim_loops > 0 {
            paths.extend(skirt_brim::concentric_loops(&square, 0.0, brim_loops, line));
        }
        layer.prime_tower = paths;
    });
}

fn tower_square(settings: &SliceSettings) -> Vec<Polygon> {
    let x = settings.wipe_tower_x_mm;
    let y = settings.wipe_tower_y_mm;
    let w = settings.prime_tower_width_mm;
    vec![vec![
        Point::from_mm(x, y),
        Point::from_mm(x + w, y),
        Point::from_mm(x + w, y + w),
        Point::from_mm(x, y + w),
    ]]
}

#[cfg(test)]
mod tests {
    use super::*;
    use bambu_config::SliceSettings;

    #[test]
    fn square_sits_at_wipe_tower_origin() {
        let mut settings = SliceSettings::default();
        settings.wipe_tower_x_mm = 15.0;
        settings.wipe_tower_y_mm = 194.264;
        settings.prime_tower_width_mm = 35.0;
        let poly = &tower_square(&settings)[0];
        assert_eq!(poly[0], Point::from_mm(15.0, 194.264));
        assert_eq!(poly[2], Point::from_mm(50.0, 229.264));
    }
}
