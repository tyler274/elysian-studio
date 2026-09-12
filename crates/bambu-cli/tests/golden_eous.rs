//! Eous figurine + chassis plates: rewrite G-code vs upstream C++ Bambu Studio.
//!
//! `tests/eous/*.3mf` is H2C 0.20 mm Standard with three plates: the character
//! (tree supports), then two 100% infill chassis objects with per-object
//! process overrides. Plate 1 is ignored by default (210k faces + tree).
//! Chassis plates stay in the default suite.

mod common;

use bambu_alloc as _;

use bambu_config::SupportType;
use bambu_gcode::{assert_matches_cpp_with, parse_config_comments, parse_gcode};
use bambu_io::load_3mf;

use common::{bambu_studio_or_skip, run_cpp_slice_3mf, rust_slice_plate, scene_3mf};

const FIGURINE_ROLES: &[&str] = &[
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

const CHASSIS_ROLES: &[&str] = &[
    "Outer wall",
    "Inner wall",
    "Bottom surface",
    "Top surface",
    "Internal solid infill",
];

#[test]
fn eous_3mf_loads() {
    let path = scene_3mf("eous");
    let model = load_3mf(&path).expect("load eous 3mf");
    assert_eq!(model.objects.len(), 3);
    assert_eq!(model.plates.len(), 3);
    let names: Vec<_> = model.objects.iter().map(|o| o.name.as_str()).collect();
    assert!(
        names.contains(&"伊埃斯.stl")
            && names.contains(&"底盘.stl")
            && names.contains(&"底座2.stl"),
        "object names {names:?}"
    );
    for (i, plate) in model.plates.iter().enumerate() {
        assert_eq!(
            plate.object_indices.len(),
            1,
            "plate {} should hold one object",
            i + 1
        );
    }
    let chassis = model
        .objects
        .iter()
        .find(|o| o.name == "底盘.stl")
        .expect("底盘 object");
    assert!(
        !chassis.volumes.is_empty(),
        "底盘 should have a printable volume"
    );
    assert_eq!(
        chassis.volumes[0]
            .config
            .get("wall_loops")
            .map(String::as_str),
        Some("2"),
        "底盘 object process overrides should land on the volume"
    );
    assert_eq!(
        chassis.volumes[0]
            .config
            .get("sparse_infill_density")
            .map(String::as_str),
        Some("100%")
    );
    assert_eq!(
        chassis.volumes[0]
            .config
            .get("enable_support")
            .map(String::as_str),
        Some("0")
    );
    let settings = model.settings.as_ref().expect("project_settings.config");
    assert!((settings.layer_height_mm - 0.2).abs() < 1e-9);
    assert_eq!(settings.wall_loops, 3);
    assert!((settings.infill_density - 0.15).abs() < 1e-9);
    assert!(settings.enable_support);
    assert_eq!(settings.support_type, SupportType::Tree);
    assert_eq!(settings.top_shell_layers, 5);
    assert_eq!(settings.bottom_shell_layers, 3);
    let fig = model.mesh_for_plate(0).expect("plate 1");
    let faces = fig.indices.len() / 3;
    assert!(
        faces > 50_000,
        "figurine should be a dense mesh, got {faces} faces"
    );
    let chassis_mesh = model.mesh_for_plate(1).expect("plate 2");
    assert!(
        chassis_mesh.indices.len() / 3 > 100,
        "chassis plate should have a mesh"
    );
}

#[test]
#[ignore = "210k-face tree-supported figurine; cargo test -p bambu-cli --test golden_eous -- --ignored"]
fn eous_figurine_matches_cpp_bambu_studio() {
    slice_plate_against_cpp(1, FIGURINE_ROLES, 0.8);
}

#[test]
fn eous_chassis_matches_cpp_bambu_studio() {
    slice_plate_against_cpp(2, CHASSIS_ROLES, 0.8);
}

fn slice_plate_against_cpp(plate: u32, roles: &[&str], z_slop_mm: f64) {
    let Some(bin) = bambu_studio_or_skip() else {
        return;
    };
    let path = scene_3mf("eous");
    let model = load_3mf(&path).expect("load eous 3mf");
    let settings = model
        .settings
        .clone()
        .expect("embedded project_settings.config");
    let ours_gcode = rust_slice_plate(&model, &settings, (plate - 1) as usize).expect("rust slice");
    let dir = std::env::temp_dir().join(format!("bambu-studio-rs-oracle-eous-{plate}"));
    let cpp_dir = dir.join("cpp_out");
    let cpp_data = dir.join("cpp_data");
    let _ = std::fs::create_dir_all(&cpp_dir);
    let _ = std::fs::create_dir_all(&cpp_data);
    std::fs::write(dir.join("eous_rs.gcode"), &ours_gcode).unwrap();
    let cpp_gcode =
        run_cpp_slice_3mf(&bin, &cpp_dir, &cpp_data, &path, plate).unwrap_or_else(|err| {
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
        Some("0.2"),
        "C++ layer_height: {cpp_cfg:?}"
    );
    let cpp_layers = cpp
        .total_layer_number
        .map(|n| n as usize)
        .unwrap_or(cpp.layer_changes);
    let layer_slop = (cpp_layers / 10).max(15);
    assert_matches_cpp_with(&ours, &cpp, roles, layer_slop, z_slop_mm);
}
