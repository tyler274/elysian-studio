//! Locate upstream `BambuStudio/resources` for oracle profiles.

use std::path::PathBuf;

pub fn bbl_resources_dir() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("BAMBU_STUDIO_RESOURCES") {
        let path = PathBuf::from(p);
        if path.is_dir() {
            return Some(path);
        }
    }
    let candidates = [
        PathBuf::from("/home/luluco/code/BambuStudio/resources"),
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../BambuStudio/resources"),
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../BambuStudio/resources"),
    ];
    candidates.into_iter().find(|p| p.is_dir())
}

/// Upstream machine + process + filament used by the C++ CLI oracle.
#[derive(Debug, Clone)]
pub struct BblOraclePaths {
    pub process: PathBuf,
    pub machine: PathBuf,
    pub filament: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BblProfileKind {
    Process,
    Filament,
    Machine,
}

impl BblProfileKind {
    pub fn dir_name(self) -> &'static str {
        match self {
            Self::Process => "process",
            Self::Filament => "filament",
            Self::Machine => "machine",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BblProfileEntry {
    pub name: String,
    pub path: PathBuf,
}

/// JSON files under `profiles/BBL/{process,filament,machine}`.
pub fn list_bbl_profiles(kind: BblProfileKind) -> Vec<BblProfileEntry> {
    let Some(root) = bbl_resources_dir() else {
        return Vec::new();
    };
    let dir = root.join("profiles/BBL").join(kind.dir_name());
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out: Vec<BblProfileEntry> = entries
        .flatten()
        .filter(|e| {
            e.path()
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
        })
        .map(|e| {
            let path = e.path();
            let name = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("profile")
                .to_string();
            BblProfileEntry { name, path }
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

pub fn bbl_oracle_paths() -> Option<BblOraclePaths> {
    let bbl = bbl_resources_dir()?.join("profiles/BBL");
    let paths = BblOraclePaths {
        process: bbl.join("process/0.20mm Standard @BBL H2C.json"),
        machine: bbl.join("machine/Bambu Lab H2C 0.4 nozzle.json"),
        filament: bbl.join("filament/Generic PLA @BBL H2C 0.4 nozzle.json"),
    };
    if paths.process.is_file() && paths.machine.is_file() && paths.filament.is_file() {
        Some(paths)
    } else {
        None
    }
}
