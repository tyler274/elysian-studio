//! Load Bambu Lab process JSON (`resources/profiles/BBL/process`).

use std::path::{Path, PathBuf};

use serde_json::Value;
use thiserror::Error;

use crate::{
    InfillPattern, IroningPattern, IroningType, SeamPosition, SliceSettings, SurfacePattern,
    WallGenerator,
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
    let map = load_inherited(dir, path)?;
    Ok(settings_from_map(&map))
}

fn load_inherited(dir: &Path, path: &Path) -> Result<serde_json::Map<String, Value>, ConfigError> {
    let text = std::fs::read_to_string(path)?;
    let value: Value = serde_json::from_str(&text)?;
    let Value::Object(mut map) = value else {
        return Err(ConfigError::Message(format!(
            "{} is not a JSON object",
            path.display()
        )));
    };
    if let Some(parent) = map.get("inherits").and_then(Value::as_str) {
        let parent_path = dir.join(format!("{parent}.json"));
        if parent_path.is_file() {
            let mut base = load_inherited(dir, &parent_path)?;
            for (k, v) in map {
                if k == "inherits" {
                    continue;
                }
                base.insert(k, v);
            }
            return Ok(base);
        }
    }
    map.remove("inherits");
    Ok(map)
}

fn settings_from_map(map: &serde_json::Map<String, Value>) -> SliceSettings {
    let mut s = SliceSettings::default();
    if let Some(v) = num(map, "layer_height") {
        s.layer_height_mm = v;
    }
    if let Some(v) = num(map, "initial_layer_print_height") {
        s.first_layer_height_mm = v;
    }
    if let Some(v) = num(map, "min_layer_height") {
        s.min_layer_height_mm = v.max(0.01);
    }
    if let Some(v) = num(map, "max_layer_height") {
        s.max_layer_height_mm = v.max(s.min_layer_height_mm);
    }
    if let Some(v) = bool_val(map, "precise_z_height") {
        s.precise_z_height = v;
    }
    if let Some(v) = num(map, "elefant_foot_compensation") {
        s.elephant_foot_mm = v.max(0.0);
    }
    if let Some(v) = num(map, "xy_contour_compensation") {
        s.xy_contour_compensation_mm = v;
    }
    if let Some(v) = num(map, "xy_hole_compensation") {
        s.xy_hole_compensation_mm = v;
    }
    if let Some(v) = num(map, "line_width") {
        s.line_width_mm = v;
    }
    if let Some(v) = u32_val(map, "wall_loops") {
        s.wall_loops = v.max(1);
    }
    if let Some(v) = percent(map, "sparse_infill_density") {
        s.infill_density = v;
    }
    if let Some(name) = text(map, "sparse_infill_pattern") {
        if let Some(p) = InfillPattern::from_name(&name) {
            s.infill_pattern = p;
        }
    }
    if let Some(name) = text(map, "seam_position") {
        if let Some(p) = SeamPosition::from_name(&name) {
            s.seam = p;
        }
    }
    if let Some(name) = text(map, "wall_generator") {
        if name.eq_ignore_ascii_case("classic") {
            s.wall_generator = WallGenerator::Classic;
        }
    }
    if let Some(v) = u32_val(map, "skirt_loops") {
        s.skirt_loops = v;
    }
    if let Some(v) = num(map, "skirt_distance") {
        s.skirt_distance_mm = v;
    }
    if let Some(v) = num(map, "brim_width") {
        s.brim_width_mm = v;
    }
    if let Some(v) = u32_val(map, "raft_layers") {
        s.raft_layers = v;
    }
    if let Some(v) = num(map, "raft_contact_distance") {
        s.raft_contact_distance_mm = v.max(0.0);
    }
    if let Some(v) = num(map, "raft_expansion") {
        s.raft_expansion_mm = v.max(0.0);
    }
    if let Some(v) = num(map, "raft_first_layer_expansion") {
        s.raft_first_layer_expansion_mm = v;
    }
    if let Some(v) = percent(map, "raft_first_layer_density") {
        s.raft_first_layer_density = v.clamp(0.10, 1.0);
    }
    if let Some(v) = bool_val(map, "enable_support") {
        s.enable_support = v;
    }
    if let Some(v) = num(map, "support_threshold_angle") {
        s.support_threshold_angle_deg = v;
    }
    if let Some(v) = num(map, "support_object_xy_distance") {
        s.support_xy_distance_mm = v;
    }
    if let Some(v) = num(map, "support_top_z_distance") {
        s.support_top_z_distance_mm = v;
    }
    if let Some(v) = u32_val(map, "support_interface_top_layers") {
        s.support_interface_layers = v;
    }
    if let Some(v) = u32_val(map, "bottom_shell_layers") {
        s.bottom_shell_layers = v;
    }
    if let Some(v) = u32_val(map, "top_shell_layers") {
        s.top_shell_layers = v;
    }
    if let Some(name) = text(map, "top_surface_pattern") {
        if let Some(p) = SurfacePattern::from_name(&name) {
            s.top_surface_pattern = p;
        }
    }
    if let Some(name) = text(map, "bottom_surface_pattern") {
        if let Some(p) = SurfacePattern::from_name(&name) {
            s.bottom_surface_pattern = p;
        }
    }
    if let Some(v) = num(map, "outer_wall_speed") {
        s.print_speed_mm_s = v;
    }
    if let Some(v) = num(map, "sparse_infill_speed") {
        s.infill_speed_mm_s = v;
    }
    if let Some(v) = num(map, "travel_speed") {
        s.travel_speed_mm_s = v;
    }
    if let Some(v) = num(map, "support_speed").or_else(|| num(map, "support_interface_speed")) {
        s.support_speed_mm_s = v;
    }
    if let Some(v) = num(map, "internal_solid_infill_speed") {
        s.solid_infill_speed_mm_s = v;
    }
    if let Some(name) = text(map, "ironing_type") {
        if let Some(t) = IroningType::from_name(&name) {
            s.ironing_type = t;
        }
    }
    if let Some(name) = text(map, "ironing_pattern") {
        if let Some(p) = IroningPattern::from_name(&name) {
            s.ironing_pattern = p;
        }
    }
    if let Some(v) = percent(map, "ironing_flow") {
        s.ironing_flow = v;
    }
    if let Some(v) = num(map, "ironing_spacing") {
        s.ironing_spacing_mm = v;
    }
    if let Some(v) = num(map, "ironing_inset") {
        s.ironing_inset_mm = v;
    }
    if let Some(v) = num(map, "ironing_speed") {
        s.ironing_speed_mm_s = v;
    }
    s
}

