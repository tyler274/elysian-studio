//! Load Bambu Lab process JSON (`resources/profiles/BBL/process`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value;
use thiserror::Error;

use crate::{
    FuzzySkinType, InfillPattern, IroningPattern, IroningType, OverhangFanThreshold, SeamPosition,
    SliceSettings, SupportType, SurfacePattern, TopOneWallType, WallGenerator,
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

/// Overlay another BBL JSON (filament, machine) onto existing settings.
pub fn overlay_bbl_profile(
    settings: &mut SliceSettings,
    path: impl AsRef<Path>,
) -> Result<(), ConfigError> {
    let path = path.as_ref();
    let dir = path.parent().unwrap_or(Path::new("."));
    let map = load_inherited(dir, path)?;
    apply_map_onto(settings, &map);
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
    Ok(settings_from_map(&map))
}

/// Emit the keys we understand as a Bambu project_settings JSON object.
pub fn project_settings_json(settings: &SliceSettings) -> Result<String, ConfigError> {
    let mut map = serde_json::Map::new();
    insert(&mut map, "version", env!("CARGO_PKG_VERSION"));
    insert(&mut map, "name", "project_settings");
    insert(&mut map, "from", "project");
    insert(&mut map, "layer_height", num_str(settings.layer_height_mm));
    insert(
        &mut map,
        "initial_layer_print_height",
        num_str(settings.first_layer_height_mm),
    );
    insert(
        &mut map,
        "min_layer_height",
        num_str(settings.min_layer_height_mm),
    );
    insert(
        &mut map,
        "max_layer_height",
        num_str(settings.max_layer_height_mm),
    );
    insert_bool(&mut map, "precise_z_height", settings.precise_z_height);
    insert(
        &mut map,
        "elefant_foot_compensation",
        num_str(settings.elephant_foot_mm),
    );
    insert(
        &mut map,
        "xy_contour_compensation",
        num_str(settings.xy_contour_compensation_mm),
    );
    insert(
        &mut map,
        "xy_hole_compensation",
        num_str(settings.xy_hole_compensation_mm),
    );
    insert(&mut map, "line_width", num_str(settings.line_width_mm));
    insert(&mut map, "wall_loops", settings.wall_loops.to_string());
    insert(
        &mut map,
        "top_one_wall_type",
        settings.top_one_wall.as_str(),
    );
    insert(
        &mut map,
        "sparse_infill_density",
        pct_str(settings.infill_density),
    );
    insert(
        &mut map,
        "sparse_infill_pattern",
        settings.infill_pattern.as_str(),
    );
    insert(&mut map, "seam_position", settings.seam.as_str());
    insert(&mut map, "wall_generator", settings.wall_generator.as_str());
    insert(
        &mut map,
        "min_feature_size",
        pct_str(settings.min_feature_size),
    );
    insert(&mut map, "min_bead_width", pct_str(settings.min_bead_width));
    insert(&mut map, "fuzzy_skin", settings.fuzzy_skin.as_str());
    insert(
        &mut map,
        "fuzzy_skin_thickness",
        num_str(settings.fuzzy_skin_thickness_mm),
    );
    insert(
        &mut map,
        "fuzzy_skin_point_distance",
        num_str(settings.fuzzy_skin_point_distance_mm),
    );
    insert_bool(
        &mut map,
        "fuzzy_skin_first_layer",
        settings.fuzzy_skin_first_layer,
    );
    insert(&mut map, "skirt_loops", settings.skirt_loops.to_string());
    insert(
        &mut map,
        "skirt_distance",
        num_str(settings.skirt_distance_mm),
    );
    insert(&mut map, "brim_width", num_str(settings.brim_width_mm));
    insert(&mut map, "raft_layers", settings.raft_layers.to_string());
    insert(
        &mut map,
        "raft_contact_distance",
        num_str(settings.raft_contact_distance_mm),
    );
    insert(
        &mut map,
        "raft_expansion",
        num_str(settings.raft_expansion_mm),
    );
    insert(
        &mut map,
        "raft_first_layer_expansion",
        num_str(settings.raft_first_layer_expansion_mm),
    );
    insert(
        &mut map,
        "raft_first_layer_density",
        pct_str(settings.raft_first_layer_density),
    );
    insert_bool(&mut map, "enable_support", settings.enable_support);
    insert(&mut map, "support_type", settings.support_type.as_str());
    insert(
        &mut map,
        "tree_support_branch_angle",
        num_str(settings.tree_branch_angle_deg),
    );
    insert(
        &mut map,
        "tree_support_branch_diameter",
        num_str(settings.tree_branch_diameter_mm),
    );
    insert(
        &mut map,
        "support_threshold_angle",
        num_str(settings.support_threshold_angle_deg),
    );
    insert(
        &mut map,
        "support_object_xy_distance",
        num_str(settings.support_xy_distance_mm),
    );
    insert(
        &mut map,
        "support_top_z_distance",
        num_str(settings.support_top_z_distance_mm),
    );
    insert(
        &mut map,
        "support_interface_top_layers",
        settings.support_interface_layers.to_string(),
    );
    insert(
        &mut map,
        "bottom_shell_layers",
        settings.bottom_shell_layers.to_string(),
    );
    insert(
        &mut map,
        "top_shell_layers",
        settings.top_shell_layers.to_string(),
    );
    insert(
        &mut map,
        "top_surface_pattern",
        settings.top_surface_pattern.as_str(),
    );
    insert(
        &mut map,
        "bottom_surface_pattern",
        settings.bottom_surface_pattern.as_str(),
    );
    insert(
        &mut map,
        "outer_wall_speed",
        num_str(settings.print_speed_mm_s),
    );
    insert(
        &mut map,
        "inner_wall_speed",
        num_str(settings.inner_wall_speed_mm_s),
    );
    insert(
        &mut map,
        "initial_layer_speed",
        num_str(settings.first_layer_speed_mm_s),
    );
    insert(
        &mut map,
        "initial_layer_infill_speed",
        num_str(settings.first_layer_infill_speed_mm_s),
    );
    insert_bool(
        &mut map,
        "detect_overhang_wall",
        settings.detect_overhang_wall,
    );
    insert_bool(
        &mut map,
        "enable_overhang_speed",
        settings.enable_overhang_speed,
    );
    insert(
        &mut map,
        "overhang_totally_speed",
        num_str(settings.overhang_speed_mm_s),
    );
    insert(
        &mut map,
        "overhang_1_4_speed",
        num_str(settings.overhang_1_4_speed_mm_s),
    );
    insert(
        &mut map,
        "overhang_2_4_speed",
        num_str(settings.overhang_2_4_speed_mm_s),
    );
    insert(
        &mut map,
        "overhang_3_4_speed",
        num_str(settings.overhang_3_4_speed_mm_s),
    );
    insert(
        &mut map,
        "overhang_4_4_speed",
        num_str(settings.overhang_4_4_speed_mm_s),
    );
    insert(
        &mut map,
        "bridge_speed",
        num_str(settings.bridge_speed_mm_s),
    );
    insert(
        &mut map,
        "top_surface_speed",
        num_str(settings.top_surface_speed_mm_s),
    );
    insert(
        &mut map,
        "small_perimeter_speed",
        if settings.small_perimeter_speed_is_percent {
            format!("{}%", num_str(settings.small_perimeter_speed))
        } else {
            num_str(settings.small_perimeter_speed)
        },
    );
    insert(
        &mut map,
        "small_perimeter_threshold",
        num_str(settings.small_perimeter_threshold_mm),
    );
    insert(
        &mut map,
        "sparse_infill_speed",
        num_str(settings.infill_speed_mm_s),
    );
    insert(
        &mut map,
        "gap_infill_speed",
        num_str(settings.gap_infill_speed_mm_s),
    );
    insert(
        &mut map,
        "filter_out_gap_fill",
        num_str(settings.filter_out_gap_fill_mm),
    );
    insert(
        &mut map,
        "travel_speed",
        num_str(settings.travel_speed_mm_s),
    );
    insert(
        &mut map,
        "support_speed",
        num_str(settings.support_speed_mm_s),
    );
    insert(
        &mut map,
        "support_interface_speed",
        num_str(settings.support_interface_speed_mm_s),
    );
    insert(
        &mut map,
        "internal_solid_infill_speed",
        num_str(settings.solid_infill_speed_mm_s),
    );
    insert(&mut map, "ironing_type", settings.ironing_type.as_str());
    insert(
        &mut map,
        "ironing_pattern",
        settings.ironing_pattern.as_str(),
    );
    insert(&mut map, "ironing_flow", pct_str(settings.ironing_flow));
    insert(
        &mut map,
        "ironing_spacing",
        num_str(settings.ironing_spacing_mm),
    );
    insert(
        &mut map,
        "ironing_inset",
        num_str(settings.ironing_inset_mm),
    );
    insert(
        &mut map,
        "ironing_speed",
        num_str(settings.ironing_speed_mm_s),
    );
    insert(
        &mut map,
        "default_acceleration",
        num_str(settings.default_acceleration_mm_s2),
    );
    insert(
        &mut map,
        "travel_acceleration",
        num_str(settings.travel_acceleration_mm_s2),
    );
    insert(
        &mut map,
        "filament_density",
        num_str(settings.filament_density_g_cm3),
    );
    insert(
        &mut map,
        "fan_min_speed",
        settings.fan_min_speed.to_string(),
    );
    insert(
        &mut map,
        "fan_max_speed",
        settings.fan_max_speed.to_string(),
    );
    insert_bool(
        &mut map,
        "enable_overhang_bridge_fan",
        settings.enable_overhang_bridge_fan,
    );
    insert(
        &mut map,
        "overhang_fan_speed",
        settings.overhang_fan_speed.to_string(),
    );
    insert(
        &mut map,
        "overhang_fan_threshold",
        settings.overhang_fan_threshold.as_str(),
    );
    insert(
        &mut map,
        "ironing_fan_speed",
        settings.ironing_fan_speed.to_string(),
    );
    insert(
        &mut map,
        "close_fan_the_first_x_layers",
        settings.close_fan_the_first_x_layers.to_string(),
    );
    insert(
        &mut map,
        "first_x_layer_part_fan_speed",
        settings.first_x_layer_part_fan_speed.to_string(),
    );
    insert(
        &mut map,
        "full_fan_speed_layer",
        settings.full_fan_speed_layer.to_string(),
    );
    insert(
        &mut map,
        "fan_cooling_layer_time",
        num_str(settings.fan_cooling_layer_time_s),
    );
    insert(
        &mut map,
        "slow_down_layer_time",
        num_str(settings.slow_down_layer_time_s),
    );
    insert_bool(
        &mut map,
        "reduce_fan_stop_start_freq",
        settings.reduce_fan_stop_start_freq,
    );
    insert_bool(
        &mut map,
        "slow_down_for_layer_cooling",
        settings.slow_down_for_layer_cooling,
    );
    insert(
        &mut map,
        "slow_down_min_speed",
        num_str(settings.slow_down_min_speed_mm_s),
    );
    insert(
        &mut map,
        "filament_flow_ratio",
        num_str(settings.flow_ratio),
    );
    insert(
        &mut map,
        "filament_max_volumetric_speed",
        num_str(settings.filament_max_volumetric_speed_mm3_s),
    );
    insert(
        &mut map,
        "retraction_length",
        num_str(settings.retraction_length_mm),
    );
    insert(
        &mut map,
        "retraction_speed",
        num_str(settings.retraction_speed_mm_s),
    );
    insert(
        &mut map,
        "deretraction_speed",
        num_str(settings.deretraction_speed_mm_s),
    );
    insert(
        &mut map,
        "retraction_minimum_travel",
        num_str(settings.retraction_minimum_travel_mm),
    );
    insert_bool(
        &mut map,
        "retract_when_changing_layer",
        settings.retract_when_changing_layer,
    );
    insert_bool(&mut map, "wipe", settings.wipe);
    insert(
        &mut map,
        "wipe_distance",
        num_str(settings.wipe_distance_mm),
    );
    insert(
        &mut map,
        "retract_before_wipe",
        format!("{}%", num_str(settings.retract_before_wipe * 100.0)),
    );
    insert(
        &mut map,
        "wipe_speed",
        format!("{}%", num_str(settings.wipe_speed_percent)),
    );
    insert_bool(
        &mut map,
        "role_base_wipe_speed",
        settings.role_base_wipe_speed,
    );
    insert(
        &mut map,
        "retract_restart_extra",
        num_str(settings.retract_restart_extra_mm),
    );
    insert(
        &mut map,
        "machine_max_jerk_x",
        num_str(settings.xy_jerk_mm_s),
    );
    insert(
        &mut map,
        "machine_max_jerk_y",
        num_str(settings.xy_jerk_mm_s),
    );
    insert(
        &mut map,
        "machine_max_jerk_z",
        num_str(settings.z_jerk_mm_s),
    );
    Ok(serde_json::to_string_pretty(&Value::Object(map))?)
}

fn insert(map: &mut serde_json::Map<String, Value>, key: &str, value: impl Into<String>) {
    map.insert(key.to_string(), Value::String(value.into()));
}

fn insert_bool(map: &mut serde_json::Map<String, Value>, key: &str, value: bool) {
    insert(map, key, if value { "1" } else { "0" });
}

fn num_str(v: f64) -> String {
    format!("{v}")
}

fn pct_str(frac: f64) -> String {
    format!("{}%", (frac * 100.0).round())
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

/// C++ `PrintRegionConfig` keys (volume / modifier metadata). Object-level
/// keys such as `layer_height` and `enable_support` are ignored here.
pub fn is_region_key(key: &str) -> bool {
    matches!(
        key,
        "line_width"
            | "wall_loops"
            | "only_one_wall_top"
            | "top_one_wall_type"
            | "sparse_infill_density"
            | "sparse_infill_pattern"
            | "seam_position"
            | "wall_generator"
            | "min_feature_size"
            | "min_bead_width"
            | "fuzzy_skin"
            | "fuzzy_skin_thickness"
            | "fuzzy_skin_point_distance"
            | "fuzzy_skin_first_layer"
            | "bottom_shell_layers"
            | "top_shell_layers"
            | "top_surface_pattern"
            | "bottom_surface_pattern"
            | "outer_wall_speed"
            | "inner_wall_speed"
            | "detect_overhang_wall"
            | "enable_overhang_speed"
            | "overhang_totally_speed"
            | "overhang_1_4_speed"
            | "overhang_2_4_speed"
            | "overhang_3_4_speed"
            | "overhang_4_4_speed"
            | "bridge_speed"
            | "top_surface_speed"
            | "small_perimeter_speed"
            | "small_perimeter_threshold"
            | "sparse_infill_speed"
            | "gap_infill_speed"
            | "internal_solid_infill_speed"
            | "ironing_type"
            | "ironing_pattern"
            | "ironing_flow"
            | "ironing_spacing"
            | "ironing_inset"
            | "ironing_speed"
            | "wall_filament"
            | "sparse_infill_filament"
            | "solid_infill_filament"
    )
}

/// Overlay 3MF / `model_settings.config` key-values onto [`SliceSettings`].
///
/// When `region_only` is set, object-level keys are skipped (C++
/// `apply_to_print_region_config`).
pub fn apply_config_pairs(
    settings: &mut SliceSettings,
    pairs: &BTreeMap<String, String>,
    region_only: bool,
) {
    let mut map = serde_json::Map::new();
    for (key, value) in pairs {
        if region_only && !is_region_key(key) {
            continue;
        }
        map.insert(key.clone(), Value::String(value.clone()));
    }
    apply_map_onto(settings, &map);
}

fn settings_from_map(map: &serde_json::Map<String, Value>) -> SliceSettings {
    let mut s = SliceSettings::default();
    apply_map_onto(&mut s, map);
    s
}

fn apply_map_onto(s: &mut SliceSettings, map: &serde_json::Map<String, Value>) {
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
    if let Some(v) = bool_val(map, "only_one_wall_top") {
        s.top_one_wall = if v {
            TopOneWallType::AllTop
        } else {
            TopOneWallType::None
        };
    }
    if let Some(name) = text(map, "top_one_wall_type") {
        if let Some(t) = TopOneWallType::from_name(&name) {
            s.top_one_wall = t;
        }
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
        if let Some(g) = WallGenerator::from_name(&name) {
            s.wall_generator = g;
        }
    }
    if let Some(v) = percent(map, "min_feature_size") {
        s.min_feature_size = v.max(0.0);
    }
    if let Some(v) = percent(map, "min_bead_width") {
        s.min_bead_width = v.max(0.0);
    }
    if let Some(name) = text(map, "fuzzy_skin") {
        if let Some(t) = FuzzySkinType::from_name(&name) {
            s.fuzzy_skin = t;
        }
    }
    if let Some(v) = num(map, "fuzzy_skin_thickness") {
        s.fuzzy_skin_thickness_mm = v.max(0.0);
    }
    if let Some(v) = num(map, "fuzzy_skin_point_distance") {
        s.fuzzy_skin_point_distance_mm = v.max(0.0);
    }
    if let Some(v) = bool_val(map, "fuzzy_skin_first_layer") {
        s.fuzzy_skin_first_layer = v;
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
    if let Some(name) = text(map, "support_type") {
        if let Some(t) = SupportType::from_name(&name) {
            s.support_type = t;
        }
    }
    if let Some(v) = num(map, "tree_support_branch_angle") {
        s.tree_branch_angle_deg = v.clamp(0.0, 89.0);
    }
    if let Some(v) = num(map, "tree_support_branch_diameter") {
        s.tree_branch_diameter_mm = v.max(0.0);
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
    if let Some(v) = num(map, "inner_wall_speed") {
        s.inner_wall_speed_mm_s = v;
    }
    if let Some(v) = num(map, "initial_layer_speed") {
        s.first_layer_speed_mm_s = v;
    }
    if let Some(v) = num(map, "initial_layer_infill_speed") {
        s.first_layer_infill_speed_mm_s = v;
    }
    if let Some(v) = bool_val(map, "detect_overhang_wall") {
        s.detect_overhang_wall = v;
    }
    if let Some(v) = bool_val(map, "enable_overhang_speed") {
        s.enable_overhang_speed = v;
    }
    if let Some(v) = num(map, "overhang_totally_speed") {
        s.overhang_speed_mm_s = v.max(0.0);
    }
    if let Some(v) = num(map, "overhang_1_4_speed") {
        s.overhang_1_4_speed_mm_s = v.max(0.0);
    }
    if let Some(v) = num(map, "overhang_2_4_speed") {
        s.overhang_2_4_speed_mm_s = v.max(0.0);
    }
    if let Some(v) = num(map, "overhang_3_4_speed") {
        s.overhang_3_4_speed_mm_s = v.max(0.0);
    }
    if let Some(v) = num(map, "overhang_4_4_speed") {
        s.overhang_4_4_speed_mm_s = v.max(0.0);
    }
    if let Some(v) = num(map, "bridge_speed") {
        s.bridge_speed_mm_s = v.max(0.0);
    }
    if let Some(v) = num(map, "top_surface_speed") {
        s.top_surface_speed_mm_s = v.max(0.0);
    }
    if let Some((v, is_percent)) = float_or_percent(map, "small_perimeter_speed") {
        s.small_perimeter_speed = v.max(0.0);
        s.small_perimeter_speed_is_percent = is_percent;
    }
    if let Some(v) = num(map, "small_perimeter_threshold") {
        s.small_perimeter_threshold_mm = v.max(0.0);
    }
    if let Some(v) = num(map, "sparse_infill_speed") {
        s.infill_speed_mm_s = v;
    }
    if let Some(v) = num(map, "gap_infill_speed") {
        s.gap_infill_speed_mm_s = v.max(0.0);
    }
    if let Some(v) = num(map, "filter_out_gap_fill") {
        s.filter_out_gap_fill_mm = v.max(0.0);
    }
    if let Some(v) = num(map, "travel_speed") {
        s.travel_speed_mm_s = v;
    }
    if let Some(v) = num(map, "support_speed") {
        s.support_speed_mm_s = v;
    }
    if let Some(v) = num(map, "support_interface_speed") {
        s.support_interface_speed_mm_s = v;
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
    if let Some(v) = num(map, "default_acceleration") {
        s.default_acceleration_mm_s2 = v.max(0.0);
    }
    if let Some(v) = num(map, "travel_acceleration") {
        s.travel_acceleration_mm_s2 = v.max(0.0);
    }
    if let Some(v) = num(map, "filament_density") {
        s.filament_density_g_cm3 = v.max(0.0);
    }
    if let Some(v) = u32_val(map, "fan_min_speed") {
        s.fan_min_speed = v.min(100);
    }
    if let Some(v) = u32_val(map, "fan_max_speed") {
        s.fan_max_speed = v.min(100);
    }
    if let Some(v) = bool_val(map, "enable_overhang_bridge_fan") {
        s.enable_overhang_bridge_fan = v;
    }
    if let Some(v) = u32_val(map, "overhang_fan_speed") {
        s.overhang_fan_speed = v.min(100);
    }
    if let Some(name) = text(map, "overhang_fan_threshold") {
        if let Some(t) = OverhangFanThreshold::from_name(&name) {
            s.overhang_fan_threshold = t;
        }
    }
    if let Some(v) = num(map, "ironing_fan_speed") {
        s.ironing_fan_speed = v.round() as i32;
    }
    if let Some(v) = u32_val(map, "close_fan_the_first_x_layers") {
        s.close_fan_the_first_x_layers = v;
    }
    if let Some(v) = u32_val(map, "first_x_layer_part_fan_speed") {
        s.first_x_layer_part_fan_speed = v.min(100);
    }
    if let Some(v) = u32_val(map, "full_fan_speed_layer") {
        s.full_fan_speed_layer = v;
    }
    if let Some(v) = num(map, "fan_cooling_layer_time") {
        s.fan_cooling_layer_time_s = v.max(0.0);
    }
    if let Some(v) = num(map, "slow_down_layer_time") {
        s.slow_down_layer_time_s = v.max(0.0);
    }
    if let Some(v) = bool_val(map, "reduce_fan_stop_start_freq") {
        s.reduce_fan_stop_start_freq = v;
    }
    if let Some(v) = bool_val(map, "slow_down_for_layer_cooling") {
        s.slow_down_for_layer_cooling = v;
    }
    if let Some(v) = num(map, "slow_down_min_speed") {
        s.slow_down_min_speed_mm_s = v.max(0.0);
    }
    if let Some(v) = num(map, "filament_flow_ratio") {
        s.flow_ratio = v.max(0.0);
    }
    if let Some(v) = num(map, "filament_max_volumetric_speed") {
        s.filament_max_volumetric_speed_mm3_s = v.max(0.0);
    }
    if let Some(v) = num(map, "machine_max_jerk_x").or_else(|| num(map, "machine_max_jerk_y")) {
        s.xy_jerk_mm_s = v.max(0.0);
    }
    if let Some(v) = num(map, "machine_max_jerk_z") {
        s.z_jerk_mm_s = v.max(0.0);
    }
    if let Some(v) = filament_or_printer(map, "filament_retraction_length", "retraction_length") {
        s.retraction_length_mm = v.max(0.0);
    }
    if let Some(v) = filament_or_printer(map, "filament_retraction_speed", "retraction_speed") {
        s.retraction_speed_mm_s = v.max(0.0);
    }
    if let Some(v) = filament_or_printer(map, "filament_deretraction_speed", "deretraction_speed") {
        s.deretraction_speed_mm_s = v.max(0.0);
    }
    if let Some(v) = filament_or_printer(
        map,
        "filament_retraction_minimum_travel",
        "retraction_minimum_travel",
    ) {
        s.retraction_minimum_travel_mm = v.max(0.0);
    }
    if let Some(v) = filament_or_printer_bool(
        map,
        "filament_retract_when_changing_layer",
        "retract_when_changing_layer",
    ) {
        s.retract_when_changing_layer = v;
    }
    if let Some(v) = filament_or_printer_bool(map, "filament_wipe", "wipe") {
        s.wipe = v;
    }
    if let Some(v) = filament_or_printer(map, "filament_wipe_distance", "wipe_distance") {
        s.wipe_distance_mm = v.max(0.0);
    }
    if let Some(v) =
        filament_or_printer_percent(map, "filament_retract_before_wipe", "retract_before_wipe")
    {
        s.retract_before_wipe = v.clamp(0.0, 1.0);
    }
    if let Some((v, _)) = float_or_percent(map, "wipe_speed") {
        s.wipe_speed_percent = v.max(0.0);
    }
    if let Some(v) = bool_val(map, "role_base_wipe_speed") {
        s.role_base_wipe_speed = v;
    }
    if let Some(v) = filament_or_printer(
        map,
        "filament_retract_restart_extra",
        "retract_restart_extra",
    ) {
        s.retract_restart_extra_mm = v;
    }
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

/// Filament JSON uses `"nil"` to inherit the printer value.
fn filament_or_printer(
    map: &serde_json::Map<String, Value>,
    filament: &str,
    printer: &str,
) -> Option<f64> {
    num(map, filament).or_else(|| num(map, printer))
}

fn filament_or_printer_bool(
    map: &serde_json::Map<String, Value>,
    filament: &str,
    printer: &str,
) -> Option<bool> {
    bool_val(map, filament).or_else(|| bool_val(map, printer))
}

fn filament_or_printer_percent(
    map: &serde_json::Map<String, Value>,
    filament: &str,
    printer: &str,
) -> Option<f64> {
    percent(map, filament).or_else(|| percent(map, printer))
}

fn float_or_percent(map: &serde_json::Map<String, Value>, key: &str) -> Option<(f64, bool)> {
    let raw = text(map, key)?;
    let trimmed = raw.trim();
    if let Some(p) = trimmed.strip_suffix('%') {
        return p.trim().parse().ok().map(|v| (v, true));
    }
    trimmed.parse().ok().map(|v| (v, false))
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
    use std::collections::BTreeMap;

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
                "sparse_infill_pattern": "gyroid",
                "wall_generator": "arachne",
                "min_feature_size": "25%",
                "min_bead_width": "85%"
            }"#,
        )
        .unwrap();
        let s = load_bbl_process(&child).unwrap();
        assert_eq!(s.wall_loops, 2);
        assert_eq!(s.top_shell_layers, 5);
        assert!((s.infill_density - 0.15).abs() < 1e-9);
        assert_eq!(s.infill_pattern, InfillPattern::Gyroid);
        assert!((s.brim_width_mm - 5.0).abs() < 1e-9);
        assert_eq!(s.wall_generator, crate::WallGenerator::Arachne);
        assert!((s.min_feature_size - 0.25).abs() < 1e-9);
        assert!((s.min_bead_width - 0.85).abs() < 1e-9);
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
        assert_eq!(s.support_type, crate::SupportType::Tree);
        assert_eq!(s.ironing_type, crate::IroningType::NoIroning);
        assert!((s.ironing_flow - 0.10).abs() < 1e-9);
        assert!((s.elephant_foot_mm - 0.15).abs() < 1e-9);
        assert!(!s.precise_z_height);
        assert_eq!(s.top_surface_pattern, crate::SurfacePattern::MonotonicLine);
        assert_eq!(s.bottom_surface_pattern, crate::SurfacePattern::Monotonic);
        assert_eq!(s.raft_layers, 0);
        assert_eq!(s.top_one_wall, crate::TopOneWallType::AllTop);
        assert_eq!(s.fuzzy_skin, crate::FuzzySkinType::None);
        assert_eq!(s.wall_generator, crate::WallGenerator::Classic);
        assert!((s.min_feature_size - 0.25).abs() < 1e-9);
        assert!((s.min_bead_width - 0.85).abs() < 1e-9);
        assert!(s.small_perimeter_speed_is_percent);
        assert!((s.small_perimeter_speed - 50.0).abs() < 1e-9);
        assert!(s.small_perimeter_threshold_mm.abs() < 1e-9);
        assert!((s.small_perimeter_speed_mm_s() - 100.0).abs() < 1e-9);
        assert!((s.support_speed_mm_s - 150.0).abs() < 1e-9);
        assert!((s.support_interface_speed_mm_s - 80.0).abs() < 1e-9);
        assert!((s.gap_infill_speed_mm_s - 250.0).abs() < 1e-9);
        let baked = SliceSettings::bbl_0_20();
        assert_eq!(baked.top_shell_layers, s.top_shell_layers);
        assert_eq!(baked.wall_loops, s.wall_loops);
        assert_eq!(baked.infill_pattern, s.infill_pattern);
        assert_eq!(baked.top_surface_pattern, s.top_surface_pattern);
        assert_eq!(baked.bottom_surface_pattern, s.bottom_surface_pattern);
        assert!((baked.infill_density - s.infill_density).abs() < 1e-9);
        assert!((baked.brim_width_mm - s.brim_width_mm).abs() < 1e-9);
        assert_eq!(baked.elephant_foot_mm, s.elephant_foot_mm);
        assert_eq!(baked.top_one_wall, s.top_one_wall);
        assert_eq!(baked.support_type, crate::SupportType::Tree);
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
        assert!((s.default_acceleration_mm_s2 - 10000.0).abs() < 1.0);
    }

    #[test]
    fn generic_pla_sets_part_cooling() {
        let paths = bbl_oracle_paths().expect("upstream BambuStudio profiles");
        let mut s = SliceSettings::default();
        overlay_bbl_profile(&mut s, &paths.filament).unwrap();
        assert_eq!(s.fan_min_speed, 100);
        assert_eq!(s.fan_max_speed, 100);
        assert_eq!(s.close_fan_the_first_x_layers, 1);
        assert!(s.reduce_fan_stop_start_freq);
        assert!((s.fan_cooling_layer_time_s - 100.0).abs() < 1e-9);
        assert!((s.slow_down_layer_time_s - 8.0).abs() < 1e-9);
        assert!((s.filament_density_g_cm3 - 1.24).abs() < 1e-9);
        assert!((s.filament_max_volumetric_speed_mm3_s - 12.0).abs() < 1e-9);
        assert!((s.flow_ratio - 0.98).abs() < 1e-9);
        assert!(s.slow_down_for_layer_cooling);
        assert!((s.slow_down_min_speed_mm_s - 20.0).abs() < 1e-9);
        assert_eq!(s.overhang_fan_speed, 100);
        assert_eq!(
            s.overhang_fan_threshold,
            crate::OverhangFanThreshold::ThreeFour
        );
        assert_eq!(s.ironing_fan_speed, -1);
    }

    #[test]
    fn p1s_machine_sets_retraction() {
        let paths = bbl_oracle_paths().expect("upstream BambuStudio profiles");
        let mut s = SliceSettings::default();
        overlay_bbl_profile(&mut s, &paths.machine).unwrap();
        assert!((s.retraction_length_mm - 0.8).abs() < 1e-9);
        assert!((s.retraction_speed_mm_s - 30.0).abs() < 1e-9);
        assert!((s.deretraction_speed_mm_s - 30.0).abs() < 1e-9);
        assert!((s.retraction_minimum_travel_mm - 1.0).abs() < 1e-9);
        assert!(s.retract_when_changing_layer);
        assert!(s.wipe);
        assert!((s.wipe_distance_mm - 2.0).abs() < 1e-9);
        assert!(s.retract_before_wipe.abs() < 1e-9);
        let baked = SliceSettings::bbl_0_20();
        assert!((baked.retraction_length_mm - 0.8).abs() < 1e-9);
        assert!(baked.wipe);
        assert!(baked.retract_when_changing_layer);
        assert!((baked.retraction_minimum_travel_mm - 1.0).abs() < 1e-9);
    }

    #[test]
    fn project_settings_json_roundtrip() {
        let mut src = SliceSettings::default();
        src.layer_height_mm = 0.28;
        src.infill_density = 0.15;
        src.wall_loops = 3;
        src.enable_support = true;
        src.support_type = crate::SupportType::Tree;
        src.ironing_type = crate::IroningType::TopSurfaces;
        let json = crate::project_settings_json(&src).unwrap();
        assert!(json.contains("\"from\": \"project\""));
        let loaded = crate::settings_from_json(&json).unwrap();
        assert!((loaded.layer_height_mm - 0.28).abs() < 1e-9);
        assert!((loaded.infill_density - 0.15).abs() < 1e-9);
        assert_eq!(loaded.wall_loops, 3);
        assert!(loaded.enable_support);
        assert_eq!(loaded.support_type, crate::SupportType::Tree);
        assert_eq!(loaded.ironing_type, crate::IroningType::TopSurfaces);
        assert!((loaded.default_acceleration_mm_s2 - 10000.0).abs() < 1e-9);
        assert!((loaded.filament_density_g_cm3 - 1.24).abs() < 1e-9);
        assert!(loaded.small_perimeter_speed_is_percent);
        assert!((loaded.small_perimeter_speed - 50.0).abs() < 1e-9);
        assert_eq!(loaded.fan_min_speed, 20);
        assert!(!loaded.reduce_fan_stop_start_freq);
        assert!(!loaded.slow_down_for_layer_cooling);
        assert!((loaded.flow_ratio - 1.0).abs() < 1e-9);
    }

    #[test]
    fn region_overrides_skip_object_keys() {
        let mut s = SliceSettings::default();
        let mut pairs = BTreeMap::new();
        pairs.insert("sparse_infill_density".into(), "100%".into());
        pairs.insert("wall_loops".into(), "6".into());
        pairs.insert("layer_height".into(), "0.08".into());
        pairs.insert("enable_support".into(), "1".into());
        apply_config_pairs(&mut s, &pairs, true);
        assert!((s.infill_density - 1.0).abs() < 1e-9);
        assert_eq!(s.wall_loops, 6);
        assert!((s.layer_height_mm - 0.2).abs() < 1e-9);
        assert!(!s.enable_support);
        apply_config_pairs(&mut s, &pairs, false);
        assert!((s.layer_height_mm - 0.08).abs() < 1e-9);
        assert!(s.enable_support);
    }
}
