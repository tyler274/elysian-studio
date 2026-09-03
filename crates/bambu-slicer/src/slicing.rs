//! Object layering as a pure function of height + first-layer height.
//!
//! Matches Bambu `generate_object_layers` (`Slicing.cpp`) and PrusaSlicer 3.0
//! `Slic3r::Biz::Algorithms::LayerHeight::generate_object_layers`: each layer is
//! a `[bottom_z, print_z]` slab (`Domain::LayerZRange`). Contours are taken at
//! mid-slab `slice_z`; G-code / preview use `print_z` (top of the slab).
//!
//! Bambu `precise_z_height` redistributes the last five slabs so `print_z`
//! lands on the object top. PrusaSlicer 3.0 leaves a FIXME for that align.
//! With a raft, the first object slab uses `layer_height` (the bed first-print
//! height is the raft flange). Raft layers and the G-code Z lift are applied
//! after contours exist. Z shrinkage compensation is not applied yet.

use bambu_config::SliceSettings;
use bambu_geom::TriangleMesh;

use crate::SlicerError;

/// Slic3r / PrusaSlicer `Domain::EPSILON`.
const EPSILON: f64 = 1e-4;

/// One object layer before contours exist (C++ `new_layers` input).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayerSpec {
    pub index: usize,
    /// Bottom of the slab (`LayerZRange::bottom_z`), object-relative until
    /// [`layer_plan`] adds the mesh AABB `zmin`.
    pub bottom_z_mm: f64,
    /// Mid-slab plane used for triangle–plane intersection (`Layer::slice_z`).
    pub slice_z_mm: f64,
    /// Top of the slab; nozzle Z after the layer (`Layer::print_z`).
    pub print_z_mm: f64,
    /// Slab thickness (`Layer::height`).
    pub height_mm: f64,
}

impl LayerSpec {
    fn from_range(index: usize, bottom_z_mm: f64, print_z_mm: f64) -> Self {
        let height_mm = print_z_mm - bottom_z_mm;
        Self {
            index,
            bottom_z_mm,
            slice_z_mm: 0.5 * (bottom_z_mm + print_z_mm),
            print_z_mm,
            height_mm,
        }
    }
}

/// Bottom/top pairs from `generate_object_layers`, plus derived slice_z / height.
pub fn generate_object_layers(object_height_mm: f64, settings: &SliceSettings) -> Vec<LayerSpec> {
    if object_height_mm <= EPSILON {
        return Vec::new();
    }

    let h0 = settings.first_object_layer_height_mm();
    let h = settings.layer_height_mm.max(1e-6);
    // C++ `SlicingParameters`: min layer height is clamped to not exceed `layer_height`.
    let min_h = settings.min_layer_height_mm.max(0.01).min(h);

    let mut out = Vec::new();
    let mut print_z = h0;
    out.push(LayerSpec::from_range(0, 0.0, print_z));

    // C++ probes the next mid-plane against object height before committing a slab.
    let mut slice_z = print_z + 0.5 * min_h;
    while slice_z < object_height_mm {
        let height = h;
        let lo = print_z;
        slice_z = lo + 0.5 * height;
        if slice_z + EPSILON >= object_height_mm {
            break;
        }
        print_z += height;
        out.push(LayerSpec::from_range(out.len(), lo, print_z));
        slice_z = print_z + 0.5 * min_h;
    }

    if settings.precise_z_height {
        align_last_layers_to_object_height(&mut out, object_height_mm, settings);
    }
    out
}

/// Bambu `adjust_layer_series_to_align_object_height`: spread the top gap
/// across the last five slabs, clamped to min/max layer height.
fn align_last_layers_to_object_height(
    plan: &mut [LayerSpec],
    object_height_mm: f64,
    settings: &SliceSettings,
) {
    if plan.len() < 6 {
        return;
    }
    let last_z = plan.last().unwrap().print_z_mm;
    if (last_z - object_height_mm).abs() < EPSILON {
        return;
    }

    let min_h = settings.min_layer_height_mm.max(0.01);
    let max_h = settings.max_layer_height_mm.max(min_h);
    let start = plan.len() - 5;
    let mut heights: Vec<f64> = plan[start..].iter().map(|s| s.height_mm).collect();
    let mut can_adjust = [true; 5];
    let need_taller = last_z < object_height_mm;
    let mut gap = (last_z - object_height_mm).abs();

    let valid_count = |can_adjust: &[bool; 5]| can_adjust.iter().filter(|b| **b).count();

    while gap > EPSILON {
        let n = valid_count(&can_adjust);
        if n == 0 {
            return;
        }
        let delta = gap / n as f64;
        let mut remain = 0.0;
        for i in 0..5 {
            if !can_adjust[i] {
                continue;
            }
            let h = &mut heights[i];
            if need_taller {
                if (*h - max_h).abs() < EPSILON {
                    remain += delta;
                    can_adjust[i] = false;
                    continue;
                }
                if *h + delta > max_h {
                    remain += *h + delta - max_h;
                    *h = max_h;
                    can_adjust[i] = false;
                } else {
                    *h += delta;
                }
            } else if (*h - min_h).abs() < EPSILON {
                remain += delta;
                can_adjust[i] = false;
            } else if *h - delta < min_h {
                remain += min_h + delta - *h;
                *h = min_h;
                can_adjust[i] = false;
            } else {
                *h -= delta;
            }
        }
        gap = remain;
        if gap < EPSILON {
            break;
        }
    }

    let mut z = plan[start].bottom_z_mm;
    for (i, spec) in plan[start..].iter_mut().enumerate() {
        *spec = LayerSpec::from_range(spec.index, z, z + heights[i]);
        z = spec.print_z_mm;
    }
}

