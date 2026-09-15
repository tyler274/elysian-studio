//! Slice entry point: Vulkan triangle–plane compute with CPU fallback.
//!
//! Clipper / walls / infill remain CPU. Only the mesh/plane step is GPU.

use std::sync::OnceLock;

use bambu_config::SliceSettings;
use bambu_geom::TriangleMesh;
use bambu_model::ModelVolume;
use bambu_slicer::{
    layer_plan, slice_from_contours_with_assist, slice_mesh, slice_volumes,
    slice_volumes_with_planes, zip_plan_contours, GpuAssist, SliceResult, SlicerError,
};

use crate::compute::VulkanSliceAccel;
use crate::GpuError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SliceBackend {
    VulkanCompute,
    Cpu,
}

impl SliceBackend {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::VulkanCompute => "vulkan-compute",
            Self::Cpu => "cpu",
        }
    }
}

impl std::fmt::Display for SliceBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

fn shared_accel() -> Option<&'static VulkanSliceAccel> {
    static CELL: OnceLock<Option<VulkanSliceAccel>> = OnceLock::new();
    CELL.get_or_init(|| match VulkanSliceAccel::new() {
        Ok(accel) => {
            tracing::info!("Vulkan compute slice accelerator ready");
            Some(accel)
        }
        Err(err) => {
            tracing::warn!("Vulkan compute slice unavailable: {err}");
            None
        }
    })
    .as_ref()
}

fn gpu_assist(
    accel: &VulkanSliceAccel,
    mesh: &TriangleMesh,
    zs: &[f64],
    settings: &SliceSettings,
) -> GpuAssist {
    GpuAssist {
        occupancy: if settings.enable_support {
            accel.occupancy_contacts(mesh, zs).unwrap_or_default()
        } else {
            Vec::new()
        },
    }
}

/// Prefer Vulkan plane intersection; fall back to CPU contours.
pub fn slice_with_gpu_or_cpu(
    mesh: &TriangleMesh,
    settings: &SliceSettings,
) -> Result<(SliceResult, SliceBackend), SlicerError> {
    let plan = layer_plan(mesh, settings)?;
    let zs: Vec<f64> = plan.iter().map(|s| s.slice_z_mm).collect();
    if let Some(accel) = shared_accel() {
        match accel.contours_for_layers(mesh, &zs) {
            Ok(layers) => {
                tracing::info!("sliced {} contour layers on Vulkan compute", layers.len());
                let assist = gpu_assist(accel, mesh, &zs, settings);
                return Ok((
                    slice_from_contours_with_assist(
                        zip_plan_contours(&plan, layers),
                        settings,
                        Some(mesh),
                        Some(&assist),
                    ),
                    SliceBackend::VulkanCompute,
                ));
            }
            Err(err) => {
                tracing::warn!("Vulkan compute slice failed ({err}); falling back to CPU");
            }
        }
    }
    Ok((slice_mesh(mesh, settings)?, SliceBackend::Cpu))
}

/// GPU triangle–plane for parts / negatives / paint-color meshes; Clipper stays CPU.
pub fn slice_volumes_with_gpu_or_cpu(
    volumes: &[ModelVolume],
    settings: &SliceSettings,
) -> Result<(SliceResult, SliceBackend), SlicerError> {
    let Some(accel) = shared_accel() else {
        return Ok((slice_volumes(volumes, settings)?, SliceBackend::Cpu));
    };
    let mut used_gpu = false;
    let result = slice_volumes_with_planes(volumes, settings, |mesh, zs| {
        match accel.contours_for_layers(mesh, zs) {
            Ok(layers) => {
                used_gpu = true;
                layers.into_iter().map(|(_, p)| p).collect()
            }
            Err(err) => {
                tracing::warn!("Vulkan volume plane failed ({err}); CPU plane for one mesh");
                zs.iter()
                    .map(|&z| bambu_geom::union_polygons(&bambu_slicer::slice_at_z(mesh, z as f32)))
                    .collect()
            }
        }
    })?;
    let backend = if used_gpu {
        SliceBackend::VulkanCompute
    } else {
        SliceBackend::Cpu
    };
    Ok((result, backend))
}

/// Require Vulkan compute; do not fall back.
pub fn slice_on_vulkan(
    mesh: &TriangleMesh,
    settings: &SliceSettings,
) -> Result<SliceResult, GpuError> {
    let plan = layer_plan(mesh, settings).map_err(|e| GpuError::Request(e.to_string()))?;
    let zs: Vec<f64> = plan.iter().map(|s| s.slice_z_mm).collect();
    let accel = VulkanSliceAccel::new()?;
    let layers = accel.contours_for_layers(mesh, &zs)?;
    let assist = gpu_assist(&accel, mesh, &zs, settings);
    Ok(slice_from_contours_with_assist(
        zip_plan_contours(&plan, layers),
        settings,
        Some(mesh),
        Some(&assist),
    ))
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use bambu_geom::Point;
    use bambu_slicer::{slice_from_contours, zip_plan_contours};

    #[test]
    fn gpu_cube_layer_count_matches_cpu_when_adapter_exists() {
        let Ok(accel) = VulkanSliceAccel::new() else {
            eprintln!("skipping GPU slice test (no Vulkan adapter)");
            return;
        };
        let mesh = TriangleMesh::cube(20.0);
        let settings = SliceSettings::default();
        let plan = bambu_slicer::layer_plan(&mesh, &settings).unwrap();
        let zs: Vec<f64> = plan.iter().map(|s| s.slice_z_mm).collect();
        let gpu_layers = accel.contours_for_layers(&mesh, &zs).unwrap();
        let gpu = slice_from_contours(zip_plan_contours(&plan, gpu_layers), &settings, Some(&mesh));
        let cpu = slice_mesh(&mesh, &settings).unwrap();
        let delta = gpu.layers.len().abs_diff(cpu.layers.len());
        assert!(
            delta <= 2,
            "gpu layers={} cpu layers={}",
            gpu.layers.len(),
            cpu.layers.len()
        );
        assert!(!gpu.layers.is_empty());
        assert!(gpu.layers[gpu.layers.len() / 2]
            .perimeters()
            .next()
            .is_some());
    }

    #[test]
    fn gpu_occupancy_and_gyroid_skip_without_adapter() {
        let Ok(accel) = VulkanSliceAccel::new() else {
            eprintln!("skipping GPU occupancy/gyroid test (no Vulkan adapter)");
            return;
        };
        let mesh = TriangleMesh::cube(20.0);
        let zs = [0.1, 10.0, 19.9];
        let occ = accel.occupancy_contacts(&mesh, &zs).unwrap();
        assert_eq!(occ.len(), 3);
        let square = vec![vec![
            Point::from_mm(0.0, 0.0),
            Point::from_mm(20.0, 0.0),
            Point::from_mm(20.0, 20.0),
            Point::from_mm(0.0, 20.0),
        ]];
        let gyroid = accel.gyroid_polylines(&square, 2.0, 0.2, 10.0).unwrap();
        assert!(!gyroid.is_empty(), "GPU gyroid should clip some iso lines");
        let adaptive = accel.adaptive_polylines(&square, 4.0, 10.0).unwrap();
        assert!(!adaptive.is_empty() || gyroid.len() < 1000);
    }
}
