//! Remielle 4-colour lithophane: rewrite G-code vs upstream C++ Bambu Studio.
//!
//! `tests/remielle/*.3mf` is H2C 0.08 mm High Quality, 100% zig-zag fill, four
//! PETG colour volumes plus a board. The rewrite still emits single-filament
//! G-code, so the oracle compares layer/Z geometry and object FEATURE roles.
//! The C++ compare is ignored by default: 848k faces at 0.08 mm is too heavy
//! for every workspace run (`--ignored` to opt in).

mod common;

use bambu_alloc as _;

use bambu_config::SupportType;
use bambu_gcode::{assert_matches_cpp_with, parse_config_comments, parse_gcode};
use bambu_io::load_3mf;

use common::{bambu_studio_or_skip, run_cpp_slice_3mf, rust_slice_plate, scene_3mf};

const OBJECT_ROLES: &[&str] = &[
    "Outer wall",
    "Bottom surface",
    "Top surface",
    "Internal solid infill",
    "Brim",
];

#[test]
fn remielle_3mf_loads() {
    let path = scene_3mf("remielle");
    let model = load_3mf(&path).expect("load remielle 3mf");
    assert_eq!(model.objects.len(), 1, "one assembled colour object");
    assert_eq!(model.plates.len(), 1);
    let names: Vec<_> = model.objects[0]
        .volumes
        .iter()
        .map(|v| v.name.as_str())
        .collect();
    assert_eq!(names, ["White", "Board", "Red", "Yellow", "Blue"]);
    let extruders: Vec<_> = model.objects[0]
        .volumes
        .iter()
        .map(|v| v.config.get("extruder").map(String::as_str))
        .collect();
    assert_eq!(
        extruders,
        [Some("1"), Some("1"), Some("2"), Some("3"), Some("4")]
    );
    let settings = model.settings.as_ref().expect("project_settings.config");
    assert!(
        (settings.layer_height_mm - 0.08).abs() < 1e-9,
        "layer_height {}",
        settings.layer_height_mm
    );
    assert!(
        (settings.first_layer_height_mm - 0.08).abs() < 1e-9,
        "first_layer {}",
        settings.first_layer_height_mm
    );
    assert_eq!(settings.wall_loops, 1);
    assert!((settings.infill_density - 1.0).abs() < 1e-9);
    assert_eq!(settings.top_shell_layers, 0);
    assert_eq!(settings.bottom_shell_layers, 1);
    assert!(!settings.enable_support);
    assert_eq!(settings.support_type, SupportType::Tree);
    assert!(settings.enable_prime_tower);
    let mesh = model.mesh_for_plate(0).expect("plate 1 mesh");
    let faces = mesh.indices.len() / 3;
    assert!(
        faces > 100_000,
        "lithophane should be a dense colour mesh, got {faces} faces"
    );
    let aabb = mesh.aabb().expect("aabb");
    assert!(
        aabb.size().x > 50.0 && aabb.size().y > 50.0,
        "XY too small: {:?}",
        aabb.size()
    );
}

#[test]
#[ignore = "848k-face 0.08 mm colour plate; cargo test -p bambu-cli --test golden_remielle -- --ignored"]
fn remielle_matches_cpp_bambu_studio() {
    let Some(bin) = bambu_studio_or_skip() else {
        return;
    };
    let path = scene_3mf("remielle");
    let model = load_3mf(&path).expect("load remielle 3mf");
    let settings = model
        .settings
        .clone()
        .expect("embedded project_settings.config");
    let ours_gcode = rust_slice_plate(&model, &settings, 0).expect("rust slice");
    let dir = std::env::temp_dir().join("bambu-studio-rs-oracle-remielle");
    let cpp_dir = dir.join("cpp_out");
    let cpp_data = dir.join("cpp_data");
    let _ = std::fs::create_dir_all(&cpp_dir);
    let _ = std::fs::create_dir_all(&cpp_data);
    std::fs::write(dir.join("remielle_rs.gcode"), &ours_gcode).unwrap();
    let cpp_gcode = run_cpp_slice_3mf(&bin, &cpp_dir, &cpp_data, &path, 1).unwrap_or_else(|err| {
        panic!(
            "C++ Bambu Studio oracle failed using {}:\n{err}",
            bin.display()
        )
    });
    let ours = parse_gcode(&ours_gcode);
    let cpp = parse_gcode(&cpp_gcode);
    let cpp_cfg = parse_config_comments(&cpp_gcode);
    assert_eq!(
        cpp_cfg.get("layer_height").map(String::as_str),
        Some("0.08"),
        "C++ layer_height: {cpp_cfg:?}"
    );
    assert_eq!(
        cpp_cfg.get("wall_loops").map(String::as_str),
        Some("1"),
        "C++ wall_loops: {cpp_cfg:?}"
    );
    let density = cpp_cfg
        .get("sparse_infill_density")
        .map(String::as_str)
        .unwrap_or("");
    assert!(
        density.contains("100"),
        "C++ sparse_infill_density should be 100%, got {density:?}"
    );
    let cpp_layers = cpp
        .total_layer_number
        .map(|n| n as usize)
        .unwrap_or(cpp.layer_changes);
    let layer_slop = (cpp_layers / 10).max(8);
    assert_matches_cpp_with(&ours, &cpp, OBJECT_ROLES, layer_slop, 0.8);
}