pub fn layer_plan(
    mesh: &TriangleMesh,
    settings: &SliceSettings,
) -> Result<Vec<LayerSpec>, SlicerError> {
    let aabb = mesh.aabb().ok_or(SlicerError::EmptyBounds)?;
    if mesh.indices.is_empty() {
        return Err(SlicerError::EmptyMesh);
    }
    let height = (aabb.max.z - aabb.min.z) as f64;
    let mut plan = generate_object_layers(height, settings);
    let z0 = aabb.min.z as f64;
    if z0.abs() > EPSILON {
        for spec in &mut plan {
            spec.bottom_z_mm += z0;
            spec.slice_z_mm += z0;
            spec.print_z_mm += z0;
        }
    }
    Ok(plan)
}

/// Contour sample heights (slice_z) for the CPU/GPU plane pass.
pub fn layer_z_values(
    mesh: &TriangleMesh,
    settings: &SliceSettings,
) -> Result<Vec<f64>, SlicerError> {
    Ok(layer_plan(mesh, settings)?
        .into_iter()
        .map(|s| s.slice_z_mm)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use bambu_config::SliceSettings;

    #[test]
    fn equal_heights_match_midplane_grid() {
        let settings = SliceSettings::default();
        let plan = generate_object_layers(20.0, &settings);
        assert_eq!(plan.len(), 100);
        assert!((plan[0].bottom_z_mm).abs() < 1e-9);
        assert!((plan[0].slice_z_mm - 0.1).abs() < 1e-9);
        assert!((plan[0].print_z_mm - 0.2).abs() < 1e-9);
        assert!((plan[0].height_mm - 0.2).abs() < 1e-9);
        let last = plan.last().unwrap();
        assert!((last.print_z_mm - 20.0).abs() < 1e-9);
        assert!((last.slice_z_mm - 19.9).abs() < 1e-9);
    }

    #[test]
    fn thicker_first_layer_is_fixed_slab() {
        let mut settings = SliceSettings::default();
        settings.first_layer_height_mm = 0.3;
        settings.layer_height_mm = 0.2;
        let plan = generate_object_layers(20.0, &settings);
        assert!((plan[0].height_mm - 0.3).abs() < 1e-9);
        assert!((plan[0].slice_z_mm - 0.15).abs() < 1e-9);
        assert!((plan[0].print_z_mm - 0.3).abs() < 1e-9);
        assert!((plan[1].height_mm - 0.2).abs() < 1e-9);
        assert!((plan[1].print_z_mm - 0.5).abs() < 1e-9);
        let last = plan.last().unwrap();
        // C++ keeps a slab when mid-plane `slice_z` is below the object, so
        // `print_z` can sit a fraction of a layer under the top.
        assert!(last.slice_z_mm < 20.0);
        assert!(last.print_z_mm < 20.0 + last.height_mm);
        assert!(plan.len() < 100);
    }

    #[test]
    fn raft_uses_normal_height_for_first_object_slab() {
        let mut settings = SliceSettings::default();
        settings.first_layer_height_mm = 0.28;
        settings.layer_height_mm = 0.2;
        settings.raft_layers = 2;
        let plan = generate_object_layers(20.0, &settings);
        assert!((plan[0].height_mm - 0.2).abs() < 1e-9);
        assert!((plan[0].print_z_mm - 0.2).abs() < 1e-9);
        assert!((plan[1].print_z_mm - 0.4).abs() < 1e-9);
    }

    #[test]
    fn precise_z_aligns_last_five_slabs() {
        let mut settings = SliceSettings::default();
        settings.first_layer_height_mm = 0.3;
        settings.layer_height_mm = 0.2;
        settings.precise_z_height = true;
        let plan = generate_object_layers(20.0, &settings);
        let last = plan.last().unwrap();
        assert!(
            (last.print_z_mm - 20.0).abs() < EPSILON,
            "print_z={}",
            last.print_z_mm
        );
        for spec in plan.iter().rev().take(5) {
            assert!(spec.height_mm >= settings.min_layer_height_mm - EPSILON);
            assert!(spec.height_mm <= settings.max_layer_height_mm + EPSILON);
        }
    }
}
