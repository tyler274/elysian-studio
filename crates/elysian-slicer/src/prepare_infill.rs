//! `PrintObjectStep::PrepareInfill`: top / bottom / bridge vs sparse regions.
//!
//! Simplified `detect_surfaces_type` + `discover_horizontal_shells` /
//! `discover_vertical_shells`. Neighbor contours are grown slightly so clipper
//! slivers are not treated as shells. Shell windows follow C++ layer count **or**
//! `top_shell_thickness` / `bottom_shell_thickness`. When
//! `ensure_vertical_shell_thickness` is enabled, slope rings become extra
//! internal solid (`diff(infill, intersect(neighbor infills))`), then C++
//! `offset2_ex` / `shrink_ex` regularization and tiny in-model drop filtering
//! against neighbor `lslices`. Sparse islands at or below
//! `minimum_sparse_infill_area` become internal solid. C++ `combine_infill`
//! (`infill_combination`) intersects sparse across layers that fit under the
//! nozzle and prints the overlap on the uppermost layer at the stacked height.
//! Parameter modifiers fill each `LayerRegion` with its own settings (C++).
//! `interface_shells` treats same-region neighbors as the cover so stacked
//! materials get top/bottom skins at the joint (BBL `"0"` keeps all-region cover).
//! Internal solid that overlaps a wide `stTop` on the next layer is `stSubTop`
//! (`sub_top_surface_pattern`; C++ default monotonic).

use bambu_config::{EnsureVerticalShellThickness, Flow, FlowRole, InfillPattern, SliceSettings};
use bambu_geom::{
    difference_polygons, intersect_polygons, offset_polygons, offset_polygons_square,
    union_polygons, Point, Polygon, Polyline, TriangleMesh,
};
use rayon::prelude::*;

use crate::infill;
use crate::Layer;

const COVER_MM: f64 = 0.15;
/// C++ `EPSILON` used with shell thickness windows.
const SHELL_THICKNESS_EPSILON: f64 = 1e-4;
/// C++ `min_perimeter_infill_spacing = solid_infill_spacing * 1.05`.
const VERTICAL_SHELL_SPACING_SCALE: f64 = 1.05;

pub fn apply(layers: &mut [Layer], settings: &SliceSettings, mesh: Option<&TriangleMesh>) {
    if layers.is_empty() {
        return;
    }
    let nreg = layers
        .iter()
        .map(|layer| layer.region_infill.len())
        .max()
        .unwrap_or(0);
    if nreg <= 1 {
        fill_into(layers, settings, mesh, None, None);
        return;
    }
    let contours: Vec<Vec<Polygon>> = layers.iter().map(|l| l.contours.clone()).collect();
    for layer in layers.iter_mut() {
        layer.infill.clear();
        layer.combined_infill.clear();
        layer.combined_infill_height_mm = 0.0;
        layer.solid_infill.clear();
        layer.floating_vertical_shell.clear();
        layer.floating_areas.clear();
        layer.top_surface.clear();
        layer.bottom_surface.clear();
        layer.bridge.clear();
        layer.top_region.clear();
        layer.region_fills = vec![crate::RegionFills::default(); nreg];
    }
    let n = layers.len();
    let zs: Vec<f64> = layers.iter().map(|l| l.print_z_mm).collect();
    let union_infill: Vec<Vec<Polygon>> = layers.iter().map(|l| l.infill_region.clone()).collect();
    let mut shells = Vec::with_capacity(nreg);
    let mut cfgs = Vec::with_capacity(nreg);
    for r in 0..nreg {
        let regions: Vec<Vec<Polygon>> = layers
            .iter()
            .map(|layer| layer.region_infill.get(r).cloned().unwrap_or_default())
            .collect();
        let cfg = layers
            .iter()
            .find_map(|layer| layer.region_settings.get(r).cloned())
            .unwrap_or_else(|| settings.clone());
        let neighbors = if cfg.interface_shells {
            &regions
        } else {
            &union_infill
        };
        shells.push(detect_shells(&regions, neighbors, &zs, &contours, &cfg));
        cfgs.push((cfg, regions));
    }
    let mut shared_sparse = vec![Vec::new(); n];
    for map in &shells {
        for (i, polys) in map.sparse.iter().enumerate() {
            append_union(&mut shared_sparse[i], polys.clone());
        }
    }
    for (r, (cfg, regions)) in cfgs.into_iter().enumerate() {
        for (layer, region) in layers.iter_mut().zip(&regions) {
            layer.infill_region = region.clone();
        }
        emit_shells(
            layers,
            &cfg,
            mesh,
            &shells[r],
            Some(r),
            Some(&shared_sparse),
        );
    }
    for (layer, region) in layers.iter_mut().zip(union_infill) {
        layer.infill_region = region;
    }
}

