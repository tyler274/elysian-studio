//! Multifilament test tower: rewrite G-code vs upstream C++ Bambu Studio CLI.
//!
//! The project 3MF embeds P1P 0.28 mm Extra Draft settings, tree supports, a
//! prime tower, and eight filament-mapped parts. The rewrite still emits
//! single-filament G-code, so this compares layer/Z geometry and object FEATURE
//! roles rather than toolchanges or wipe-tower paths.

mod common;

use bambu_alloc as _;
use std::path::PathBuf;

use bambu_config::SupportType;
use bambu_gcode::{assert_matches_cpp_with, parse_config_comments, parse_gcode};
use bambu_io::load_3mf;
use bambu_model::ModelVolume;

use common::{bambu_studio_or_skip, run_cpp_slice_3mf, rust_slice_plate, tests_dir};

const TOWER_3MF: &str = "Multifilament+advanced+full+test+tower.3mf";

/// Object roles the rewrite should match when the C++ oracle emits them.
const TOWER_OBJECT_ROLES: &[&str] = &[
    "Outer wall",
    "Inner wall",
    "Sparse infill",
    "Bottom surface",
    "Top surface",
    "Internal solid infill",
    "Floating vertical shell",
    "Brim",
    "Support",
    "Support interface",
    "Prime tower",
];

fn tower_3mf_path() -> PathBuf {
    tests_dir().join("multicolor").join(TOWER_3MF)
}

#[test]
fn tower_3mf_loads() {
    let path = tower_3mf_path();
    assert!(path.is_file(), "missing {} ({})", TOWER_3MF, path.display());
    let model = load_3mf(&path).expect("load 3mf");
    assert_eq!(model.objects.len(), 1, "expected one assembled object");
    assert_eq!(
        model.objects[0].volumes.len(),
        8,
        "expected blank + 7 colour labels"
    );
    assert!(
        model.objects[0]
            .volumes
            .iter()
            .any(|v| v.config.get("extruder").is_some()),
        "tower parts should carry per-volume extruder ids"
    );
    assert!(
        model.objects[0]
            .volumes
            .iter()
            .any(ModelVolume::needs_volume_slice),
        "different extruders should take the volume-slice path"
    );
    assert_eq!(model.plates.len(), 1);
    assert_eq!(model.plates[0].name, "Advanced Full Tower");
    let settings = model.settings.as_ref().expect("project_settings.config");
    assert!(
        (settings.layer_height_mm - 0.28).abs() < 1e-9,
        "layer_height {}",
        settings.layer_height_mm
    );
    assert!(
        (settings.first_layer_height_mm - 0.2).abs() < 1e-9,
        "first_layer {}",
        settings.first_layer_height_mm
    );
    assert_eq!(settings.wall_loops, 1);
    assert!((settings.infill_density - 0.05).abs() < 1e-9);
    assert!(settings.enable_support);
    assert_eq!(settings.support_type, SupportType::Tree);
    assert!(settings.enable_prime_tower);
    assert!(
        (settings.wipe_tower_x_mm - 15.0).abs() < 0.01,
        "wipe_tower_x {}",
        settings.wipe_tower_x_mm
    );
    assert!(
        (settings.wipe_tower_y_mm - 194.264).abs() < 0.01,
        "wipe_tower_y {}",
        settings.wipe_tower_y_mm
    );
    assert!((settings.prime_tower_width_mm - 35.0).abs() < 1e-9);
    assert_eq!(settings.filament_count, 8);
    assert!(settings.has_wipe_tower());
    let mesh = model.mesh_for_plate(0).expect("plate 1 mesh");
    assert!(
        mesh.indices.len() > 1000,
        "too few triangles: {}",
        mesh.indices.len()
    );
    let aabb = mesh.aabb().expect("aabb");
    assert!(
        aabb.size().z > 50.0,
        "tower height too small: {:?}",
        aabb.size()
    );
}

#[test]
fn tower_matches_cpp_bambu_studio() {
    let Some(bin) = bambu_studio_or_skip() else {
        return;
    };

    let path = tower_3mf_path();
    let model = load_3mf(&path).expect("load 3mf");
    let settings = model
        .settings
        .clone()
        .expect("embedded project_settings.config");

    let ours_gcode = rust_slice_plate(&model, &settings, 0).expect("rust slice");
    let dir = std::env::temp_dir().join("bambu-studio-rs-oracle-tower");
    let cpp_dir = dir.join("cpp_out");
    let cpp_data = dir.join("cpp_data");
    let _ = std::fs::create_dir_all(&cpp_dir);
    let _ = std::fs::create_dir_all(&cpp_data);
    std::fs::write(dir.join("tower_rs.gcode"), &ours_gcode).unwrap();

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
        Some("0.28"),
        "C++ did not keep 3MF process layer_height: {cpp_cfg:?}"
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
        density.contains('5'),
        "C++ sparse_infill_density should be 5%, got {density:?}"
    );
    assert_eq!(
        cpp_cfg.get("enable_support").map(String::as_str),
        Some("1"),
        "C++ enable_support: {cpp_cfg:?}"
    );
    assert!(
        cpp_cfg
            .get("support_type")
            .is_some_and(|s| s.contains("tree")),
        "C++ support_type should be tree: {cpp_cfg:?}"
    );
    assert_eq!(
        cpp_cfg.get("enable_prime_tower").map(String::as_str),
        Some("1"),
        "C++ enable_prime_tower: {cpp_cfg:?}"
    );

    let cpp_layers = cpp
        .total_layer_number
        .map(|n| n as usize)
        .unwrap_or(cpp.layer_changes);
    // Independent support layer height and the C++ wipe tower can add extra
    // CHANGE_LAYER comments vs the rewrite's object-only stack.
    let layer_slop = (cpp_layers / 10).max(15);
    assert_matches_cpp_with(&ours, &cpp, TOWER_OBJECT_ROLES, layer_slop, 0.8);
}
