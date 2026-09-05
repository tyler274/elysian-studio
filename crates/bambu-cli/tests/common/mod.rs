//! Shared C++ Bambu Studio CLI helpers for golden tests.

use std::path::{Path, PathBuf};
use std::process::Command;

pub fn require_oracle() -> bool {
    matches!(
        std::env::var("BAMBU_STUDIO_REQUIRE_ORACLE").as_deref(),
        Ok("1") | Ok("true") | Ok("TRUE") | Ok("yes")
    )
}

pub fn find_gcode(outdir: &Path) -> Option<PathBuf> {
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