fn fill_into(
    layers: &mut [Layer],
    settings: &SliceSettings,
    mesh: Option<&TriangleMesh>,
    region: Option<usize>,
    shared_sparse: Option<&[Vec<Polygon>]>,
) {
    let regions: Vec<Vec<Polygon>> = layers.iter().map(|l| l.infill_region.clone()).collect();
    let zs: Vec<f64> = layers.iter().map(|l| l.print_z_mm).collect();
    let contours: Vec<Vec<Polygon>> = layers.iter().map(|l| l.contours.clone()).collect();
    let shells = detect_shells(&regions, &regions, &zs, &contours, settings);
    emit_shells(layers, settings, mesh, &shells, region, shared_sparse);
}

struct ShellMap {
    top: Vec<Vec<Polygon>>,
    bottom: Vec<Vec<Polygon>>,
    solid: Vec<Vec<Polygon>>,
    sparse: Vec<Vec<Polygon>>,
}

fn detect_shells(
    regions: &[Vec<Polygon>],
    neighbors: &[Vec<Polygon>],
    zs: &[f64],
    contours: &[Vec<Polygon>],
    settings: &SliceSettings,
) -> ShellMap {
    let n = regions.len();
    let top_n = settings.top_shell_layers as usize;
    let bottom_n = settings.bottom_shell_layers as usize;
    let top_th = settings.top_shell_thickness_mm;
    let bottom_th = settings.bottom_shell_thickness_mm;

    let mut top = vec![Vec::new(); n];
    let mut bottom = vec![Vec::new(); n];
    if top_n > 0 {
        top.par_iter_mut().enumerate().for_each(|(i, slot)| {
            let above = neighbors.get(i + 1).map_or(&[][..], Vec::as_slice);
            *slot = difference_polygons(&regions[i], &cover(above));
        });
    }
    if bottom_n > 0 {
        bottom.par_iter_mut().enumerate().for_each(|(i, slot)| {
            let below = if i > 0 {
                neighbors[i - 1].as_slice()
            } else {
                &[]
            };
            *slot = difference_polygons(&regions[i], &cover(below));
        });
    }

    let mut solid = vec![Vec::new(); n];
    solid.par_iter_mut().enumerate().for_each(|(i, slot)| {
        let mut acc = top[i].clone();
        acc.extend(bottom[i].iter().cloned());
        *slot = union_polygons(&acc);
    });

    for j in 0..n {
        if top_n > 0 {
            for k in 1..=j {
                let dz = zs.get(j).copied().unwrap_or(0.0) - zs.get(j - k).copied().unwrap_or(0.0);
                if past_shell_window(k, top_n, top_th, dz) {
                    break;
                }
                if !within_shell_window(k, top_n, top_th, dz) {
                    continue;
                }
                let extra = intersect_polygons(&regions[j - k], &top[j]);
                append_union(&mut solid[j - k], extra);
            }
        }
        if bottom_n > 0 {
            for k in 1..(n - j) {
                let dz = zs.get(j + k).copied().unwrap_or(0.0) - zs.get(j).copied().unwrap_or(0.0);
                if past_shell_window(k, bottom_n, bottom_th, dz) {
                    break;
                }
                if !within_shell_window(k, bottom_n, bottom_th, dz) {
                    continue;
                }
                let extra = intersect_polygons(&regions[j + k], &bottom[j]);
                append_union(&mut solid[j + k], extra);
            }
        }
    }

    if settings.ensure_vertical_shell_thickness == EnsureVerticalShellThickness::Enabled {
        for (i, slot) in solid.iter_mut().enumerate() {
            let extra = vertical_shell_extra(i, regions, contours, zs, settings);
            append_union(slot, extra);
        }
    }

    let mut sparse: Vec<Vec<Polygon>> = (0..n)
        .into_par_iter()
        .map(|i| difference_polygons(&regions[i], &solid[i]))
        .collect();
    promote_small_sparse(&mut solid, &mut sparse, settings);
    ShellMap {
        top,
        bottom,
        solid,
        sparse,
    }
}

