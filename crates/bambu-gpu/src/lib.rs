//! wgpu / Vulkan viewport and compute slice acceleration.

#![forbid(unsafe_code)]

mod camera;
mod compute;
mod scene;
mod slice_accel;

pub use bambu_preview::ToolpathBuffer;
pub use compute::VulkanSliceAccel;
pub use scene::{OrbitCamera, ViewportEvent, ViewportScene, BED_MM};
pub use slice_accel::{slice_on_vulkan, slice_with_gpu_or_cpu, SliceBackend};

use thiserror::Error;
use wgpu::{Backends, InstanceDescriptor};

#[derive(Debug, Error)]
pub enum GpuError {
    #[error("no Vulkan adapter: {0}")]
    NoAdapter(String),
    #[error("wgpu request: {0}")]
    Request(String),
}

#[derive(Debug, Clone)]
pub struct AdapterReport {
    pub name: String,
    pub backend: String,
    pub is_vulkan: bool,
}

/// Enumerate a Vulkan adapter. On Linux this is required before the iced
/// compositor starts (`WGPU_BACKEND=vulkan`).
pub fn probe_vulkan() -> Result<AdapterReport, GpuError> {
    let instance = wgpu::Instance::new(&InstanceDescriptor {
        backends: Backends::VULKAN,
        ..Default::default()
    });
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        force_fallback_adapter: false,
        compatible_surface: None,
    }))
    .map_err(|e| GpuError::NoAdapter(e.to_string()))?;

    let info = adapter.get_info();
    let backend = format!("{:?}", info.backend);
    let is_vulkan = info.backend == wgpu::Backend::Vulkan;
    Ok(AdapterReport {
        name: info.name,
        backend,
        is_vulkan,
    })
}

pub fn force_vulkan_env() {
    // std::env::set_var is unsafe since Rust 1.87; workspace forbids unsafe.
    // Probe uses Backends::VULKAN. Nix wrapper sets WGPU_BACKEND=vulkan for iced.
    if std::env::var("WGPU_BACKEND").ok().as_deref() != Some("vulkan") {
        tracing::warn!("WGPU_BACKEND is not vulkan; iced may pick a non-Vulkan backend");
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn probe_vulkan_or_skip_without_gpu() {
        match probe_vulkan() {
            Ok(report) => {
                assert!(
                    report.is_vulkan,
                    "expected Vulkan adapter, got {} ({})",
                    report.backend, report.name
                );
            }
            Err(err) => {
                eprintln!("skipping vulkan probe (no GPU / sandbox): {err}");
            }
        }
    }
}
