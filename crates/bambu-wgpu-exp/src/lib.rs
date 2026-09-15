//! Opt-in token for wgpu `EXPERIMENTAL_*` features (ray query).
//!
//! wgpu 30 requires [`wgpu::ExperimentalFeatures::enabled`] to request
//! [`wgpu::Features::EXPERIMENTAL_RAY_QUERY`]. That constructor is `unsafe`
//! because the API is still experimental ([wgpu#1040](https://github.com/gfx-rs/wgpu/issues/1040)).
//! First-party crates stay `forbid(unsafe_code)`; this crate owns the token.
//!
//! Report RT validation/runtime bugs upstream to gfx-rs/wgpu.

/// Point ash/wgpu at NixOS's NVIDIA Vulkan ICD before the first `Instance`.
///
/// `/run/opengl-driver/share/vulkan/icd.d/nvidia_icd.json` names
/// `libGLX_nvidia.so.0`. The Khronos loader (`libvulkan.so.1`) is not in that
/// tree until `hardware.graphics.extraPackages` includes `vulkan-loader`.
/// Flake wrap sets `LIB_VULKAN_PATH`; otherwise we pick a loader built against
/// this process's glibc. Existing `VK_ICD_FILENAMES` / `VK_DRIVER_FILES` stay.
pub fn pin_linux_nvidia_vulkan() {
    #[cfg(target_os = "linux")]
    pin_linux_nvidia_vulkan_inner();
}

#[cfg(target_os = "linux")]
fn pin_linux_nvidia_vulkan_inner() {
    const ICD: &str = "/run/opengl-driver/share/vulkan/icd.d/nvidia_icd.json";
    const DRIVER_LIB: &str = "/run/opengl-driver/lib";
    const DRIVER_SHARE: &str = "/run/opengl-driver/share";

    if std::path::Path::new(ICD).is_file() {
        set_if_empty("VK_ICD_FILENAMES", ICD);
        set_if_empty("VK_DRIVER_FILES", ICD);
    }
    let loader_dir = vulkan_loader_libdir();
    prepend_search_path("LD_LIBRARY_PATH", DRIVER_LIB);
    if let Some(dir) = loader_dir {
        prepend_search_path("LD_LIBRARY_PATH", &dir);
    }
    prepend_search_path("XDG_DATA_DIRS", DRIVER_SHARE);
    set_if_empty("WGPU_BACKEND", "vulkan");
}

#[cfg(target_os = "linux")]
fn dir_has_loader(dir: &str) -> bool {
    let p = std::path::Path::new(dir).join("libvulkan.so.1");
    p.exists()
}

/// Directory that contains a `libvulkan.so.1` this process can `dlopen`.
#[cfg(target_os = "linux")]
fn vulkan_loader_libdir() -> Option<String> {
    static CACHED: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    CACHED
        .get_or_init(|| {
            if let Some(dir) = option_env!("BAMBU_VULKAN_LOADER_DIR") {
                if dir_has_loader(dir) {
                    return Some(dir.to_string());
                }
            }
            for dir in ["/run/opengl-driver/lib", "/run/current-system/sw/lib"] {
                if dir_has_loader(dir) {
                    return Some(dir.to_string());
                }
            }
            if let Ok(user) = std::env::var("USER") {
                let dir = format!("/etc/profiles/per-user/{user}/lib");
                if dir_has_loader(&dir) {
                    return Some(dir);
                }
            }
            if let Ok(home) = std::env::var("HOME") {
                let dir = format!("{home}/.nix-profile/lib");
                if dir_has_loader(&dir) {
                    return Some(dir);
                }
            }
            if let Some(dir) = loader_dir_matching_process_glibc() {
                return Some(dir);
            }
            if let Ok(dir) = std::env::var("LIB_VULKAN_PATH") {
                let dir = dir.trim();
                if !dir.is_empty() && dir_has_loader(dir) {
                    return Some(dir.to_string());
                }
            }
            None
        })
        .clone()
}

/// `nix-store --referrers` of this process's glibc: one matching `vulkan-loader`.
#[cfg(target_os = "linux")]
fn loader_dir_matching_process_glibc() -> Option<String> {
    let glibc = glibc_store_path_from_maps()?;
    let mut out = None;
    for bin in ["nix-store", "/run/current-system/sw/bin/nix-store"] {
        if let Ok(o) = std::process::Command::new(bin)
            .args(["-q", "--referrers", &glibc])
            .env_remove("LD_LIBRARY_PATH")
            .output()
        {
            if o.status.success() && !o.stdout.is_empty() {
                out = Some(o);
                break;
            }
        }
    }
    let out = out?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut found = None;
    for line in stdout.lines() {
        let line = line.trim();
        if !line.contains("-vulkan-loader-") {
            continue;
        }
        let name = std::path::Path::new(line)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");
        if name.contains("-dev-") || name.ends_with("-dev") || name.contains("-debug") {
            continue;
        }
        let dir = format!("{line}/lib");
        if dir_has_loader(&dir) {
            found = Some(dir);
        }
    }
    found
}

#[cfg(target_os = "linux")]
fn glibc_store_path_from_maps() -> Option<String> {
    let maps = std::fs::read_to_string("/proc/self/maps").ok()?;
    for line in maps.lines() {
        let Some(idx) = line.find("/nix/store/") else {
            continue;
        };
        let rest = &line[idx..];
        if !(rest.contains("-glibc-") && rest.contains("ld-linux")) {
            continue;
        }
        let path = rest.split_whitespace().next()?;
        let lib_dir = std::path::Path::new(path).parent()?;
        return Some(lib_dir.parent()?.to_string_lossy().into_owned());
    }
    None
}

#[cfg(target_os = "linux")]
fn set_if_empty(key: &str, value: &str) {
    match std::env::var(key) {
        Ok(v) if !v.is_empty() => {}
        _ => {
            // Safety: called from the UI/test thread before wgpu constructs a
            // Vulkan instance; no other threads are racing on these keys.
            unsafe { std::env::set_var(key, value) };
        }
    }
}

#[cfg(target_os = "linux")]
fn prepend_search_path(key: &str, dir: &str) {
    let new = match std::env::var(key) {
        Ok(existing) if existing.split(':').any(|p| p == dir) => return,
        Ok(existing) if !existing.is_empty() => format!("{dir}:{existing}"),
        _ => dir.to_string(),
    };
    // Safety: same contract as [`set_if_empty`].
    unsafe { std::env::set_var(key, new) };
}

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

#[cfg(all(test, target_os = "linux"))]
mod loader_tests {
    #[test]
    fn discovers_khronos_loader_dir() {
        eprintln!(
            "maps_glibc={:?} baked={:?} loader={:?}",
            super::glibc_store_path_from_maps(),
            option_env!("BAMBU_VULKAN_LOADER_DIR"),
            super::vulkan_loader_libdir()
        );
        let dir = super::vulkan_loader_libdir().expect("libvulkan.so.1 dir");
        assert!(
            super::dir_has_loader(&dir),
            "missing libvulkan.so.1 in {dir}"
        );
    }
}
