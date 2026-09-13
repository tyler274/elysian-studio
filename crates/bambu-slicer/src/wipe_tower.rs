//! Prime tower (`PrintStep::WipeTower`).
//!
//! C++ `WipeTower` purges into a `prime_tower_width` square at `wipe_tower_x/y`
//! on tool changes. Sparse hatch plus rib walls; layer 0 adds brim.
//! Interface layers, framework spacing, and flat ironing follow the H2C knobs.
//! [`SliceSettings::wipe_tower_skips_sparse_layers`] skips fill on layers with
//! no toolchange, matching `wipe_tower_no_sparse_layers`.

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
    let mut spacing = (line * 2.2).max(line);
    if settings.prime_tower_enable_framework {
        spacing *= 2.0;
    }
    // Extra purge volume as filament count grows (C++ wipe-volume matrix stand-in).
    let purge = f64::from(settings.filament_count.max(2) as u32).sqrt();
    spacing = (spacing / purge).max(line);
    let clip = settings.nozzle_diameter_mm * LOOP_CLIPPING_OVER_NOZZLE;
    let brim_w = settings.prime_tower_brim_width_mm.max(0.0);
    let brim_loops = if brim_w > 1e-9 {
        (brim_w / line).round().max(1.0) as u32
    } else {
        0
    };
    let skip_sparse = settings.wipe_tower_skips_sparse_layers();
    let n = layers.len();
    let interface = settings.enable_tower_interface_features;
    layers.par_iter_mut().enumerate().for_each(|(i, layer)| {
        if skip_sparse && i > 0 && !layer.has_toolchange(settings) {
            layer.prime_tower = Vec::new();
            return;
        }
        let dense = interface && (i < 2 || i + 2 >= n);
        let hatch = if dense { line } else { spacing };
        let mut paths = Vec::new();
        let rib_loops = if interface { 2u32 } else { 1 };
        for k in 0..rib_loops {
            let rings = offset_polygons(&square, -line * (0.5 + f64::from(k)));
            for mut ring in rings {
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
        }
        paths.extend(infill::rectilinear(&fill, hatch, i, 0.0));
        if i == 0 && brim_loops > 0 {
            paths.extend(skirt_brim::concentric_loops(&square, 0.0, brim_loops, line));
        }
        if settings.prime_tower_flat_ironing && i + 1 == n {
            paths.extend(infill::concentric(
                &fill,
                (line * 0.4).max(0.08),
                settings.nozzle_diameter_mm * LOOP_CLIPPING_OVER_NOZZLE,
            ));
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

    fn dummy_layers(n: usize) -> Vec<Layer> {
        (0..n)
            .map(|i| Layer {
                z_mm: 0.2 * i as f64 + 0.1,
                index: i,
                height_mm: 0.2,
                print_z_mm: 0.2 * (i as f64 + 1.0),
                contours: Vec::new(),
                outer_walls: Vec::new(),
                inner_walls: Vec::new(),
                gap_infill: Vec::new(),
                infill_region: Vec::new(),
                infill: Vec::new(),
                combined_infill: Vec::new(),
                combined_infill_height_mm: 0.0,
                solid_infill: Vec::new(),
                floating_vertical_shell: Vec::new(),
                floating_areas: Vec::new(),
                top_surface: Vec::new(),
                bottom_surface: Vec::new(),
                bridge: Vec::new(),
                support: Vec::new(),
                support_interface: Vec::new(),
                support_region: Vec::new(),
                skirt: Vec::new(),
                brim: Vec::new(),
                ironing: Vec::new(),
                support_ironing: Vec::new(),
                prime_tower: Vec::new(),
                top_region: Vec::new(),
                support_enforcer: Vec::new(),
                support_blocker: Vec::new(),
                region_infill: Vec::new(),
                region_settings: Vec::new(),
                region_outer_walls: Vec::new(),
                region_inner_walls: Vec::new(),
                region_gap_infill: Vec::new(),
                region_fills: Vec::new(),
                lift_overhangs: Vec::new(),
            })
            .collect()
    }

    #[test]
    fn interface_ribs_and_ironing_add_paths() {
        let mut settings = SliceSettings::default();
        settings.enable_prime_tower = true;
        settings.filament_count = 2;
        settings.prime_tower_width_mm = 35.0;
        settings.enable_tower_interface_features = false;
        settings.prime_tower_flat_ironing = false;
        let mut off = dummy_layers(5);
        apply(&mut off, &settings);
        let sparse = off[2].prime_tower.len();
        settings.enable_tower_interface_features = true;
        settings.prime_tower_flat_ironing = true;
        settings.prime_tower_enable_framework = true;
        let mut on = dummy_layers(5);
        apply(&mut on, &settings);
        assert!(
            on[0].prime_tower.len() > sparse,
            "interface ribs should add a second wall: sparse={sparse} interface={}",
            on[0].prime_tower.len()
        );
        assert!(
            on[4].prime_tower.len() > on[2].prime_tower.len(),
            "flat ironing should densify the last layer"
        );
    }
}
