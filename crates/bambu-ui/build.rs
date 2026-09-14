include!("../bambu-wgpu-exp/vulkan_loader_dir.rs");

fn main() {
    println!("cargo:rerun-if-changed=../bambu-wgpu-exp/vulkan_loader_dir.rs");
    emit_loader_rpath("bins");
}
