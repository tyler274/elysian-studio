//! Request wgpu experimental ray query when the adapter supports it.

/// Adapter ∩ [`wgpu::Features::EXPERIMENTAL_RAY_QUERY`].
pub fn ray_query_features(adapter: &wgpu::Adapter) -> wgpu::Features {
    elysian_wgpu_exp::ray_query_features(adapter)
}

/// Experimental-feature token required to request `EXPERIMENTAL_*` wgpu features.
pub fn experimental_features(
    adapter: &wgpu::Adapter,
) -> wgpu::ExperimentalFeatures {
    if ray_query_features(adapter).is_empty() {
        wgpu::ExperimentalFeatures::disabled()
    } else {
        elysian_wgpu_exp::experimental_features()
    }
}

/// iced bind-group caps, plus acceleration-structure limits when ray query exists.
pub fn device_limits(
    adapter: &wgpu::Adapter,
    base: wgpu::Limits,
) -> wgpu::Limits {
    let limits = wgpu::Limits {
        max_bind_groups: base.max_bind_groups.max(2),
        max_non_sampler_bindings: base.max_non_sampler_bindings.max(2048),
        ..base
    };
    let (_features, _exp, limits) =
        elysian_wgpu_exp::ray_query_device(adapter, limits);
    limits
}
