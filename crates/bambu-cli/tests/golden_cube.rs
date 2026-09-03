//! Golden cube: rewrite G-code vs upstream C++ Bambu Studio CLI.

use std::path::{Path, PathBuf};
use std::process::Command;

use bambu_config::{
    bbl_oracle_paths, load_bbl_process, write_flattened_bbl_profile, SliceSettings,
};
use bambu_gcode::{
    assert_matches_cpp, layer_stats, parse_config_comments, parse_gcode, write_gcode,
};
use bambu_geom::TriangleMesh;
use bambu_io::write_stl;
use bambu_slicer::slice_mesh;

#[test]
fn cube_gcode_layer_count() {
    let mesh = TriangleMesh::cube(20.0);
    let settings = SliceSettings::default();
    let sliced = slice_mesh(&mesh, &settings).expect("slice");
    let gcode = write_gcode(&settings, &sliced).expect("gcode");
    let stats = layer_stats(&gcode);
    assert!(
        (90..=105).contains(&stats.layer_comments),
        "unexpected layer comments: {stats:?}\n{}",
        &gcode[..gcode.len().min(800)]
    );
    assert!(gcode.contains("G1 X"), "missing extrusion moves");
    assert!(gcode.contains("; CHANGE_LAYER"));
}

#[test]
fn cube_matches_cpp_bambu_studio() {
    let Some(bin) = find_bambu_studio() else {
        if require_oracle() {
            panic!(
                "BAMBU_STUDIO_REQUIRE_ORACLE=1 but no C++ bambu-studio CLI was found. Set BAMBU_STUDIO or install Bambu Studio."
            );
        }
        eprintln!("skipping C++ oracle: bambu-studio not on PATH (set BAMBU_STUDIO_REQUIRE_ORACLE=1 to fail)");
        return;
    };

    let profiles = bbl_oracle_paths().unwrap_or_else(|| {
        panic!(
            "upstream BambuStudio profiles not found; set BAMBU_STUDIO_RESOURCES or keep ../BambuStudio checked out"
        );
    });

    let dir = std::env::temp_dir().join("bambu-studio-rs-oracle");
    let cpp_dir = dir.join("cpp_out");
    let cpp_data = dir.join("cpp_data");
    let flat_dir = dir.join("flat");
    let _ = std::fs::create_dir_all(&cpp_dir);
    let _ = std::fs::create_dir_all(&cpp_data);
    let _ = std::fs::create_dir_all(&flat_dir);

    let stl = dir.join("cube_20mm.stl");
    write_stl(&stl, &TriangleMesh::cube(20.0)).expect("write stl");

    let process_flat = flat_dir.join("process.json");
    let machine_flat = flat_dir.join("machine.json");
    let filament_flat = flat_dir.join("filament.json");
    write_flattened_bbl_profile(&profiles.process, &process_flat).expect("flatten process");
    write_flattened_bbl_profile(&profiles.machine, &machine_flat).expect("flatten machine");
    write_flattened_bbl_profile(&profiles.filament, &filament_flat).expect("flatten filament");

    let settings = load_bbl_process(&profiles.process).expect("load BBL process");
    assert_eq!(settings.wall_loops, 2);
    assert!((settings.infill_density - 0.15).abs() < 1e-9);
    let sliced = slice_mesh(&TriangleMesh::cube(20.0), &settings).expect("rust slice");
    let ours_gcode = write_gcode(&settings, &sliced).expect("rust gcode");
    std::fs::write(dir.join("cube_rs.gcode"), &ours_gcode).unwrap();

    let cpp_gcode = run_cpp_slice(
        &bin,
        &cpp_dir,
        &cpp_data,
        &stl,
        &machine_flat,
        &process_flat,
        &filament_flat,
    )
    .unwrap_or_else(|err| {
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
        "C++ did not apply flattened BBL process (layer_height): {cpp_cfg:?}"
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
        "C++ sparse_infill_density should be 15% after flatten, got {density:?}"
    );
    assert_eq!(
        cpp_cfg.get("top_shell_layers").map(String::as_str),
        Some("5"),
        "C++ top_shell_layers: {cpp_cfg:?}"
    );
    assert_eq!(
        cpp_cfg.get("skirt_loops").map(String::as_str),
        Some("0"),
        "C++ skirt_loops: {cpp_cfg:?}"
    );

    assert_matches_cpp(&ours, &cpp);
    assert!(
        ours.features.contains("Brim"),
        "rewrite missing Brim under BBL 0.20: {:?}",
        ours.features
    );
}

fn require_oracle() -> bool {
    matches!(
        std::env::var("BAMBU_STUDIO_REQUIRE_ORACLE").as_deref(),
        Ok("1") | Ok("true") | Ok("TRUE") | Ok("yes")
    )
}

fn run_cpp_slice(
    bin: &Path,
    outdir: &Path,
    datadir: &Path,
    input: &Path,
    machine: &Path,
    process: &Path,
    filament: &Path,
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

    let load_settings = format!("{};{}", machine.display(), process.display());
    let output = Command::new(bin)
        .arg(format!("--datadir={}", datadir.display()))
        .arg("--debug=0")
        .arg("--slice=0")
        .arg(format!("--outputdir={}", outdir.display()))
        .arg("--load-settings")
        .arg(&load_settings)
        .arg("--load-filaments")
        .arg(filament.as_os_str())
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
        return Err(format!("{} --slice=0 failed: {captured}", bin.display()));
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

fn find_gcode(outdir: &Path) -> Option<PathBuf> {
    let preferred = outdir.join("plate_1.gcode");
    if preferred.is_file() {
        return Some(preferred);
    }
    let entries = std::fs::read_dir(outdir).ok()?;
    entries
        .flatten()
        .map(|e| e.path())
        .find(|p| p.extension().and_then(|e| e.to_str()) == Some("gcode"))
}

fn find_bambu_studio() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("BAMBU_STUDIO") {
        let path = PathBuf::from(p);
        if path.is_file() {
            return Some(path);
        }
    }
    for name in ["bambu-studio", "BambuStudio", "bambu-studio-bin"] {
        if let Ok(out) = Command::new("which").arg(name).output() {
            if out.status.success() {
                let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !path.is_empty() {
                    return Some(PathBuf::from(path));
                }
            }
        }
    }
    let home = PathBuf::from("/home/luluco/code/BambuStudio");
    for rel in [
        "build/src/bambu-studio",
        "build/src/BambuStudio",
        "build-release/src/bambu-studio",
    ] {
        let candidate = home.join(rel);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}
