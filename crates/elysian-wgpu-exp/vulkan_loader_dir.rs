// Shared by `build.rs` files: find a `libvulkan.so.1` matching this glibc.

use std::path::{Path, PathBuf};
use std::process::Command;

fn vulkan_loader_has_lib(dir: &Path) -> bool {
    dir.join("libvulkan.so.1").exists()
}

fn discover_vulkan_loader_dir() -> Option<String> {
    if let Ok(dir) = std::env::var("LIB_VULKAN_PATH") {
        let dir = dir.trim();
        if !dir.is_empty() && vulkan_loader_has_lib(Path::new(dir)) {
            return Some(dir.to_string());
        }
    }
    let opengl = Path::new("/run/opengl-driver/lib");
    if vulkan_loader_has_lib(opengl) {
        return Some(opengl.to_string_lossy().into_owned());
    }
    loader_matching_process_glibc()
}

fn loader_matching_process_glibc() -> Option<String> {
    let glibc = glibc_from_maps().or_else(rustc_glibc_store_path)?;
    let mut out = None;
    for bin in [
        "nix-store",
        "/run/current-system/sw/bin/nix-store",
        "/usr/bin/nix-store",
    ] {
        if let Ok(o) = Command::new(bin)
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
        let name = Path::new(line).file_name()?.to_str()?;
        if name.contains("-dev-") || name.ends_with("-dev") || name.contains("-debug") {
            continue;
        }
        let dir = PathBuf::from(line).join("lib");
        if vulkan_loader_has_lib(&dir) {
            found = Some(dir.to_string_lossy().into_owned());
        }
    }
    found
}

fn glibc_from_maps() -> Option<String> {
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
        let lib_dir = Path::new(path).parent()?;
        return Some(lib_dir.parent()?.to_string_lossy().into_owned());
    }
    None
}

fn rustc_glibc_store_path() -> Option<String> {
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let out = Command::new("ldd").arg(rustc).output().ok()?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    for line in stdout.lines() {
        if !line.contains("libc.so.6") {
            continue;
        }
        let Some(idx) = line.find("/nix/store/") else {
            continue;
        };
        let path = line[idx..].split_whitespace().next()?;
        let lib_dir = Path::new(path).parent()?;
        return Some(lib_dir.parent()?.to_string_lossy().into_owned());
    }
    None
}

fn emit_loader_rpath(kind: &str) {
    println!("cargo:rerun-if-env-changed=LIB_VULKAN_PATH");
    println!("cargo:rerun-if-changed=/run/opengl-driver/lib/libvulkan.so.1");
    let Some(dir) = discover_vulkan_loader_dir() else {
        println!("cargo:warning=no glibc-matched libvulkan.so.1 (set LIB_VULKAN_PATH)");
        return;
    };
    println!("cargo:rustc-env=BAMBU_VULKAN_LOADER_DIR={dir}");
    println!("cargo:rustc-link-search=native={dir}");
    println!("cargo:rustc-link-lib=dylib=vulkan");
    println!("cargo:rustc-link-arg=-Wl,-rpath,{dir}");
    if kind == "bins" {
        println!("cargo:rustc-link-arg-bins=-Wl,-rpath,{dir}");
    }
}
