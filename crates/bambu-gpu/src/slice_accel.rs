//! Slice entry point: Vulkan triangle–plane compute with CPU fallback.
//!
//! Clipper / walls / infill remain CPU. Only the mesh/plane step is GPU.

use std::sync::OnceLock;

use bambu_config::SliceSettings;
use bambu_geom::TriangleMesh;
use bambu_slicer::{
    layer_plan, slice_from_contours, slice_mesh, zip_plan_contours, SliceResult, SlicerError,
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
                return Ok((
                    slice_from_contours(zip_plan_contours(&plan, layers), settings),
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

/// Require Vulkan compute; do not fall back.
pub fn slice_on_vulkan(
    mesh: &TriangleMesh,
    settings: &SliceSettings,
) -> Result<SliceResult, GpuError> {
    let plan = layer_plan(mesh, settings).map_err(|e| GpuError::Request(e.to_string()))?;
    let zs: Vec<f64> = plan.iter().map(|s| s.slice_z_mm).collect();
    let accel = VulkanSliceAccel::new()?;
    let layers = accel.contours_for_layers(mesh, &zs)?;
    Ok(slice_from_contours(
        zip_plan_contours(&plan, layers),
        settings,
    ))
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use bambu_config::SliceSettings;
    use bambu_geom::TriangleMesh;

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
        let gpu = slice_from_contours(zip_plan_contours(&plan, gpu_layers), &settings);
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
}
