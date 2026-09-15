include!("../elysian-wgpu-exp/vulkan_loader_dir.rs");

fn main() {
    println!("cargo:rerun-if-changed=../elysian-wgpu-exp/vulkan_loader_dir.rs");
    emit_loader_rpath("bins");
}