/// C++ `i < idx + n_layers || z_span < thickness - EPSILON`.
fn within_shell_window(k: usize, n_layers: usize, thickness_mm: f64, z_span: f64) -> bool {
    n_layers > 0
        && (k < n_layers || (thickness_mm > 0.0 && z_span + SHELL_THICKNESS_EPSILON < thickness_mm))
}

fn past_shell_window(k: usize, n_layers: usize, thickness_mm: f64, z_span: f64) -> bool {
    k >= n_layers && (thickness_mm <= 0.0 || z_span + SHELL_THICKNESS_EPSILON >= thickness_mm)
}

/// C++ `LayerRegion::prepare_fill_surfaces`: sparse islands at or below
/// `minimum_sparse_infill_area` become internal solid.
fn promote_small_sparse(
    solid: &mut [Vec<Polygon>],
    sparse: &mut [Vec<Polygon>],
    settings: &SliceSettings,
) {
    if settings.spiral_mode || settings.infill_density <= 0.0 {
        return;
    }
    let min_area = settings.minimum_sparse_infill_area_mm2;
    if min_area <= 0.0 {
        return;
    }
    for i in 0..sparse.len() {
        let mut small = Vec::new();
        sparse[i].retain(|poly| {
            let area = signed_area_mm2(poly);
            if area > 0.0 && area <= min_area {
                small.push(poly.clone());
                false
            } else {
                true
            }
        });
        if small.is_empty() {
            continue;
        }
        append_union(&mut solid[i], small);
        sparse[i] = difference_polygons(&sparse[i], &solid[i]);
    }
}

fn signed_area_mm2(poly: &[Point]) -> f64 {
    if poly.len() < 3 {
        return 0.0;
    }
    let mut acc = 0.0;
    for i in 0..poly.len() {
        let a = poly[i];
        let b = poly[(i + 1) % poly.len()];
        let (ax, ay) = a.to_mm();
        let (bx, by) = b.to_mm();
        acc += ax * by - bx * ay;
    }
    acc * 0.5
}

/// C++ `discover_vertical_shells`: extra internal solid is infill minus the
/// intersection of neighbor `fill_expolygons` in the shell window.
fn vertical_shell_extra(
    i: usize,
    regions: &[Vec<Polygon>],
    contours: &[Vec<Polygon>],
    zs: &[f64],
    settings: &SliceSettings,
) -> Vec<Polygon> {
    let n = regions.len();
    if i >= n || regions[i].is_empty() {
        return Vec::new();
    }
    let mut holes = regions[i].clone();
    let z_i = zs.get(i).copied().unwrap_or(0.0);
    let top_n = settings.top_shell_layers as usize;
    let bottom_n = settings.bottom_shell_layers as usize;
    if top_n > 0 {
        for k in 1..n - i {
            let j = i + k;
            let dz = zs.get(j).copied().unwrap_or(z_i) - z_i;
            if past_shell_window(k, top_n, settings.top_shell_thickness_mm, dz) {
                break;
            }
            if !within_shell_window(k, top_n, settings.top_shell_thickness_mm, dz) {
                continue;
            }
            if combine_holes(&mut holes, &regions[j]) {
                break;
            }
        }
    }
    if bottom_n > 0 && !holes.is_empty() {
        for k in 1..=i {
            let j = i - k;
            let dz = z_i - zs.get(j).copied().unwrap_or(z_i);
            if past_shell_window(k, bottom_n, settings.bottom_shell_thickness_mm, dz) {
                break;
            }
            if !within_shell_window(k, bottom_n, settings.bottom_shell_thickness_mm, dz) {
                continue;
            }
            if combine_holes(&mut holes, &regions[j]) {
                break;
            }
        }
    }
    let lower = if i > 0 {
        contours.get(i - 1).map_or(&[][..], Vec::as_slice)
    } else {
        &[]
    };
    let upper = contours.get(i + 1).map_or(&[][..], Vec::as_slice);
    regularize_vertical_shell(
        difference_polygons(&regions[i], &holes),
        settings.line_width_for(FlowRole::SolidInfill, i == 0),
        &regions[i],
        lower,
        upper,
    )
}

