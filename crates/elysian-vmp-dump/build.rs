fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default() != "linux" {
        return;
    }
    println!("cargo:rerun-if-changed=src/agent.cpp");
    println!("cargo:rerun-if-changed=src/hook.c");
    cc::Build::new()
        .cpp(true)
        .std("c++17")
        .file("src/agent.cpp")
        .compile("vmp_agent");
    println!("cargo:rustc-link-lib=stdc++");

    let manifest = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let hook = out.join("libbambu_vmp_hook.so");
    let cc = std::env::var("CC").unwrap_or_else(|_| "cc".into());
    let status = std::process::Command::new(&cc)
        .arg("-shared")
        .arg("-fPIC")
        .arg("-O2")
        .arg("-D_GNU_SOURCE")
        .arg("-pthread")
        .arg("-o")
        .arg(&hook)
        .arg(manifest.join("src/hook.c"))
        .arg("-ldl")
        .status()
        .expect("compile libbambu_vmp_hook.so");
    if !status.success() {
        panic!("cc failed to build libbambu_vmp_hook.so");
    }
    if let Some(dir) = out.ancestors().nth(3) {
        let dest = dir.join("libbambu_vmp_hook.so");
        let _ = std::fs::copy(&hook, dest);
    }
}
