//! Slim / organic-ish tree supports (Bambu `tree(auto)` / `drop_nodes`).
//!
//! Contact samples drop toward local cluster centroids (organic trunks) while
//! staying outside an XY gap around the part. Disk radius follows
//! `tree_support_branch_diameter_angle`. This is not the full 3D organic solver.

use bambu_config::{SliceSettings, SupportBasePattern, LOOP_CLIPPING_OVER_NOZZLE};
use bambu_geom::{offset_polygons, scale, union_polygons, Point, Polygon, Polyline};
use rayon::prelude::*;

use crate::clip::point_in_polygons;
use crate::infill;
use crate::GpuAssist;
use crate::Layer;

const DISK_SIDES: usize = 12;
const MIN_MM: f64 = 0.05;

pub fn apply(
    layers: &mut [Layer],
    settings: &SliceSettings,
    overhangs: &[Vec<Polygon>],
    assist: Option<&GpuAssist>,
) {
    let n = layers.len();
    if n == 0 {
        return;
    }
    let diameter = settings.tree_branch_diameter_mm.max(settings.line_width_mm);
    let spacing = scale(settings.tree_branch_distance_mm.max(MIN_MM));
    let zs: Vec<f64> = layers.iter().map(|l| l.z_mm).collect();
    let contacts: Vec<Vec<Point>> = overhangs
        .par_iter()
        .enumerate()
        .map(|(i, overhang)| {
            let mut pts = sample_contacts(overhang, spacing);
            if overhang.is_empty() {
                return pts;
            }
            if let (Some(assist), Some(&z)) = (assist, zs.get(i)) {
                for (az, extra) in &assist.occupancy {
                    if (az - z).abs() < 1e-4 {
                        pts.extend(
                            extra
                                .iter()
                                .copied()
                                .filter(|p| point_in_polygons(*p, overhang)),
                        );
                    }
                }
            }
            pts
        })
        .collect();
    let radii: Vec<f64> = (0..n)
        .map(|i| settings.tree_branch_radius_mm(mm_to_top(i, layers, overhangs)))
        .collect();
    let nodes = drop_nodes(layers, settings, &contacts, diameter, &radii);
    draw(layers, settings, overhangs, &nodes, &radii);
}

fn sample_contacts(overhang: &[Polygon], spacing: i64) -> Vec<Point> {
    if overhang.is_empty() || spacing <= 0 {
        return Vec::new();
    }
    let mut pts = Vec::new();
    for poly in overhang {
        pts.extend(poly.iter().copied());
    }
    if let Some((min, max)) = infill::bbox(overhang) {
        let mut y = min.y;
        while y <= max.y {
            let mut x = min.x;
            while x <= max.x {
                let p = Point::new(x, y);
                if point_in_polygons(p, overhang) {
                    pts.push(p);
                }
                x = x.saturating_add(spacing);
            }
            y = y.saturating_add(spacing);
        }
    }
    merge_close(&pts, spacing / 2)
}

fn drop_nodes(
    layers: &[Layer],
    settings: &SliceSettings,
    contacts: &[Vec<Point>],
    diameter: f64,
    radii: &[f64],
) -> Vec<Vec<Point>> {
    let n = layers.len();
    let tan_a = settings.tree_branch_angle_deg.to_radians().tan();
    let merge = scale(diameter.max(MIN_MM));
    let cluster = merge.saturating_mul(4);
    let mut nodes = vec![Vec::new(); n];
    if n == 0 {
        return nodes;
    }
    let mut current = contacts[n - 1].clone();
    nodes[n - 1] = current.clone();
    for i in (0..n - 1).rev() {
        let dz = (layers[i + 1].print_z_mm - layers[i].print_z_mm).max(1e-6);
        let max_move = scale((dz * tan_a).max(MIN_MM));
        let xy = settings.support_xy_gap_mm(i) + radii[i];
        let forbidden = offset_polygons(&layers[i].contours, xy);
        let global = centroid(&current);
        let mut next: Vec<Point> = current
            .iter()
            .map(|p| {
                let trunk = cluster_centroid(*p, &current, cluster)
                    .or(global)
                    .unwrap_or(*p);
                let attractor = nearest_toward_trunk(*p, &current, trunk);
                let q = move_toward(*p, attractor, max_move);
                push_out(q, &forbidden, max_move)
            })
            .collect();
        next.extend(contacts[i].iter().copied());
        next = merge_close(&next, merge);
        nodes[i] = next.clone();
        current = next;
    }
    nodes
}