/// C++ `combine_holes`: empty neighbor clears the intersection (all internal
/// becomes extra solid).
fn combine_holes(holes: &mut Vec<Polygon>, neighbor: &[Polygon]) -> bool {
    if holes.is_empty() || neighbor.is_empty() {
        holes.clear();
        return true;
    }
    *holes = intersect_polygons(holes, neighbor);
    holes.is_empty()
}

/// C++ `discover_vertical_shells` `offset2_ex` / `shrink_ex` (jtSquare).
/// Open regions narrower than 0.65× spacing, close gaps under 1.2× spacing,
/// then expand by 0.2× spacing. Drop crumbs that are tiny (or small and fully
/// wrapped in neighbor `lslices`) unless expanding them would cover an internal
/// island.
fn regularize_vertical_shell(
    shell: Vec<Polygon>,
    line_width_mm: f64,
    internal: &[Polygon],
    lower_lslices: &[Polygon],
    upper_lslices: &[Polygon],
) -> Vec<Polygon> {
    if shell.is_empty() || line_width_mm <= 0.0 {
        return shell;
    }
    let spacing = line_width_mm * VERTICAL_SHELL_SPACING_SCALE;
    let narrow_ensure = 0.5 * 0.65 * spacing;
    let narrow_sparse = 0.5 * 1.2 * spacing;
    let tiny_overlap = 0.2 * spacing;
    let opened = offset_polygons_square(&union_polygons(&shell), -narrow_ensure);
    if opened.is_empty() {
        return Vec::new();
    }
    let closed = offset_polygons_square(&opened, narrow_ensure + narrow_sparse);
    let grown = offset_polygons_square(&closed, -(narrow_sparse - tiny_overlap));
    filter_tiny_vertical_drops(grown, spacing, internal, lower_lslices, upper_lslices)
}

/// C++ `regularized_shell.erase(remove_if)` after `offset2_ex`.
fn filter_tiny_vertical_drops(
    shell: Vec<Polygon>,
    spacing: f64,
    internal: &[Polygon],
    lower_lslices: &[Polygon],
    upper_lslices: &[Polygon],
) -> Vec<Polygon> {
    let min_area = spacing * 1.5;
    let wrap_area = spacing * 8.0;
    let object_volume = intersect_polygons(lower_lslices, upper_lslices);
    shell
        .into_iter()
        .filter(|p| {
            let area = signed_area_mm2(p).abs();
            let tiny = area < min_area;
            let wrapped = area < wrap_area
                && difference_polygons(std::slice::from_ref(p), &object_volume).is_empty();
            if !tiny && !wrapped {
                return true;
            }
            let expanded = offset_polygons(std::slice::from_ref(p), spacing);
            difference_polygons(internal, &expanded).len() < internal.len()
        })
        .collect()
}

