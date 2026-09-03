//! 3D honeycomb infill (Slic3r / Bambu `Fill3DHoneycomb`).
//!
//! Horizontal slices of a truncated-octahedron tessellation (David Eccles).
//! Coordinates stay in millimetres until conversion to scaled [`Point`]s.

use bambu_geom::{unscale, Point, Polygon, Polyline};

use super::{bbox, clip_to_region};

pub fn fill(region: &[Polygon], spacing_mm: f64, density: f64, z_mm: f64) -> Vec<Polyline> {
    let Some((min, max)) = bbox(region) else {
        return Vec::new();
    };
    // `spacing_mm` is already `line_width / density` from `infill_spacing_mm`.
    let density = density.max(0.05);
    let z_scale0 = std::f64::consts::SQRT_2;
    let mut grid = spacing_mm * ((z_scale0 + 1.0) / 2.0);
    if grid <= 1e-4 {
        return Vec::new();
    }

    let min_x = unscale(min.x) - grid;
    let min_y = unscale(min.y) - grid;
    let width = unscale(max.x - min.x) + 2.0 * grid;
    let height = unscale(max.y - min.y) + 2.0 * grid;

    let layer_h = spacing_mm.max(1e-4);
    let mut layers_per_module = ((grid * 2.0) / (z_scale0 * layer_h) + 0.05).floor();
    if density > 0.42 {
        layers_per_module = 2.0;
        grid = spacing_mm * 1.1;
    }
    layers_per_module = layers_per_module.max(2.0);
    let z_scale = (grid * 2.0) / (layers_per_module * layer_h);
    let grid = spacing_mm * ((z_scale + 1.0) / 2.0);

    let zpos = z_mm * z_scale;
    let paths = make_grid(zpos, grid, width, height);
    let shifted = paths
        .into_iter()
        .map(|pl| {
            pl.into_iter()
                .map(|p| {
                    let (x, y) = p.to_mm();
                    Point::from_mm(x + min_x, y + min_y)
                })
                .collect()
        })
        .collect();
    clip_to_region(shifted, region)
}

fn tri_wave(pos: f64, grid: f64) -> f64 {
    let mut t = (pos / (grid * 2.0)) + 0.25;
    t -= t.floor();
    (1.0 - (t * 8.0 - 4.0).abs()) * (grid / 4.0) + (grid / 4.0)
}

fn sgn(v: f64) -> f64 {
    if v > 0.0 {
        1.0
    } else if v < 0.0 {
        -1.0
    } else {
        0.0
    }
}

fn troct_wave(pos: f64, grid: f64, zpos: f64) -> f64 {
    let perp = tri_wave(zpos, grid) / 2.0;
    let y = tri_wave(pos, grid);
    if y.abs() > perp.abs() {
        sgn(y) * perp
    } else {
        y * sgn(perp)
    }
}

fn critical_points(zpos: f64, grid: f64) -> Vec<f64> {
    let mut res = vec![0.0];
    let perp = (tri_wave(zpos, grid) / 2.0).abs();
    let n = perp / grid;
    if n > 0.0 {
        res.push(grid * n);
        res.push(grid * (1.0 - n));
        res.push(grid * (1.0 + n));
        res.push(grid * (2.0 - n));
    }
    res
}

fn colinear_points(crit: &[f64], grid: f64, grid_length: f64) -> Vec<f64> {
    let mut points = vec![0.0];
    let mut c_loc = 0.0;
    while c_loc < grid_length {
        for &cp in crit {
            points.push(c_loc + cp);
        }
        c_loc += grid * 2.0;
    }
    points.push(grid_length);
    points
}

fn perpend_points(
    zpos: f64,
    crit: &[f64],
    grid: f64,
    grid_length: f64,
    offset_base: f64,
    perp_dir: f64,
) -> Vec<f64> {
    let mut points = vec![offset_base];
    let mut c_loc = 0.0;
    while c_loc < grid_length {
        for &cp in crit {
            let offset = troct_wave(cp, grid, zpos);
            points.push(offset_base + offset * perp_dir);
        }
        c_loc += grid * 2.0;
    }
    points.push(offset_base);
    points
}

fn zip_xy(xs: &[f64], ys: &[f64]) -> Polyline {
    let n = xs.len().min(ys.len());
    (0..n).map(|i| Point::from_mm(xs[i], ys[i])).collect()
}

fn make_grid(zpos: f64, grid: f64, bound_w: f64, bound_h: f64) -> Vec<Polyline> {
    let crit = critical_points(zpos, grid);
    let period = grid * 2.0;
    let z_cycle = {
        let mut t = (zpos + grid / 2.0) / period;
        t -= t.floor();
        t
    };
    let print_vert = z_cycle < 0.5;
    let mut out = Vec::new();
    if print_vert {
        let mut perp_dir = -1.0;
        let mut x = 0.0;
        while x <= bound_w {
            let xs = perpend_points(zpos, &crit, grid, bound_h, x, perp_dir);
            let ys = colinear_points(&crit, grid, bound_h);
            let mut line = zip_xy(&xs, &ys);
            if perp_dir > 0.0 {
                line.reverse();
            }
            if line.len() >= 2 {
                out.push(line);
            }
            x += grid;
            perp_dir *= -1.0;
        }
    } else {
        let mut perp_dir = 1.0;
        let mut y = grid;
        while y <= bound_h {
            let xs = colinear_points(&crit, grid, bound_w);
            let ys = perpend_points(zpos, &crit, grid, bound_w, y, perp_dir);
            let mut line = zip_xy(&xs, &ys);
            if perp_dir < 0.0 {
                line.reverse();
            }
            if line.len() >= 2 {
                out.push(line);
            }
            y += grid;
            perp_dir *= -1.0;
        }
    }
    out
}