fn draw(
    layers: &mut [Layer],
    settings: &SliceSettings,
    overhangs: &[Vec<Polygon>],
    nodes: &[Vec<Point>],
    radii: &[f64],
) {
    let n = layers.len();
    let top_gap = settings.support_top_gap_layers();
    let interface_n = settings.support_interface_layers.max(1);
    let support_w = settings.line_width_for(bambu_config::FlowRole::SupportMaterial, false);
    let inset = support_w * 0.5;
    let interface_spacing = settings.support_interface_hatch_spacing_mm();
    let pad_spacing = support_w.max(MIN_MM);
    layers.par_iter_mut().enumerate().for_each(|(i, layer)| {
        if top_gap > 0 && super::overhang_in_range(overhangs, i, 0, top_gap) {
            return;
        }
        let radius = radii[i];
        let disks: Vec<Polygon> = nodes[i].iter().map(|p| regular_ngon(*p, radius)).collect();
        let unioned = union_polygons(&disks);
        let mut region = unioned.clone();
        let contact_idx = if top_gap == 0 { i } else { i + top_gap + 1 };
        let contact = overhangs.get(contact_idx).filter(|o| !o.is_empty());
        let is_roof = (1..=interface_n).any(|d| {
            let j = i + top_gap + d as usize;
            j < n && !overhangs[j].is_empty()
        });
        if let Some(overhang) = contact {
            region.extend(overhang.iter().cloned());
            region = union_polygons(&region);
        }
        layer.support_region = region.clone();
        if let Some(overhang) = contact {
            let fill = offset_polygons(overhang, -inset.min(radius * 0.25));
            let fill = if fill.is_empty() {
                overhang.to_vec()
            } else {
                fill
            };
            layer.support_interface = super::fill_support_interface(
                &fill,
                interface_spacing,
                i,
                settings,
                settings.support_interface_loop_pattern,
            );
        } else if is_roof {
            let fill = offset_polygons(&unioned, -inset);
            if !fill.is_empty() {
                layer.support_interface =
                    super::fill_support_interface(&fill, interface_spacing, i, settings, false);
            }
        } else if i == 0 {
            let pads = offset_polygons(&unioned, radius.max(0.4));
            let pads = if pads.is_empty() { unioned } else { pads };
            layer.support = infill::rectilinear(&pads, pad_spacing, i, settings.support_angle_deg);
            layer.support_region = pads;
        } else {
            layer.support = tree_trunk_paths(&unioned, i, settings, support_w);
        }
    });
}

/// C++ tree `make_perimeter_and_infill` / disk outlines.
///
/// BBL `tree_support_wall_count` `-1` plus `support_base_pattern` `default`
/// keeps one outline (hollow disks). Honeycomb / rectilinear / grid fill the
/// trunk. `0` is infill-only; `1`/`2` add sheath loops.
fn tree_trunk_paths(
    region: &[Polygon],
    layer_idx: usize,
    settings: &SliceSettings,
    support_w: f64,
) -> Vec<Polyline> {
    let wall_count = settings.tree_support_wall_count;
    let pattern = settings.support_base_pattern.tree_fill();
    let sheath = if wall_count < 0 {
        1_usize
    } else {
        wall_count.max(0) as usize
    };
    let mut out = Vec::new();
    if sheath == 1 {
        out.extend(region.iter().filter(|p| p.len() >= 3).cloned());
    } else if sheath >= 2 {
        out.extend(region.iter().filter(|p| p.len() >= 3).cloned());
        let clip = settings.nozzle_diameter_mm * LOOP_CLIPPING_OVER_NOZZLE;
        out.extend(
            infill::concentric(region, support_w.max(MIN_MM), clip)
                .into_iter()
                .take(sheath - 1),
        );
    }
    let want_fill = pattern != SupportBasePattern::None || wall_count == 0;
    if want_fill {
        let inset = if sheath > 0 {
            let inner = offset_polygons(region, -support_w * sheath as f64);
            if inner.is_empty() {
                region.to_vec()
            } else {
                inner
            }
        } else {
            region.to_vec()
        };
        out.extend(super::fill_support_base(
            &inset,
            settings.support_spacing_mm(),
            layer_idx,
            settings,
        ));
    }
    out
}

fn mm_to_top(i: usize, layers: &[Layer], overhangs: &[Vec<Polygon>]) -> f64 {
    overhangs
        .iter()
        .enumerate()
        .skip(i)
        .find(|(_, overhang)| !overhang.is_empty())
        .map(|(j, _)| (layers[j].print_z_mm - layers[i].print_z_mm).max(0.0))
        .unwrap_or(0.0)
}

