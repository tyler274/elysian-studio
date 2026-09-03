//! Golden cube: our G-code layer count vs optional C++ Bambu Studio CLI.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use bambu_config::SliceSettings;
use bambu_gcode::{layer_stats, write_gcode};
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
}

#[test]
fn cube_matches_cpp_bambu_studio_when_available() {
    let Some(bin) = find_bambu_studio() else {
        eprintln!("skipping C++ oracle: bambu-studio not on PATH");
        return;
    };

    let dir = std::env::temp_dir().join("bambu-studio-rs-golden");
    let _ = std::fs::create_dir_all(&dir);
    let generated_stl = dir.join("cube_20mm.stl");
    let ours = dir.join("cube_rs.gcode");
    let cpp_dir = dir.join("cpp_out");
    let _ = std::fs::create_dir_all(&cpp_dir);

    write_stl(&generated_stl, &TriangleMesh::cube(20.0)).expect("write stl");

    let mesh = TriangleMesh::cube(20.0);
    let settings = SliceSettings::default();
    let sliced = slice_mesh(&mesh, &settings).unwrap();
    std::fs::write(&ours, write_gcode(&settings, &sliced).unwrap()).unwrap();

    let repo_stl = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/golden/cube_20mm.stl");

    let mut inputs = vec![generated_stl];
    if repo_stl.is_file() {
        inputs.push(repo_stl);
    }

    // Bambu Studio CLI (from --help / BambuStudio.cpp cli_actions):
    //   --slice=0       slice all plates (also accepted as `--slice 0`)
    //   --debug 0       fatal-only logging
    //   --outputdir DIR writes plate_N.gcode into DIR
    // There is no `--export-gcode`.
    let flag_sets: [&[&str]; 2] = [
        &["--slice=0", "--debug", "0", "--outputdir"],
        &["--slice", "0", "--debug", "0", "--outputdir"],
    ];

    let mut last_skip = String::new();
    for input in &inputs {
        for flags in flag_sets {
            match run_cpp_slice(&bin, flags, &cpp_dir, input) {
                Ok(cpp_gcode) => {
                    let ours_gcode = std::fs::read_to_string(&ours).unwrap();
                    let ours_stats = layer_stats(&ours_gcode);
                    let cpp_stats = layer_stats(&cpp_gcode);
                    let cpp_layers = cpp_stats.layer_comments.max(cpp_stats.unique_z);
                    let ours_layers = ours_stats.layer_comments;
                    let delta = (ours_layers as i32 - cpp_layers as i32).unsigned_abs() as usize;
                    assert!(
                        delta <= 10,
                        "layer count diverged: rust={ours_layers} cpp={cpp_layers} ({cpp_stats:?})"
                    );
                    return;
                }
                Err(msg) => last_skip = msg,
            }
        }
    }

    eprintln!("skipping C++ oracle: {last_skip}");
}

fn run_cpp_slice(
    bin: &Path,
    flags: &[&str],
    outdir: &Path,
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
        .args(flags)
        .arg(outdir)
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
            "{} {:?} {} {} failed: {captured}",
            bin.display(),
            flags,
            outdir.display(),
            input.display()
        ));
    }

    let gcode_path = find_gcode(outdir).ok_or_else(|| {
        format!(
            "{} succeeded but no .gcode under {} (STL-only CLI may need a 3MF/profile). {captured}",
            bin.display(),
            outdir.display()
        )
    })?;

    std::fs::read_to_string(&gcode_path).map_err(|err| {
        format!(
            "failed to read {}: {err}. {captured}",
            gcode_path.display()
        )
    })
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
    let _ = Duration::from_secs(1);
    None
}