fn text(map: &serde_json::Map<String, Value>, key: &str) -> Option<String> {
    map.get(key).and_then(value_text)
}

fn value_text(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(if *b { "1".into() } else { "0".into() }),
        Value::Array(a) => a.first().and_then(value_text),
        _ => None,
    }
}

fn num(map: &serde_json::Map<String, Value>, key: &str) -> Option<f64> {
    let raw = text(map, key)?;
    raw.trim_end_matches('%').trim().parse().ok()
}

fn percent(map: &serde_json::Map<String, Value>, key: &str) -> Option<f64> {
    let raw = text(map, key)?;
    let trimmed = raw.trim();
    if let Some(p) = trimmed.strip_suffix('%') {
        return p.trim().parse::<f64>().ok().map(|v| v / 100.0);
    }
    trimmed
        .parse::<f64>()
        .ok()
        .map(|v| if v > 1.0 { v / 100.0 } else { v })
}

fn u32_val(map: &serde_json::Map<String, Value>, key: &str) -> Option<u32> {
    num(map, key).map(|v| v.round() as u32)
}

fn bool_val(map: &serde_json::Map<String, Value>, key: &str) -> Option<bool> {
    let raw = text(map, key)?;
    match raw.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" => Some(true),
        "0" | "false" | "no" => Some(false),
        _ => None,
    }
}

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

