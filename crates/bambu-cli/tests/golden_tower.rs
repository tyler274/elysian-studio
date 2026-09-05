//! Multifilament test tower: rewrite G-code vs upstream C++ Bambu Studio CLI.
//!
//! The project 3MF embeds P1P 0.28 mm Extra Draft settings, tree supports, a
//! prime tower, and eight filament-mapped parts. The rewrite still emits
//! single-filament G-code, so this compares layer/Z geometry and object FEATURE
//! roles rather than toolchanges or wipe-tower paths.

mod common;

use bambu_alloc as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use bambu_config::SupportType;
use bambu_gcode::{assert_matches_cpp_with, parse_config_comments, parse_gcode, write_gcode};
use bambu_io::load_3mf;
use bambu_model::ModelVolume;
use bambu_slicer::{slice_mesh, slice_volumes};

use common::{find_bambu_studio, find_gcode, require_oracle};

const TOWER_3MF: &str = "Multifilament+advanced+full+test+tower.3mf";

/// Object roles the rewrite should match when the C++ oracle emits them.
/// Prime tower / Flush stay C++-only until multi-material G-code exists.
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
];

fn tower_3mf_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/multicolor")
        .join(TOWER_3MF)
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
    let Some(bin) = find_bambu_studio() else {
        if require_oracle() {
            panic!(
                "BAMBU_STUDIO_REQUIRE_ORACLE=1 but no C++ bambu-studio CLI was found. Set BAMBU_STUDIO or install Bambu Studio."
            );
        }
        eprintln!(
            "skipping C++ oracle: bambu-studio not on PATH (set BAMBU_STUDIO_REQUIRE_ORACLE=1 to fail)"
        );
        return;
    };

    let path = tower_3mf_path();
    let model = load_3mf(&path).expect("load 3mf");
    let settings = model
        .settings
        .clone()
        .expect("embedded project_settings.config");

    let ours_gcode = rust_slice_plate(&model, &settings).expect("rust slice");
    let dir = std::env::temp_dir().join("bambu-studio-rs-oracle-tower");
    let cpp_dir = dir.join("cpp_out");
    let cpp_data = dir.join("cpp_data");
    let _ = std::fs::create_dir_all(&cpp_dir);
    let _ = std::fs::create_dir_all(&cpp_data);
    std::fs::write(dir.join("tower_rs.gcode"), &ours_gcode).unwrap();

    let cpp_gcode = run_cpp_slice_3mf(&bin, &cpp_dir, &cpp_data, &path).unwrap_or_else(|err| {
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

fn rust_slice_plate(
    model: &bambu_model::Model,
    settings: &bambu_config::SliceSettings,
) -> Result<String, String> {
    let mut volumes = model.world_volumes_for_plate(0);
    if volumes.is_empty() {
        return Err("plate 1 has no volumes".into());
    }
    ensure_on_bed_volumes(&mut volumes);
    let sliced = if volumes.iter().any(ModelVolume::needs_volume_slice) {
        slice_volumes(&volumes, settings).map_err(|e| e.to_string())?
    } else {
        let mut mesh = model
            .mesh_for_plate(0)
            .ok_or_else(|| "plate 1 mesh missing".to_string())?;
        ensure_on_bed_mesh(&mut mesh);
        slice_mesh(&mesh, settings).map_err(|e| e.to_string())?
    };
    write_gcode(settings, &sliced).map_err(|e| e.to_string())
}

fn ensure_on_bed_mesh(mesh: &mut bambu_geom::TriangleMesh) {
    if let Some(aabb) = mesh.aabb() {
        if aabb.min.z.abs() > 1e-4 {
            let dz = -aabb.min.z;
            for v in &mut mesh.vertices {
                v.z += dz;
            }
        }
    }
}

fn ensure_on_bed_volumes(volumes: &mut [ModelVolume]) {
    let min_z = volumes
        .iter()
        .filter_map(|v| v.mesh.aabb())
        .map(|a| a.min.z)
        .fold(f32::INFINITY, f32::min);
    if min_z.is_finite() && min_z.abs() > 1e-4 {
        let dz = -min_z;
        for vol in volumes {
            for v in &mut vol.mesh.vertices {
                v.z += dz;
            }
        }
    }
}

fn run_cpp_slice_3mf(
    bin: &Path,
    outdir: &Path,
    datadir: &Path,
    input: &Path,
) -> Result<String, String> {
    let _ = std::fs::create_dir_all(outdir);
    if let Ok(entries) = std::fs::read_dir(outdir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("gcode") {
                let _ = std::fs::remove_file(path);
            }
        }
    }

    let output = Command::new(bin)
        .arg(format!("--datadir={}", datadir.display()))
        .arg("--debug=0")
        .arg("--slice=1")
        .arg(format!("--outputdir={}", outdir.display()))
        .arg("--ensure-on-bed")
        .arg("--no-check")
        .arg(input)
        .output()
        .map_err(|err| format!("failed to spawn {}: {err}", bin.display()))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let captured = format!(
        "status={:?}\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}",
        output.status.code()
    );

    if !output.status.success() {
        return Err(format!("{} --slice=1 failed: {captured}", bin.display()));
    }

    let gcode_path = find_gcode(outdir).ok_or_else(|| {
        format!(
            "{} succeeded but no .gcode under {}. {captured}",
            bin.display(),
            outdir.display()
        )
    })?;

    std::fs::read_to_string(&gcode_path)
        .map_err(|err| format!("failed to read {}: {err}. {captured}", gcode_path.display()))
}
