//! Load Bambu Lab process JSON (`resources/profiles/BBL/process`).

mod emit;
mod inherit;
mod parse;
mod paths;
mod xy;

use std::path::{Path, PathBuf};

use serde_json::Value;
use thiserror::Error;

use crate::SliceSettings;

pub use emit::{config_block_gcode, project_settings_json};
pub use inherit::{flatten_bbl_profile, write_flattened_bbl_profile};
pub use parse::{apply_config_pairs, is_region_key, normalize_filament_colour};
pub use paths::{
    bbl_oracle_paths, bbl_resources_dir, delete_user_filament, json_instantiation_enabled,
    list_bbl_profiles, list_filament_json_dir, list_instantiated_bbl_profiles,
    list_studio_user_filaments, patch_filament_colour, save_user_filament, BblOraclePaths,
    BblProfileEntry, BblProfileKind,
};

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Message(String),
}

/// Load a BBL process profile, following `inherits` in the same directory.
pub fn load_bbl_process(path: impl AsRef<Path>) -> Result<SliceSettings, ConfigError> {
    let path = path.as_ref();
    let dir = path.parent().unwrap_or(Path::new("."));
    let map = inherit::load_inherited(dir, path)?;
    Ok(parse::settings_from_map(&map))
}

/// Overlay another BBL JSON (filament, machine) onto existing settings.
pub fn overlay_bbl_profile(
    settings: &mut SliceSettings,
    path: impl AsRef<Path>,
) -> Result<(), ConfigError> {
    let path = path.as_ref();
    let dir = path.parent().unwrap_or(Path::new("."));
    let map = inherit::load_inherited(dir, path)?;
    parse::apply_map_onto(settings, &map);
    Ok(())
}

/// Flatten a system filament and write `"from": "User"` under `dest_dir`.
pub fn clone_filament_as_user(
    src: impl AsRef<Path>,
    dest_dir: impl AsRef<Path>,
    name: &str,
) -> Result<PathBuf, ConfigError> {
    let src = src.as_ref();
    let mut value = flatten_bbl_profile(src)?;
    if let Value::Object(map) = &mut value {
        map.insert("name".into(), Value::String(name.to_string()));
        map.insert("from".into(), Value::String("User".into()));
        map.insert("type".into(), Value::String("filament".into()));
        map.insert("instantiation".into(), Value::String("true".into()));
        if let Some(stem) = src.file_stem().and_then(|s| s.to_str()) {
            map.insert("inherits".into(), Value::String(stem.to_string()));
        }
    }
    paths::save_user_filament(dest_dir, name, &value)
}

/// Parse Bambu `project_settings.config` / process JSON (no `inherits`).
pub fn settings_from_json(text: &str) -> Result<SliceSettings, ConfigError> {
    let value: Value = serde_json::from_str(text)?;
    let Value::Object(map) = value else {
        return Err(ConfigError::Message(
            "project settings is not a JSON object".into(),
        ));
    };
    Ok(parse::settings_from_map(&map))
}

#[cfg(test)]
mod tests;
