//! Load Bambu Lab process JSON (`resources/profiles/BBL/process`).

mod emit;
mod inherit;
mod parse;
mod paths;
mod xy;

use std::path::Path;

use serde_json::Value;
use thiserror::Error;

use crate::SliceSettings;

pub use emit::{config_block_gcode, project_settings_json};
pub use inherit::{flatten_bbl_profile, write_flattened_bbl_profile};
pub use parse::{apply_config_pairs, is_region_key};
pub use paths::{
    bbl_oracle_paths, bbl_resources_dir, list_bbl_profiles, BblOraclePaths, BblProfileEntry,
    BblProfileKind,
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
