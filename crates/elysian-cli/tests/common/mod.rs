//! Shared C++ Bambu Studio CLI helpers for golden tests.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;

use bambu_config::SliceSettings;
use bambu_gcode::write_gcode;
use bambu_geom::TriangleMesh;
use bambu_model::{Model, ModelVolume};
use bambu_slicer::{slice_mesh, slice_volumes};

pub fn require_oracle() -> bool {
    matches!(
        std::env::var("BAMBU_STUDIO_REQUIRE_ORACLE").as_deref(),
        Ok("1") | Ok("true") | Ok("TRUE") | Ok("yes")
    )
}

pub fn tests_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests")
}

/// The sole `.3mf` in `tests/<dir>/`.
pub fn scene_3mf(dir: &str) -> PathBuf {
    let folder = tests_dir().join(dir);
    let mut found: Vec<_> = std::fs::read_dir(&folder)
        .unwrap_or_else(|err| panic!("read {}: {err}", folder.display()))
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("3mf"))
        .collect();
    found.sort();
    assert_eq!(
        found.len(),
        1,
        "expected one .3mf in {} (found {found:?})",
        folder.display()
    );
    found.remove(0)
}

pub fn find_gcode(outdir: &Path) -> Option<PathBuf> {
    find_gcode_for_plate(outdir, 1)
}

pub fn find_gcode_for_plate(outdir: &Path, plate: u32) -> Option<PathBuf> {
    let preferred = outdir.join(format!("plate_{plate}.gcode"));
    if preferred.is_file() {
        return Some(preferred);
    }
    let entries = std::fs::read_dir(outdir).ok()?;
    entries
        .flatten()
        .map(|e| e.path())
        .find(|p| p.extension().and_then(|e| e.to_str()) == Some("gcode"))
}

pub fn find_bambu_studio() -> Option<PathBuf> {
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

pub fn bambu_studio_or_skip() -> Option<PathBuf> {
    match find_bambu_studio() {
        Some(bin) => Some(bin),
        None => {
            if require_oracle() {
                panic!(
                    "BAMBU_STUDIO_REQUIRE_ORACLE=1 but no C++ bambu-studio CLI was found. Set BAMBU_STUDIO or install Bambu Studio."
                );
            }
            eprintln!(
                "skipping C++ oracle: bambu-studio not on PATH (set BAMBU_STUDIO_REQUIRE_ORACLE=1 to fail)"
            );
            None
        }
    }
}

pub fn rust_slice_plate(
    model: &Model,
    settings: &SliceSettings,
    plate: usize,
) -> Result<String, String> {
    let plate_n = plate + 1;
    let mut volumes = model.world_volumes_for_plate(plate);
    if volumes.is_empty() {
        return Err(format!("plate {plate_n} has no volumes"));
    }
    ensure_on_bed_volumes(&mut volumes);
    let object_settings = bambu_model::agreed_object_settings(&volumes, settings);
    let sliced = if volumes.iter().any(ModelVolume::needs_volume_slice) {
        slice_volumes(&volumes, settings).map_err(|e| e.to_string())?
    } else {
        let mut mesh = model
            .mesh_for_plate(plate)
            .ok_or_else(|| format!("plate {plate_n} mesh missing"))?;
        ensure_on_bed_mesh(&mut mesh);
        slice_mesh(&mesh, &object_settings).map_err(|e| e.to_string())?
    };
    write_gcode(settings, &sliced).map_err(|e| e.to_string())
}

pub fn ensure_on_bed_mesh(mesh: &mut TriangleMesh) {
    if let Some(aabb) = mesh.aabb() {
        if aabb.min.z.abs() > 1e-4 {
            let dz = -aabb.min.z;
            for v in &mut mesh.vertices {
                v.z += dz;
            }
        }
    }
}

pub fn ensure_on_bed_volumes(volumes: &mut [ModelVolume]) {
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

pub fn run_cpp_slice_3mf(
    bin: &Path,
    outdir: &Path,
    datadir: &Path,
    input: &Path,
    plate: u32,
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
        .arg(format!("--slice={plate}"))
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
        return Err(format!(
            "{} --slice={plate} failed: {captured}",
            bin.display()
        ));
    }

    let gcode_path = find_gcode_for_plate(outdir, plate).ok_or_else(|| {
        format!(
            "{} succeeded but no .gcode under {}. {captured}",
            bin.display(),
            outdir.display()
        )
    })?;

    std::fs::read_to_string(&gcode_path)
        .map_err(|err| format!("failed to read {}: {err}. {captured}", gcode_path.display()))
}
