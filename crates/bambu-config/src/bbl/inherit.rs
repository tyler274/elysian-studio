//! Follow BBL `inherits` / `include` when loading system profiles.

use std::path::Path;

use serde_json::Value;

use super::ConfigError;

pub(super) fn load_inherited(
    dir: &Path,
    path: &Path,
) -> Result<serde_json::Map<String, Value>, ConfigError> {
    let text = std::fs::read_to_string(path)?;
    let value: Value = serde_json::from_str(&text)?;
    let Value::Object(mut own) = value else {
        return Err(ConfigError::Message(format!(
            "{} is not a JSON object",
            path.display()
        )));
    };
    let includes = take_includes(&mut own);
    let mut out = if let Some(parent) = own.remove("inherits").and_then(|v| match v {
        Value::String(s) => Some(s),
        _ => None,
    }) {
        let parent_path = dir.join(format!("{parent}.json"));
        if parent_path.is_file() {
            load_inherited(dir, &parent_path)?
        } else {
            serde_json::Map::new()
        }
    } else {
        serde_json::Map::new()
    };
    for name in includes {
        let include_path = dir.join(format!("{name}.json"));
        if !include_path.is_file() {
            continue;
        }
        let included = load_inherited(dir, &include_path)?;
        for (k, v) in included {
            if is_profile_metadata(&k) {
                continue;
            }
            out.insert(k, v);
        }
    }
    for (k, v) in own {
        out.insert(k, v);
    }
    Ok(out)
}

fn take_includes(map: &mut serde_json::Map<String, Value>) -> Vec<String> {
    match map.remove("include") {
        Some(Value::Array(items)) => items
            .into_iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
        Some(Value::String(s)) => vec![s],
        _ => Vec::new(),
    }
}

fn is_profile_metadata(key: &str) -> bool {
    matches!(
        key,
        "name"
            | "type"
            | "from"
            | "inherits"
            | "include"
            | "instantiation"
            | "setting_id"
            | "filament_id"
    )
}

/// Merge `inherits` and emit a JSON object the C++ CLI can `--load-settings`.
pub fn flatten_bbl_profile(path: impl AsRef<Path>) -> Result<Value, ConfigError> {
    let path = path.as_ref();
    let dir = path.parent().unwrap_or(Path::new("."));
    let mut map = load_inherited(dir, path)?;
    map.remove("inherits");
    if !map.contains_key("from") {
        map.insert("from".into(), Value::String("system".into()));
    }
    Ok(Value::Object(map))
}

pub fn write_flattened_bbl_profile(
    src: impl AsRef<Path>,
    dst: impl AsRef<Path>,
) -> Result<(), ConfigError> {
    let value = flatten_bbl_profile(src)?;
    std::fs::write(dst, serde_json::to_vec_pretty(&value)?)?;
    Ok(())
}