fn emit_shells(
    layers: &mut [Layer],
    settings: &SliceSettings,
    mesh: Option<&TriangleMesh>,
    shells: &ShellMap,
    region: Option<usize>,
    shared_sparse: Option<&[Vec<Polygon>]>,
) {
    let zs: Vec<f64> = layers.iter().map(|l| l.z_mm).collect();
    let heights: Vec<f64> = layers.iter().map(|l| l.height_mm).collect();
    let mut leftover = shells.sparse.clone();
    let (thick, thick_h) = combine_sparse_infill(&mut leftover, &heights, settings);
    let leftover_paths = sparse_paths(&leftover, &zs, settings, mesh);
    let thick_paths = if thick.iter().all(Vec::is_empty) {
        vec![Vec::new(); leftover.len()]
    } else {
        sparse_paths(&thick, &zs, settings, mesh)
    };
    let axis = infill::object_center_x(settings, mesh);
    let lower_src = shared_sparse.unwrap_or(&shells.sparse);

    layers.par_iter_mut().enumerate().for_each(|(i, layer)| {
        let first = i == 0;
        let solid_w = settings.line_width_for(FlowRole::SolidInfill, first);
        let top_w = settings.line_width_for(FlowRole::TopSolidInfill, first);
        let mut rest = difference_polygons(&shells.solid[i], &shells.top[i]);
        rest = difference_polygons(&rest, &shells.bottom[i]);
        let lower_sparse = if i > 0 {
            lower_src[i - 1].as_slice()
        } else {
            &[]
        };
        let (wide, narrow, floating) = classify_internal_solid(&rest, lower_sparse, settings);
        let next_top = shells.top.get(i + 1).map_or(&[][..], Vec::as_slice);
        let (sub_top, wide) = split_sub_top(&wide, next_top, solid_w);

        let top_region = shells.top[i].clone();
        let top_surface =
            SliceSettings::surface_fill_spacing_mm(top_w, settings.top_surface_density)
                .map(|spacing| {
                    infill::with_symmetric_y(&shells.top[i], axis, |region| {
                        infill::connect_monotonic_line_wipes(
                            infill::solid_surface(
                                region,
                                spacing,
                                i,
                                settings.top_surface_pattern,
                                settings.infill_direction_deg,
                                settings.nozzle_diameter_mm,
                            ),
                            settings.top_surface_pattern,
                            spacing,
                            settings.monotonic_travel_into_wall,
                        )
                    })
                })
                .unwrap_or_default();
        let bottom_w = if i > 0 {
            Flow::bridging_flow(
                settings,
                FlowRole::SolidInfill,
                layer.height_mm,
                first,
                settings.thick_bridges,
            )
            .spacing_mm()
        } else {
            solid_w
        };
        let bottom_density = if i == 0 {
            settings.bottom_surface_density
        } else {
            1.0
        };
        let bottom_paths = SliceSettings::surface_fill_spacing_mm(bottom_w, bottom_density)
            .map(|spacing| {
                infill::with_symmetric_y(&shells.bottom[i], axis, |region| {
                    infill::connect_monotonic_line_wipes(
                        infill::solid_surface(
                            region,
                            spacing,
                            i.wrapping_add(1),
                            settings.bottom_surface_pattern,
                            settings.bridge_fill_angle_deg(i > 0),
                            settings.nozzle_diameter_mm,
                        ),
                        settings.bottom_surface_pattern,
                        spacing,
                        settings.monotonic_travel_into_wall,
                    )
                })
            })
            .unwrap_or_default();
        let mut solid_infill = infill::with_symmetric_y(&wide, axis, |region| {
            infill::connect_monotonic_line_wipes(
                infill::solid_surface(
                    region,
                    solid_w,
                    i,
                    settings.internal_solid_infill_pattern,
                    settings.infill_direction_deg,
                    settings.nozzle_diameter_mm,
                ),
                settings.internal_solid_infill_pattern,
                solid_w,
                settings.monotonic_travel_into_wall,
            )
        });
        solid_infill.extend(infill::with_symmetric_y(&sub_top, axis, |region| {
            infill::connect_monotonic_line_wipes(
                infill::solid_surface(
                    region,
                    solid_w,
                    i,
                    settings.sub_top_surface_pattern,
                    settings.infill_direction_deg,
                    settings.nozzle_diameter_mm,
                ),
                settings.sub_top_surface_pattern,
                solid_w,
                settings.monotonic_travel_into_wall,
            )
        }));
        solid_infill.extend(infill::with_symmetric_y(&narrow, axis, |region| {
            closed_concentric(region, solid_w, settings.nozzle_diameter_mm)
        }));
        let floating_vertical_shell = infill::with_symmetric_y(&floating, axis, |region| {
            closed_concentric(region, solid_w, settings.nozzle_diameter_mm)
        });
        let infill = leftover_paths[i].clone();
        let combined = thick_paths[i].clone();
        let combined_h = thick_h[i];
        if let Some(r) = region {
            let fills = crate::RegionFills {
                sparse: infill.clone(),
                combined: combined.clone(),
                combined_height_mm: combined_h,
                solid: solid_infill.clone(),
                floating: floating_vertical_shell.clone(),
                floating_areas: lower_sparse.to_vec(),
                top: top_surface.clone(),
                bottom: if i == 0 {
                    bottom_paths.clone()
                } else {
                    Vec::new()
                },
                bridge: if i == 0 {
                    Vec::new()
                } else {
                    bottom_paths.clone()
                },
            };
            if let Some(slot) = layer.region_fills.get_mut(r) {
                *slot = fills;
            }
            append_union(&mut layer.top_region, top_region);
            layer.top_surface.extend(top_surface);
            if i == 0 {
                layer.bottom_surface.extend(bottom_paths);
            } else {
                layer.bridge.extend(bottom_paths);
            }
            layer.solid_infill.extend(solid_infill);
            layer
                .floating_vertical_shell
                .extend(floating_vertical_shell);
            append_union(&mut layer.floating_areas, lower_sparse.to_vec());
            layer.infill.extend(infill);
            layer.combined_infill.extend(combined);
            if combined_h > 0.0 {
                layer.combined_infill_height_mm = combined_h;
            }
        } else {
            layer.top_region = top_region;
            layer.top_surface = top_surface;
            if i == 0 {
                layer.bottom_surface = bottom_paths;
            } else {
                layer.bridge = bottom_paths;
            }
            layer.solid_infill = solid_infill;
            layer.floating_vertical_shell = floating_vertical_shell;
            layer.floating_areas = lower_sparse.to_vec();
            layer.infill = infill;
            layer.combined_infill = combined;
            layer.combined_infill_height_mm = combined_h;
        }
    });
}

