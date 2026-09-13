//! Opt-in token for wgpu `EXPERIMENTAL_*` features (ray query).
//!
//! wgpu 27 requires [`wgpu::ExperimentalFeatures::enabled`] to request
//! [`wgpu::Features::EXPERIMENTAL_RAY_QUERY`]. That constructor is `unsafe`
//! because the API is still experimental ([wgpu#1040](https://github.com/gfx-rs/wgpu/issues/1040)).
//! First-party crates stay `forbid(unsafe_code)`; this crate owns the token.
//!
//! Report RT validation/runtime bugs upstream to gfx-rs/wgpu.

/// Acknowledge wgpu's experimental RT contract (ray query / BLAS / TLAS).
pub fn experimental_features() -> wgpu::ExperimentalFeatures {
    // Safety: callers only enable `EXPERIMENTAL_RAY_QUERY` (and documented
    // follow-ons) on adapters that report the feature. We accept wgpu's
    // experimental-API contract and report bugs upstream.
    unsafe { wgpu::ExperimentalFeatures::enabled() }
}

/// Adapter ∩ ray-query feature. Empty if the GPU has no hardware RT.
pub fn ray_query_features(adapter: &wgpu::Adapter) -> wgpu::Features {
    adapter.features() & wgpu::Features::EXPERIMENTAL_RAY_QUERY
}

/// Device descriptor extras: features, experimental token, AS limits.
pub fn ray_query_device(
    adapter: &wgpu::Adapter,
    mut limits: wgpu::Limits,
) -> (wgpu::Features, wgpu::ExperimentalFeatures, wgpu::Limits) {
    let features = ray_query_features(adapter);
    if features.is_empty() {
        (
            wgpu::Features::empty(),
            wgpu::ExperimentalFeatures::disabled(),
            limits,
        )
    } else {
        limits = limits.using_acceleration_structure_values(adapter.limits());
        (features, experimental_features(), limits)
    }
}

/// True when this device was created with hardware ray query.
pub fn device_has_ray_query(device: &wgpu::Device) -> bool {
    device
        .features()
        .contains(wgpu::Features::EXPERIMENTAL_RAY_QUERY)
}