fn regular_ngon(center: Point, radius_mm: f64) -> Polygon {
    let (cx, cy) = center.to_mm();
    let r = radius_mm.max(MIN_MM);
    (0..DISK_SIDES)
        .map(|i| {
            let a = (i as f64) * std::f64::consts::TAU / (DISK_SIDES as f64);
            Point::from_mm(cx + r * a.cos(), cy + r * a.sin())
        })
        .collect()
}

fn centroid(pts: &[Point]) -> Option<Point> {
    if pts.is_empty() {
        return None;
    }
    let n = pts.len() as i64;
    Some(Point::new(
        pts.iter().map(|p| p.x).sum::<i64>() / n,
        pts.iter().map(|p| p.y).sum::<i64>() / n,
    ))
}

fn cluster_centroid(p: Point, pts: &[Point], radius: i64) -> Option<Point> {
    let nearby: Vec<Point> = pts
        .iter()
        .copied()
        .filter(|q| dist(p, *q) <= radius)
        .collect();
    centroid(&nearby)
}

fn nearest_toward_trunk(p: Point, pts: &[Point], trunk: Point) -> Point {
    let p_to_trunk = dist(p, trunk);
    pts.iter()
        .copied()
        .filter(|q| *q != p && dist(*q, trunk) + 1 < p_to_trunk)
        .min_by_key(|q| dist(p, *q))
        .unwrap_or(trunk)
}

fn move_toward(p: Point, target: Point, max_move: i64) -> Point {
    let dx = target.x - p.x;
    let dy = target.y - p.y;
    let d = dist(p, target);
    if d == 0 || d <= max_move {
        return target;
    }
    Point::new(p.x + dx * max_move / d, p.y + dy * max_move / d)
}

fn push_out(p: Point, forbidden: &[Polygon], max_move: i64) -> Point {
    if forbidden.is_empty() || !point_in_polygons(p, forbidden) {
        return p;
    }
    let Some(q) = closest_on_polygons(p, forbidden) else {
        return p;
    };
    let mut dx = q.x - p.x;
    let mut dy = q.y - p.y;
    let mut d = dist(p, q);
    if d == 0 {
        dx = 1;
        dy = 0;
        d = 1;
    }
    let extra = scale(0.05).max(1);
    let t = (d + extra).min(max_move.max(d));
    Point::new(p.x + dx * t / d, p.y + dy * t / d)
}

fn merge_close(pts: &[Point], radius: i64) -> Vec<Point> {
    if pts.len() <= 1 {
        return pts.to_vec();
    }
    let r2 = sq(radius.max(1));
    let mut used = vec![false; pts.len()];
    let mut out = Vec::new();
    for i in 0..pts.len() {
        if used[i] {
            continue;
        }
        used[i] = true;
        let mut acc_x = pts[i].x as i128;
        let mut acc_y = pts[i].y as i128;
        let mut n = 1i128;
        for (j, p) in pts.iter().enumerate().skip(i + 1) {
            if used[j] || dist_sq(pts[i], *p) > r2 {
                continue;
            }
            used[j] = true;
            acc_x += p.x as i128;
            acc_y += p.y as i128;
            n += 1;
        }
        out.push(Point::new((acc_x / n) as i64, (acc_y / n) as i64));
    }
    out
}

fn closest_on_polygons(p: Point, polygons: &[Polygon]) -> Option<Point> {
    let mut best: Option<(i128, Point)> = None;
    for poly in polygons {
        let n = poly.len();
        if n < 2 {
            continue;
        }
        for i in 0..n {
            let a = poly[i];
            let b = poly[(i + 1) % n];
            let q = closest_on_segment(p, a, b);
            let d2 = dist_sq(p, q);
            if best.is_none_or(|(bd, _)| d2 < bd) {
                best = Some((d2, q));
            }
        }
    }
    best.map(|(_, q)| q)
}

fn closest_on_segment(p: Point, a: Point, b: Point) -> Point {
    let abx = (b.x - a.x) as i128;
    let aby = (b.y - a.y) as i128;
    let ab2 = abx * abx + aby * aby;
    if ab2 == 0 {
        return a;
    }
    let t = ((p.x - a.x) as i128 * abx + (p.y - a.y) as i128 * aby).clamp(0, ab2);
    Point::new(a.x + (abx * t / ab2) as i64, a.y + (aby * t / ab2) as i64)
}

fn dist(a: Point, b: Point) -> i64 {
    let d2 = dist_sq(a, b);
    let d = (d2 as f64).sqrt().round() as i64;
    d.max(0)
}

fn dist_sq(a: Point, b: Point) -> i128 {
    let dx = (a.x - b.x) as i128;
    let dy = (a.y - b.y) as i128;
    dx * dx + dy * dy
}

fn sq(v: i64) -> i128 {
    let v = v as i128;
    v * v
}
