//! Bake a glibc-matched `libvulkan.so.1` rpath into dependents.

include!("vulkan_loader_dir.rs");

fn main() {
    println!("cargo:rerun-if-changed=vulkan_loader_dir.rs");
    emit_loader_rpath("lib");
}