/// C++ `PrintObject::combine_infill`. Intersect sparse (`stInternal`) across
/// consecutive layers whose combined height stays under the nozzle diameter.
/// Void the overlap on lower layers with a clearance ring so thick infill
/// cannot collide with walls. The uppermost layer keeps leftover sparse at the
/// original height and the intersection at the stacked `Surface.thickness`.
fn combine_sparse_infill(
    sparse: &mut [Vec<Polygon>],
    heights: &[f64],
    settings: &SliceSettings,
) -> (Vec<Vec<Polygon>>, Vec<f64>) {
    let n = sparse.len();
    let mut thick = vec![Vec::new(); n];
    let mut thick_h = vec![0.0; n];
    if !settings.infill_combination || settings.infill_density <= 0.0 || n < 2 {
        return (thick, thick_h);
    }
    let nozzle = combine_infill_nozzle_mm(settings);
    // C++ `m_layers` is object layers only; skip raft and the first object layer
    // (`layer->id() == 0`).
    let first_object = settings.raft_layers as usize;
    let mut combine = vec![0usize; n];
    let mut current_height = 0.0;
    let mut num_layers = 0usize;
    for layer_idx in 0..n {
        if layer_idx <= first_object {
            continue;
        }
        let height = heights
            .get(layer_idx)
            .copied()
            .unwrap_or(settings.layer_height_mm);
        if current_height + height >= nozzle + SHELL_THICKNESS_EPSILON {
            combine[layer_idx - 1] = num_layers;
            current_height = 0.0;
            num_layers = 0;
        }
        current_height += height;
        num_layers += 1;
    }
    combine[n - 1] = num_layers;

    let peri = Flow::for_role(
        settings,
        FlowRole::Perimeter,
        settings.layer_height_mm,
        false,
    )
    .width_mm;
    let solid = Flow::for_role(
        settings,
        FlowRole::SolidInfill,
        settings.layer_height_mm,
        false,
    );
    let extra = match settings.infill_pattern {
        InfillPattern::Rectilinear | InfillPattern::Grid | InfillPattern::Honeycomb => 1.5,
        _ => 0.5,
    };
    let clearance = 0.5 * peri + extra * solid.width_mm;
    let spacing = solid.spacing_mm();
    let area_threshold = spacing * spacing;

    for layer_idx in 0..n {
        let num_layers = combine[layer_idx];
        if num_layers <= 1 {
            continue;
        }
        let start = layer_idx + 1 - num_layers;
        let mut intersection = sparse[start].clone();
        for next in sparse.iter().take(layer_idx + 1).skip(start + 1) {
            intersection = intersect_polygons(&intersection, next);
            if intersection.is_empty() {
                break;
            }
        }
        if area_threshold > 0.0 {
            intersection.retain(|poly| signed_area_mm2(poly).abs() > area_threshold);
        }
        if intersection.is_empty() {
            continue;
        }
        let grown = offset_polygons(&intersection, clearance);
        for slot in sparse.iter_mut().take(layer_idx + 1).skip(start) {
            *slot = difference_polygons(slot, &grown);
        }
        thick_h[layer_idx] = heights
            .iter()
            .take(layer_idx + 1)
            .skip(start)
            .sum::<f64>()
            .max(settings.layer_height_mm);
        thick[layer_idx] = intersection;
    }
    (thick, thick_h)
}

fn combine_infill_nozzle_mm(settings: &SliceSettings) -> f64 {
    let dia = |filament: i32| {
        let idx = filament.max(1) as usize - 1;
        settings
            .nozzle_diameters_mm
            .get(idx)
            .copied()
            .unwrap_or(settings.nozzle_diameter_mm)
    };
    dia(settings.sparse_infill_filament)
        .min(dia(settings.solid_infill_filament))
        .max(1e-6)
}