pub fn bbl_oracle_paths() -> Option<BblOraclePaths> {
    let bbl = bbl_resources_dir()?.join("profiles/BBL");
    let paths = BblOraclePaths {
        process: bbl.join("process/0.20mm Standard @BBL X1C.json"),
        machine: bbl.join("machine/Bambu Lab P1S 0.4 nozzle.json"),
        filament: bbl.join("filament/Generic PLA.json"),
    };
    if paths.process.is_file() && paths.machine.is_file() && paths.filament.is_file() {
        Some(paths)
    } else {
        None
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::InfillPattern;

    #[test]
    fn inline_process_json() {
        let dir = std::env::temp_dir().join("bambu-rs-bbl-process");
        let _ = std::fs::create_dir_all(&dir);
        let parent = dir.join("base.json");
        std::fs::write(
            &parent,
            r#"{
                "type": "process",
                "name": "base",
                "wall_loops": "2",
                "layer_height": "0.2",
                "sparse_infill_density": "15%",
                "sparse_infill_pattern": "grid",
                "brim_width": "5"
            }"#,
        )
        .unwrap();
        let child = dir.join("child.json");
        std::fs::write(
            &child,
            r#"{
                "inherits": "base",
                "top_shell_layers": "5",
                "sparse_infill_pattern": "gyroid"
            }"#,
        )
        .unwrap();
        let s = load_bbl_process(&child).unwrap();
        assert_eq!(s.wall_loops, 2);
        assert_eq!(s.top_shell_layers, 5);
        assert!((s.infill_density - 0.15).abs() < 1e-9);
        assert_eq!(s.infill_pattern, InfillPattern::Gyroid);
        assert!((s.brim_width_mm - 5.0).abs() < 1e-9);
    }

    #[test]
    fn upstream_fdm_process_0_20() {
        let Some(res) = bbl_resources_dir() else {
            panic!(
                "upstream BambuStudio resources not found; set BAMBU_STUDIO_RESOURCES or keep ../BambuStudio checked out"
            );
        };
        let path = res.join("profiles/BBL/process/fdm_process_single_0.20.json");
        assert!(path.is_file(), "missing {}", path.display());
        let s = load_bbl_process(&path).unwrap();
        assert!((s.layer_height_mm - 0.2).abs() < 1e-9);
        assert_eq!(s.wall_loops, 2);
        assert_eq!(s.top_shell_layers, 5);
        assert_eq!(s.bottom_shell_layers, 3);
        assert_eq!(s.skirt_loops, 0);
        assert!((s.brim_width_mm - 5.0).abs() < 1e-9);
        assert!((s.infill_density - 0.15).abs() < 1e-9);
        assert_eq!(s.infill_pattern, InfillPattern::Grid);
        assert!(!s.enable_support);
        assert_eq!(s.ironing_type, crate::IroningType::NoIroning);
        assert!((s.ironing_flow - 0.10).abs() < 1e-9);
        assert!((s.elephant_foot_mm - 0.15).abs() < 1e-9);
        assert!(!s.precise_z_height);
        assert_eq!(s.top_surface_pattern, crate::SurfacePattern::MonotonicLine);
        assert_eq!(s.bottom_surface_pattern, crate::SurfacePattern::Monotonic);
        assert_eq!(s.raft_layers, 0);
        let baked = SliceSettings::bbl_0_20();
        assert_eq!(baked.top_shell_layers, s.top_shell_layers);
        assert_eq!(baked.wall_loops, s.wall_loops);
        assert_eq!(baked.infill_pattern, s.infill_pattern);
        assert_eq!(baked.top_surface_pattern, s.top_surface_pattern);
        assert_eq!(baked.bottom_surface_pattern, s.bottom_surface_pattern);
        assert!((baked.infill_density - s.infill_density).abs() < 1e-9);
        assert!((baked.brim_width_mm - s.brim_width_mm).abs() < 1e-9);
        assert!((baked.elephant_foot_mm - s.elephant_foot_mm).abs() < 1e-9);
    }

    #[test]
    fn flatten_standard_0_20_merges_inherits() {
        let paths = bbl_oracle_paths().expect("upstream BambuStudio profiles");
        let flat = flatten_bbl_profile(&paths.process).unwrap();
        let obj = flat.as_object().unwrap();
        assert_eq!(
            value_text(obj.get("sparse_infill_density").unwrap()).as_deref(),
            Some("15%")
        );
        assert_eq!(
            value_text(obj.get("sparse_infill_pattern").unwrap()).as_deref(),
            Some("grid")
        );
        assert_eq!(
            value_text(obj.get("top_shell_layers").unwrap()).as_deref(),
            Some("5")
        );
        assert_eq!(
            value_text(obj.get("skirt_loops").unwrap()).as_deref(),
            Some("0")
        );
        assert_eq!(
            value_text(obj.get("brim_width").unwrap()).as_deref(),
            Some("5")
        );
        assert!(!obj.contains_key("inherits"));
        let s = load_bbl_process(&paths.process).unwrap();
        assert!((s.infill_density - 0.15).abs() < 1e-9);
        assert_eq!(s.infill_pattern, InfillPattern::Grid);
        assert_eq!(s.top_shell_layers, 5);
        assert_eq!(s.skirt_loops, 0);
    }
}
