//! Locate upstream `BambuStudio/resources` for oracle profiles.

use std::path::{Path, PathBuf};

use serde_json::Value;

use super::ConfigError;

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

/// C++ combo lists only `instantiation: true` (drops `fdm_filament_*.json` bases).
pub fn list_instantiated_bbl_profiles(kind: BblProfileKind) -> Vec<BblProfileEntry> {
    list_bbl_profiles(kind)
        .into_iter()
        .filter(|e| json_instantiation_enabled(&e.path))
        .collect()
}

pub fn json_instantiation_enabled(path: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(path) else {
        return false;
    };
    let Ok(value) = serde_json::from_str::<Value>(&text) else {
        return false;
    };
    match value.get("instantiation") {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => s.eq_ignore_ascii_case("true"),
        _ => false,
    }
}

/// JSON files in a user filament directory (rewrite or Studio).
pub fn list_filament_json_dir(dir: impl AsRef<Path>) -> Vec<BblProfileEntry> {
    let dir = dir.as_ref();
    let Ok(entries) = std::fs::read_dir(dir) else {
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

pub fn save_user_filament(
    dir: impl AsRef<Path>,
    name: &str,
    value: &Value,
) -> Result<PathBuf, ConfigError> {
    let dir = dir.as_ref();
    std::fs::create_dir_all(dir)?;
    let path = dir.join(format!("{}.json", filament_file_stem(name)));
    std::fs::write(&path, serde_json::to_vec_pretty(value)?)?;
    Ok(path)
}

pub fn delete_user_filament(dir: impl AsRef<Path>, name: &str) -> Result<(), ConfigError> {
    let path = dir
        .as_ref()
        .join(format!("{}.json", filament_file_stem(name)));
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err.into()),
    }
}

pub fn patch_filament_colour(path: impl AsRef<Path>, colour: &str) -> Result<(), ConfigError> {
    let path = path.as_ref();
    let text = std::fs::read_to_string(path)?;
    let mut value: Value = serde_json::from_str(&text)?;
    if let Value::Object(map) = &mut value {
        map.insert("filament_colour".into(), Value::String(colour.to_string()));
    }
    std::fs::write(path, serde_json::to_vec_pretty(&value)?)?;
    Ok(())
}

/// Read-only: C++ Studio `~/.config/BambuStudio/user/*/filament/`.
pub fn list_studio_user_filaments() -> Vec<BblProfileEntry> {
    let root = xdg_config_home().join("BambuStudio").join("user");
    let Ok(users) = std::fs::read_dir(&root) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for user in users.flatten() {
        let dir = user.path().join("filament");
        if dir.is_dir() {
            out.extend(list_filament_json_dir(&dir));
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out.dedup_by(|a, b| a.name == b.name);
    out
}

pub(super) fn filament_file_stem(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '\0' => '_',
            _ => c,
        })
        .collect();
    if s.trim().is_empty() {
        String::from("filament")
    } else {
        s
    }
}

fn xdg_config_home() -> PathBuf {
    if let Ok(p) = std::env::var("XDG_CONFIG_HOME") {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    std::env::var("HOME")
        .map(|h| PathBuf::from(h).join(".config"))
        .unwrap_or_else(|_| PathBuf::from(".config"))
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