/// C++ `NARROW_INFILL_AREA_THRESHOLD` in `Fill.cpp`.
const NARROW_INFILL_AREA_THRESHOLD_MM: f64 = 3.0;

/// C++ `group_fills`: narrow internal solid over lower-layer sparse becomes
/// `stFloatingVerticalShell` (`ipFloatingConcentric`); other narrow islands use
/// `ipConcentricInternal`.
fn classify_internal_solid(
    rest: &[Polygon],
    lower_sparse: &[Polygon],
    settings: &SliceSettings,
) -> (Vec<Polygon>, Vec<Polygon>, Vec<Polygon>) {
    if !settings.detect_narrow_internal_solid_infill {
        return (rest.to_vec(), Vec::new(), Vec::new());
    }
    let mut wide = Vec::new();
    let mut narrow = Vec::new();
    let mut floating = Vec::new();
    for poly in rest {
        if poly.len() < 3 || !is_narrow_infill_area(poly) {
            wide.push(poly.clone());
            continue;
        }
        if overlaps_lower_internal(poly, lower_sparse) {
            floating.push(poly.clone());
        } else {
            narrow.push(poly.clone());
        }
    }
    (wide, narrow, floating)
}

/// C++ `PrintObject::discover_sub_top_surfaces`. An island narrower than this
/// erosion cannot justify a sub-top band.
const SUB_TOP_MIN_TOP_EROSION_MM: f64 = 1.5;

/// C++ `stSubTop`: retype a wide internal-solid island that overlaps a large
/// enough top on the next layer. The whole island uses `sub_top_surface_pattern`.
fn split_sub_top(
    wide: &[Polygon],
    next_top: &[Polygon],
    solid_spacing_mm: f64,
) -> (Vec<Polygon>, Vec<Polygon>) {
    if wide.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let mask = large_sub_top_mask(next_top);
    if mask.is_empty() {
        return (Vec::new(), wide.to_vec());
    }
    let open_r = 0.5 * solid_spacing_mm.max(0.0);
    let mut sub_top = Vec::new();
    let mut rest = Vec::new();
    for island in wide {
        let hit = intersect_polygons(std::slice::from_ref(island), &mask);
        let claimed = if open_r > 1e-9 {
            !offset_polygons(&offset_polygons(&hit, -open_r), open_r).is_empty()
        } else {
            !hit.is_empty()
        };
        if claimed {
            sub_top.push(island.clone());
        } else {
            rest.push(island.clone());
        }
    }
    (sub_top, rest)
}

fn large_sub_top_mask(tops: &[Polygon]) -> Vec<Polygon> {
    let min_extent = 2.0 * SUB_TOP_MIN_TOP_EROSION_MM;
    union_polygons(tops)
        .into_iter()
        .filter(|top| {
            let Some((w, h)) = polygon_size_mm(top) else {
                return false;
            };
            w >= min_extent
                && h >= min_extent
                && !offset_polygons(std::slice::from_ref(top), -SUB_TOP_MIN_TOP_EROSION_MM)
                    .is_empty()
        })
        .collect()
}

fn polygon_size_mm(poly: &Polygon) -> Option<(f64, f64)> {
    if poly.is_empty() {
        return None;
    }
    let mut min_x = f64::INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    for p in poly {
        let (x, y) = p.to_mm();
        min_x = min_x.min(x);
        min_y = min_y.min(y);
        max_x = max_x.max(x);
        max_y = max_y.max(y);
    }
    Some((max_x - min_x, max_y - min_y))
}

fn is_narrow_infill_area(poly: &Polygon) -> bool {
    offset_polygons(std::slice::from_ref(poly), -NARROW_INFILL_AREA_THRESHOLD_MM).is_empty()
}

fn overlaps_lower_internal(poly: &Polygon, lower: &[Polygon]) -> bool {
    if lower.is_empty() {
        return false;
    }
    let grown = offset_polygons(std::slice::from_ref(poly), 1e-3);
    !intersect_polygons(&grown, lower).is_empty()
}

fn closed_concentric(region: &[Polygon], spacing_mm: f64, nozzle_mm: f64) -> Vec<Polyline> {
    infill::concentric(
        region,
        spacing_mm,
        nozzle_mm.max(0.0) * bambu_config::LOOP_CLIPPING_OVER_NOZZLE,
    )
}

