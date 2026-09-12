//! Calibration torture block: rewrite G-code vs upstream C++ Bambu Studio.
//!
//! `tests/calibration_block/*.3mf` is H2C 0.16 mm Standard with thirteen
//! bodies on one plate (tree supports, 15% grid). The rewrite still emits
//! single-filament G-code, so the oracle compares layer/Z geometry and object
//! FEATURE roles rather than toolchanges. The C++ compare is ignored by default:
//! the Studio CLI exits `-101` (`CLI_GCODE_PATH_CONFLICTS`) on this plate's
//! wipe tower.

mod common;

use bambu_alloc as _;

use bambu_config::SupportType;
use bambu_gcode::{assert_matches_cpp_with, parse_config_comments, parse_gcode};
use bambu_io::load_3mf;

use common::{bambu_studio_or_skip, run_cpp_slice_3mf, rust_slice_plate, scene_3mf};

const OBJECT_ROLES: &[&str] = &[
    "Outer wall",
    "Inner wall",
    "Sparse infill",
    "Bottom surface",
    "Top surface",
    "Internal solid infill",
    "Brim",
    "Support",
    "Support interface",
];

#[test]
fn calibration_block_3mf_loads() {
    let path = scene_3mf("calibration_block");
    let model = load_3mf(&path).expect("load calibration block 3mf");
    assert_eq!(model.objects.len(), 13, "thirteen Körper parts");
    assert_eq!(model.plates.len(), 1);
    assert_eq!(
        model.plates[0].object_indices.len(),
        13,
        "all bodies on plate 1"
    );
    assert!(
        model.objects.iter().all(|o| o.name.starts_with("Körper")),
        "unexpected names: {:?}",
        model
            .objects
            .iter()
            .map(|o| o.name.as_str())
            .collect::<Vec<_>>()
    );
    assert!(
        model.objects.iter().flat_map(|o| &o.volumes).any(|v| v
            .config
            .get("extruder")
            .map(String::as_str)
            == Some("6")),
        "bodies should carry extruder 6"
    );
    let settings = model.settings.as_ref().expect("project_settings.config");
    assert!(
        (settings.layer_height_mm - 0.16).abs() < 1e-9,
        "layer_height {}",
        settings.layer_height_mm
    );
    assert!(
        (settings.first_layer_height_mm - 0.2).abs() < 1e-9,
        "first_layer {}",
        settings.first_layer_height_mm
    );
    assert_eq!(settings.wall_loops, 2);
    assert!((settings.infill_density - 0.15).abs() < 1e-9);
    assert_eq!(settings.top_shell_layers, 6);
    assert_eq!(settings.bottom_shell_layers, 4);
    assert!(settings.enable_support);
    assert_eq!(settings.support_type, SupportType::Tree);
    assert!(settings.enable_prime_tower);
    let mesh = model.mesh_for_plate(0).expect("plate 1 mesh");
    let faces = mesh.indices.len() / 3;
    assert!(
        faces > 1000,
        "calibration block should have real geometry, got {faces} faces"
    );
}

#[test]
#[ignore = "C++ CLI exits -101 (wipe-tower G-code path conflicts); cargo test -p bambu-cli --test golden_calibration -- --ignored"]
fn calibration_block_matches_cpp_bambu_studio() {
    let Some(bin) = bambu_studio_or_skip() else {
        return;
    };
    let path = scene_3mf("calibration_block");
    let model = load_3mf(&path).expect("load calibration block 3mf");
    let settings = model
        .settings
        .clone()
        .expect("embedded project_settings.config");
    let ours_gcode = rust_slice_plate(&model, &settings, 0).expect("rust slice");
    let dir = std::env::temp_dir().join("bambu-studio-rs-oracle-calibration");
    let cpp_dir = dir.join("cpp_out");
    let cpp_data = dir.join("cpp_data");
    let _ = std::fs::create_dir_all(&cpp_dir);
    let _ = std::fs::create_dir_all(&cpp_data);
    std::fs::write(dir.join("calibration_rs.gcode"), &ours_gcode).unwrap();
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
        Some("0.16"),
        "C++ layer_height: {cpp_cfg:?}"
    );
    assert_eq!(
        cpp_cfg.get("wall_loops").map(String::as_str),
        Some("2"),
        "C++ wall_loops: {cpp_cfg:?}"
    );
    let density = cpp_cfg
        .get("sparse_infill_density")
        .map(String::as_str)
        .unwrap_or("");
    assert!(
        density.contains("15"),
        "C++ sparse_infill_density should be 15%, got {density:?}"
    );
    assert_eq!(
        cpp_cfg.get("enable_support").map(String::as_str),
        Some("1"),
        "C++ enable_support: {cpp_cfg:?}"
    );
    let cpp_layers = cpp
        .total_layer_number
        .map(|n| n as usize)
        .unwrap_or(cpp.layer_changes);
    let layer_slop = (cpp_layers / 10).max(15);
    assert_matches_cpp_with(&ours, &cpp, OBJECT_ROLES, layer_slop, 0.8);
}