fn sparse_paths(
    sparse: &[Vec<Polygon>],
    zs: &[f64],
    settings: &SliceSettings,
    mesh: Option<&TriangleMesh>,
) -> Vec<Vec<Polyline>> {
    let axis = infill::object_center_x(settings, mesh);
    match settings.infill_pattern {
        InfillPattern::Lightning => infill::with_symmetric_y_layers(sparse, axis, |regions| {
            infill::generate_lightning(regions, settings)
                .into_iter()
                .map(|paths| infill::apply_sparse_multiline(paths, settings))
                .collect()
        }),
        InfillPattern::AdaptiveCubic | InfillPattern::SupportCubic => {
            let support_only = settings.infill_pattern == InfillPattern::SupportCubic;
            let spacing = infill::adaptive::line_spacing_mm(settings);
            let octree =
                mesh.and_then(|mesh| infill::adaptive::Octree::build(mesh, spacing, support_only));
            (0..sparse.len())
                .into_par_iter()
                .map(|i| {
                    infill::with_symmetric_y(&sparse[i], axis, |region| {
                        let paths = octree
                            .as_ref()
                            .map(|octree| infill::adaptive::fill(region, octree, zs[i]))
                            .unwrap_or_default();
                        infill::apply_sparse_multiline(paths, settings)
                    })
                })
                .collect()
        }
        _ => (0..sparse.len())
            .into_par_iter()
            .map(|i| {
                infill::with_symmetric_y(&sparse[i], axis, |region| {
                    infill::generate(region, settings, i, zs[i])
                })
            })
            .collect(),
    }
}

fn cover(polygons: &[Polygon]) -> Vec<Polygon> {
    if polygons.is_empty() {
        Vec::new()
    } else {
        offset_polygons(polygons, COVER_MM)
    }
}

fn append_union(dst: &mut Vec<Polygon>, extra: Vec<Polygon>) {
    if extra.is_empty() {
        return;
    }
    let mut acc = std::mem::take(dst);
    acc.extend(extra);
    *dst = union_polygons(&acc);
}

#[cfg(test)]
mod tests {
    use super::*;
    use bambu_geom::Point;

    fn rect(width_mm: f64, height_mm: f64) -> Polygon {
        vec![
            Point::from_mm(0.0, 0.0),
            Point::from_mm(width_mm, 0.0),
            Point::from_mm(width_mm, height_mm),
            Point::from_mm(0.0, height_mm),
        ]
    }

    #[test]
    fn regularize_drops_narrow_vertical_shell() {
        let w = 0.42;
        assert!(
            regularize(vec![rect(0.15, 10.0)], w).is_empty(),
            "opening 0.65× spacing should drop a 0.15 mm ring"
        );
        let kept = regularize(vec![rect(2.0, 10.0)], w);
        assert!(
            !kept.is_empty(),
            "a 2 mm slope band should survive regularization"
        );
    }

    fn regularize(shell: Vec<Polygon>, w: f64) -> Vec<Polygon> {
        regularize_vertical_shell(shell, w, &[], &[], &[])
    }

    #[test]
    fn filter_tiny_drops_in_model_crumbs() {
        let spacing = 0.42 * VERTICAL_SHELL_SPACING_SCALE;
        let island = rect(1.5, 2.0);
        let body = rect(10.0, 10.0);
        let dropped = filter_tiny_vertical_drops(
            vec![island.clone()],
            spacing,
            std::slice::from_ref(&body),
            std::slice::from_ref(&body),
            std::slice::from_ref(&body),
        );
        assert!(
            dropped.is_empty(),
            "a 3 mm² in-model crumb should drop when it does not cover internal infill"
        );
        let core = rect(0.4, 0.4);
        let kept = filter_tiny_vertical_drops(
            vec![island],
            spacing,
            std::slice::from_ref(&core),
            std::slice::from_ref(&body),
            std::slice::from_ref(&body),
        );
        assert!(
            !kept.is_empty(),
            "keep a small shell that covers a thin internal island"
        );
        let wide = rect(2.0, 10.0);
        let survived = filter_tiny_vertical_drops(
            vec![wide],
            spacing,
            std::slice::from_ref(&body),
            std::slice::from_ref(&body),
            std::slice::from_ref(&body),
        );
        assert!(
            !survived.is_empty(),
            "a 20 mm² slope band stays even when fully in-model"
        );
    }
}
