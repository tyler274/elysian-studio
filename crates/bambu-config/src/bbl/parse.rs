//! Parse Bambu JSON / 3MF key-values onto [`SliceSettings`].

use std::collections::BTreeMap;

use serde_json::Value;

use crate::{
    EnsureVerticalShellThickness, FilamentMetalStickiness, FuzzySkinType, InfillPattern,
    IroningPattern, IroningType, OverhangFanThreshold, ReduceInfillRetractionMode, SeamPosition,
    SliceSettings, SupportBasePattern, SupportInterfacePattern, SupportType, SurfacePattern,
    TopOneWallType, WallGenerator, WallSequence, ZHopType,
};

/// C++ `PrintRegionConfig` keys (volume / modifier metadata). Object-level
/// keys such as `layer_height` and `enable_support` are ignored here.
pub fn is_region_key(key: &str) -> bool {
    matches!(
        key,
        "line_width"
            | "outer_wall_line_width"
            | "inner_wall_line_width"
            | "sparse_infill_line_width"
            | "internal_solid_infill_line_width"
            | "top_surface_line_width"
            | "wall_loops"
            | "alternate_extra_wall"
            | "only_one_wall_top"
            | "top_one_wall_type"
            | "only_one_wall_first_layer"
            | "sparse_infill_density"
            | "sparse_infill_pattern"
            | "fill_multiline"
            | "infill_combination"
            | "infill_direction"
            | "symmetric_infill_y_axis"
            | "minimum_sparse_infill_area"
            | "infill_wall_overlap"
            | "seam_position"
            | "seam_gap"
            | "wall_generator"
            | "wall_sequence"
            | "wall_infill_order"
            | "precise_outer_wall"
            | "min_feature_size"
            | "min_bead_width"
            | "fuzzy_skin"
            | "fuzzy_skin_thickness"
            | "fuzzy_skin_point_distance"
            | "fuzzy_skin_first_layer"
            | "bottom_shell_layers"
            | "top_shell_layers"
            | "top_shell_thickness"
            | "bottom_shell_thickness"
            | "ensure_vertical_shell_thickness"
            | "top_surface_pattern"
            | "bottom_surface_pattern"
            | "internal_solid_infill_pattern"
            | "top_surface_density"
            | "bottom_surface_density"
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
            | "bridge_flow"
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
            | "extruder"
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

pub(super) fn settings_from_map(map: &serde_json::Map<String, Value>) -> SliceSettings {
    let mut s = SliceSettings::default();
    apply_map_onto(&mut s, map);
    s
}

pub(super) fn apply_map_onto(s: &mut SliceSettings, map: &serde_json::Map<String, Value>) {
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
    if let Some(v) = num(map, "initial_layer_line_width") {
        s.initial_layer_line_width_mm = v.max(0.0);
    }
    if let Some(v) = num(map, "initial_layer_infill_line_width") {
        s.initial_layer_infill_line_width_mm = v.max(0.0);
    }
    if let Some(v) = num(map, "outer_wall_line_width") {
        s.outer_wall_line_width_mm = v.max(0.0);
    }
    if let Some(v) = num(map, "inner_wall_line_width") {
        s.inner_wall_line_width_mm = v.max(0.0);
    }
    if let Some(v) = num(map, "sparse_infill_line_width") {
        s.sparse_infill_line_width_mm = v.max(0.0);
    }
    if let Some(v) = num(map, "internal_solid_infill_line_width") {
        s.internal_solid_infill_line_width_mm = v.max(0.0);
    }
    if let Some(v) = num(map, "top_surface_line_width") {
        s.top_surface_line_width_mm = v.max(0.0);
    }
    if let Some(v) = num(map, "support_line_width") {
        s.support_line_width_mm = v.max(0.0);
    }
    if let Some(v) = nums(map, "nozzle_diameter") {
        s.nozzle_diameters_mm = v;
        if let Some(&first) = s.nozzle_diameters_mm.first() {
            s.nozzle_diameter_mm = first;
        }
    }
    if let Some(v) = u32_val(map, "wall_loops") {
        s.wall_loops = v.max(1);
    }
    if let Some(v) = bool_val(map, "alternate_extra_wall") {
        s.alternate_extra_wall = v;
    }
    if let Some(v) = i32_val(map, "extruder") {
        if v != 0 {
            s.wall_filament = v;
            s.sparse_infill_filament = v;
            s.solid_infill_filament = v;
        }
    }
    if let Some(v) = i32_val(map, "wall_filament") {
        if v > 0 {
            s.wall_filament = v;
        }
    }
    if let Some(v) = i32_val(map, "sparse_infill_filament") {
        if v > 0 {
            s.sparse_infill_filament = v;
        }
    }
    if let Some(v) = i32_val(map, "solid_infill_filament") {
        if v > 0 {
            s.solid_infill_filament = v;
        }
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
    if let Some(v) = bool_val(map, "only_one_wall_first_layer") {
        s.only_one_wall_first_layer = v;
    }
    if let Some(v) = percent(map, "sparse_infill_density") {
        s.infill_density = v;
    }
    if let Some(name) = text(map, "sparse_infill_pattern") {
        if let Some(p) = InfillPattern::from_name(&name) {
            s.infill_pattern = p;
        }
    }
    if let Some(v) = u32_val(map, "fill_multiline") {
        s.fill_multiline = v.clamp(1, 5);
    }
    if let Some(v) = bool_val(map, "infill_combination") {
        s.infill_combination = v;
    }
    if let Some(v) = num(map, "infill_direction") {
        s.infill_direction_deg = v.rem_euclid(360.0);
    }
    if let Some(v) = bool_val(map, "symmetric_infill_y_axis") {
        s.symmetric_infill_y_axis = v;
    }
    if let Some(v) = num(map, "minimum_sparse_infill_area") {
        s.minimum_sparse_infill_area_mm2 = v.max(0.0);
    }
    if let Some(v) = percent(map, "infill_wall_overlap") {
        s.infill_wall_overlap = v.clamp(0.0, 1.0);
    }
    if let Some(name) = text(map, "seam_position") {
        if let Some(p) = SeamPosition::from_name(&name) {
            s.seam = p;
        }
    }
    if let Some(v) = bool_val(map, "seam_placement_away_from_overhangs") {
        s.seam_placement_away_from_overhangs = v;
    }
    if let Some(v) = percent(map, "seam_gap") {
        s.seam_gap = v.max(0.0);
    }
    if let Some(name) = text(map, "wall_generator") {
        if let Some(g) = WallGenerator::from_name(&name) {
            s.wall_generator = g;
        }
    }
    // C++ `handle_legacy`: `wall_infill_order` remaps to `wall_sequence`.
    // Config.cpp also sets `is_infill_first` when the legacy value starts with infill.
    if let Some(name) = text(map, "wall_infill_order") {
        if let Some(seq) = WallSequence::from_name(&name) {
            s.wall_sequence = seq;
        }
        if name.trim().to_ascii_lowercase().starts_with("infill/") {
            s.is_infill_first = true;
        }
    }
    if let Some(name) = text(map, "wall_sequence") {
        if let Some(seq) = WallSequence::from_name(&name) {
            s.wall_sequence = seq;
        }
    }
    if let Some(v) = bool_val(map, "precise_outer_wall") {
        s.precise_outer_wall = v;
    }
    if let Some(v) = bool_val(map, "is_infill_first") {
        s.is_infill_first = v;
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
    if let Some(v) = u32_val(map, "skirt_height") {
        s.skirt_height = v;
    }
    if let Some(name) = text(map, "draft_shield") {
        if let Some(d) = crate::DraftShield::from_name(&name) {
            s.draft_shield = d;
        }
    }
    if let Some(v) = bool_val(map, "ooze_prevention") {
        s.ooze_prevention = v;
    }
    if let Some(v) = num(map, "skirt_distance") {
        s.skirt_distance_mm = v;
    }
    if let Some(v) = num(map, "brim_width") {
        s.brim_width_mm = v;
    }
    if let Some(v) = num(map, "brim_object_gap") {
        s.brim_object_gap_mm = v.max(0.0);
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
    if let Some(v) = bool_val(map, "support_on_build_plate_only") {
        s.support_on_build_plate_only = v;
    }
    if let Some(v) = num(map, "max_bridge_length") {
        s.max_bridge_length_mm = v.max(0.0);
    }
    if let Some(v) = bool_val(map, "bridge_no_support") {
        s.bridge_no_support = v;
    }
    if let Some(v) = bool_val(map, "support_remove_small_overhang") {
        s.support_remove_small_overhang = v;
    }
    if let Some(v) = bool_val(map, "support_critical_regions_only") {
        s.support_critical_regions_only = v;
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
    if let Some(v) = i32_val(map, "tree_support_wall_count") {
        s.tree_support_wall_count = v.clamp(-1, 2);
    }
    if let Some(v) = bool_val(map, "interface_shells") {
        s.interface_shells = v;
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
    if let Some(v) = bool_val(map, "support_interface_loop_pattern") {
        s.support_interface_loop_pattern = v;
    }
    if let Some(name) = text(map, "support_base_pattern") {
        if let Some(p) = SupportBasePattern::from_name(&name) {
            s.support_base_pattern = p;
        }
    }
    if let Some(v) = num(map, "support_base_pattern_spacing") {
        s.support_base_pattern_spacing_mm = v.max(0.0);
    }
    if let Some(name) = text(map, "support_interface_pattern") {
        if let Some(p) = SupportInterfacePattern::from_name(&name) {
            s.support_interface_pattern = p;
        }
    }
    if let Some(v) = num(map, "support_interface_spacing") {
        s.support_interface_spacing_mm = v.max(0.0);
    }
    if let Some(v) = num(map, "support_angle") {
        s.support_angle_deg = v.rem_euclid(360.0);
    }
    if let Some(v) = bool_val(map, "enable_support_ironing") {
        s.enable_support_ironing = v;
    }
    if let Some(name) = text(map, "support_ironing_pattern") {
        if let Some(p) = IroningPattern::from_name(&name) {
            s.support_ironing_pattern = p;
        }
    }
    if let Some(v) = num(map, "support_ironing_spacing") {
        s.support_ironing_spacing_mm = v.max(0.0);
    }
    if let Some(v) = num(map, "support_ironing_inset") {
        s.support_ironing_inset_mm = v.max(0.0);
    }
    if let Some(v) = num(map, "support_expansion") {
        s.support_expansion_mm = v;
    }
    if let Some(v) = u32_val(map, "bottom_shell_layers") {
        s.bottom_shell_layers = v;
    }
    if let Some(v) = u32_val(map, "top_shell_layers") {
        s.top_shell_layers = v;
    }
    if let Some(v) = num(map, "top_shell_thickness") {
        s.top_shell_thickness_mm = v.max(0.0);
    }
    if let Some(v) = num(map, "bottom_shell_thickness") {
        s.bottom_shell_thickness_mm = v.max(0.0);
    }
    if let Some(name) = text(map, "ensure_vertical_shell_thickness") {
        if let Some(level) = EnsureVerticalShellThickness::from_name(&name) {
            s.ensure_vertical_shell_thickness = level;
        }
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
    if let Some(name) = text(map, "internal_solid_infill_pattern") {
        if let Some(p) = SurfacePattern::from_name(&name) {
            s.internal_solid_infill_pattern = p;
        }
    }
    if let Some(v) = percent(map, "top_surface_density") {
        s.top_surface_density = v.clamp(0.0, 1.0);
    }
    if let Some(v) = percent(map, "bottom_surface_density") {
        s.bottom_surface_density = v.clamp(0.0, 1.0);
    }
    if let Some(v) = bool_val(map, "detect_narrow_internal_solid_infill") {
        s.detect_narrow_internal_solid_infill = v;
    }
    if let Some(v) = bool_val(map, "detect_floating_vertical_shell") {
        s.detect_floating_vertical_shell = v;
    }
    if let Some((v, is_percent)) = float_or_percent(map, "vertical_shell_speed") {
        s.vertical_shell_speed = v.max(0.0);
        s.vertical_shell_speed_is_percent = is_percent;
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
    if let Some(v) = num(map, "bridge_flow") {
        s.bridge_flow = v.max(0.0);
    }
    if let Some(v) = bool_val(map, "thick_bridges") {
        s.thick_bridges = v;
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
    if let Some(name) = text(map, "reduce_infill_retraction_mode") {
        if let Some(m) = ReduceInfillRetractionMode::from_name(&name) {
            s.reduce_infill_retraction_mode = m;
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
    if let Some(v) = num(map, "outer_wall_acceleration") {
        s.outer_wall_acceleration_mm_s2 = v.max(0.0);
    }
    if let Some(v) = num(map, "inner_wall_acceleration") {
        s.inner_wall_acceleration_mm_s2 = v.max(0.0);
    }
    if let Some(v) = num(map, "initial_layer_acceleration") {
        s.initial_layer_acceleration_mm_s2 = v.max(0.0);
    }
    if let Some(v) = num(map, "top_surface_acceleration") {
        s.top_surface_acceleration_mm_s2 = v.max(0.0);
    }
    if let Some((v, is_pct)) = float_or_percent(map, "sparse_infill_acceleration") {
        s.sparse_infill_acceleration = v.max(0.0);
        s.sparse_infill_acceleration_is_percent = is_pct;
    }
    if let Some(v) = num(map, "travel_acceleration") {
        s.travel_acceleration_mm_s2 = v.max(0.0);
    }
    if let Some(v) = num(map, "initial_layer_travel_acceleration") {
        s.initial_layer_travel_acceleration_mm_s2 = v.max(0.0);
    }
    if let Some(v) = num(map, "travel_short_distance_acceleration") {
        s.travel_short_distance_acceleration_mm_s2 = v.max(0.0);
    }
    if let Some(v) = num(map, "machine_max_acceleration_retracting") {
        s.retract_acceleration_mm_s2 = v.max(0.0);
    }
    overlay_machine_limit(
        &mut s.machine_limits.acceleration_x_mm_s2,
        map,
        "machine_max_acceleration_x",
    );
    overlay_machine_limit(
        &mut s.machine_limits.acceleration_y_mm_s2,
        map,
        "machine_max_acceleration_y",
    );
    overlay_machine_limit(
        &mut s.machine_limits.acceleration_z_mm_s2,
        map,
        "machine_max_acceleration_z",
    );
    overlay_machine_limit(
        &mut s.machine_limits.acceleration_e_mm_s2,
        map,
        "machine_max_acceleration_e",
    );
    overlay_machine_limit(
        &mut s.machine_limits.acceleration_extruding_mm_s2,
        map,
        "machine_max_acceleration_extruding",
    );
    overlay_machine_limit(
        &mut s.machine_limits.acceleration_travel_mm_s2,
        map,
        "machine_max_acceleration_travel",
    );
    overlay_machine_limit(
        &mut s.machine_limits.speed_x_mm_s,
        map,
        "machine_max_speed_x",
    );
    overlay_machine_limit(
        &mut s.machine_limits.speed_y_mm_s,
        map,
        "machine_max_speed_y",
    );
    overlay_machine_limit(
        &mut s.machine_limits.speed_z_mm_s,
        map,
        "machine_max_speed_z",
    );
    overlay_machine_limit(
        &mut s.machine_limits.speed_e_mm_s,
        map,
        "machine_max_speed_e",
    );
    if let Some(v) = num(map, "filament_density") {
        s.filament_density_g_cm3 = v.max(0.0);
    }
    if let Some(name) = text(map, "filament_metal_stickiness") {
        if let Some(t) = FilamentMetalStickiness::from_name(&name) {
            s.filament_metal_stickiness = t;
        }
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
    if let Some(v) = bool_val(map, "auxiliary_fan") {
        s.auxiliary_fan = v;
    }
    if let Some(v) = u32_val(map, "additional_cooling_fan_speed") {
        s.additional_cooling_fan_speed = v.min(100);
    }
    if let Some(v) = u32_val(map, "close_additional_fan_first_x_layers") {
        s.close_additional_fan_first_x_layers = v;
    }
    if let Some(v) = u32_val(map, "additional_fan_full_speed_layer") {
        s.additional_fan_full_speed_layer = v;
    }
    if let Some(v) = u32_val(map, "first_x_layer_fan_speed") {
        s.first_x_layer_fan_speed = v.min(100);
    }
    if let Some(v) = num(map, "pre_start_fan_time") {
        s.pre_start_fan_time_s = v.max(0.0);
    }
    if let Some(v) = bool_val(map, "support_air_filtration") {
        s.support_air_filtration = v;
    }
    if let Some(v) = bool_val(map, "activate_air_filtration") {
        s.activate_air_filtration = v;
    }
    if let Some(v) = u32_val(map, "during_print_exhaust_fan_speed") {
        s.during_print_exhaust_fan_speed = v.min(100);
    }
    if let Some(v) = u32_val(map, "complete_print_exhaust_fan_speed") {
        s.complete_print_exhaust_fan_speed = v.min(100);
    }
    if let Some(v) = bool_val(map, "slow_down_for_layer_cooling") {
        s.slow_down_for_layer_cooling = v;
    }
    if let Some(v) = bool_val(map, "no_slow_down_for_cooling_on_outwalls") {
        s.no_slow_down_for_cooling_on_outwalls = v;
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
    if let Some(v) = num(map, "machine_max_jerk_e") {
        s.e_jerk_mm_s = v.max(0.0);
    }
    if let Some(name) = text(map, "gcode_flavor") {
        if let Some(flavor) = crate::GCodeFlavor::from_name(&name) {
            s.gcode_flavor = flavor;
        }
    }
    if let Some(v) = text(map, "layer_change_gcode") {
        s.layer_change_gcode = v;
    }
    if let Some(v) = text(map, "machine_end_gcode") {
        s.machine_end_gcode = v;
    }
    if let Some(v) = text(map, "filament_end_gcode") {
        s.filament_end_gcode = v;
    }
    if let Some(v) = filament_or_printer_bool(
        map,
        "filament_long_retractions_when_cut",
        "long_retractions_when_cut",
    ) {
        s.long_retraction_when_cut = v;
    }
    if let Some(v) = filament_or_printer_bool(
        map,
        "filament_long_retractions_when_ec",
        "long_retractions_when_ec",
    ) {
        s.long_retraction_when_ec = v;
    }
    if let Some(v) = filament_or_printer(
        map,
        "filament_retraction_distances_when_cut",
        "retraction_distances_when_cut",
    ) {
        s.retraction_distance_when_cut = v.max(0.0);
    }
    if let Some(v) = filament_or_printer(
        map,
        "filament_retraction_distances_when_ec",
        "retraction_distances_when_ec",
    ) {
        s.retraction_distance_when_ec = v.max(0.0);
    }
    if let Some(v) = text(map, "machine_start_gcode") {
        s.machine_start_gcode = v;
    }
    if let Some(v) = text(map, "filament_start_gcode") {
        s.filament_start_gcode = v;
    }
    if let Some(v) = text(map, "time_lapse_gcode") {
        s.time_lapse_gcode = v;
    }
    if let Some(v) = num(map, "timelapse_type") {
        s.timelapse_type = v.round().clamp(0.0, 1.0) as u8;
    }
    if let Some(v) = bool_val(map, "farthest_point_timelapse") {
        s.farthest_point_timelapse = v;
    }
    if let Some(v) = bool_val(map, "spiral_mode") {
        s.spiral_mode = v;
    }
    if let Some(v) = bool_val(map, "enable_arc_fitting") {
        s.enable_arc_fitting = v;
    }
    if let Some(v) = num(map, "resolution") {
        s.resolution_mm = v.max(0.001);
    }
    if let Some(v) = bool_val(map, "enable_wrapping_detection") {
        s.enable_wrapping_detection = v;
    }
    if let Some(v) = bool_val(map, "enable_prime_tower") {
        s.enable_prime_tower = v;
    }
    if let Some(v) = num(map, "wipe_tower_x") {
        s.wipe_tower_x_mm = v;
    }
    if let Some(v) = num(map, "wipe_tower_y") {
        s.wipe_tower_y_mm = v;
    }
    if let Some(v) = num(map, "prime_tower_width") {
        s.prime_tower_width_mm = v.max(0.0);
    }
    if let Some(v) = num(map, "prime_tower_brim_width") {
        s.prime_tower_brim_width_mm = v;
    }
    if let Some(v) = nums(map, "filament_diameter") {
        s.filament_count = v.len().max(1);
        if let Some(&d) = v.first() {
            s.filament_diameter_mm = d.max(0.0);
        }
    }
    if let Some(v) = text(map, "wrapping_detection_gcode") {
        s.wrapping_detection_gcode = v;
    }
    if let Some(v) = ints(map, "filament_map") {
        s.filament_map = v;
    }
    if let Some(v) = ints(map, "physical_extruder_map") {
        s.physical_extruder_map = v;
    }
    if let Some(v) = bool_val(map, "scan_first_layer") {
        s.scan_first_layer = v;
    }
    if let Some(v) = text(map, "printer_structure") {
        s.printer_structure = v;
    }
    if let Some(v) = text(map, "print_sequence") {
        s.print_sequence = v;
    }
    if let Some(v) = num(map, "printable_height") {
        s.printable_height_mm = v.max(0.0);
    }
    if let Some(v) = map.get("printable_area") {
        let polys = polygons_from_area_value(v);
        if let Some(poly) = polys.into_iter().next() {
            s.printable_area = poly;
        }
    }
    if let Some(v) = map.get("extruder_printable_area") {
        let polys = polygons_from_area_value(v);
        if !polys.is_empty() {
            s.extruder_printable_areas = polys;
        }
    }
    if let Some(v) = map.get("bed_exclude_area") {
        if let Some(poly) = polygons_from_area_value(v).into_iter().next() {
            s.bed_exclude_area = poly;
        }
    }
    if let Some(v) = num(map, "extruder_clearance_max_radius") {
        s.extruder_clearance_max_radius_mm = v.max(0.0);
    }
    if let Some(v) = text(map, "filament_type") {
        s.filament_type = v;
    }
    if let Some(v) = text(map, "filament_vendor") {
        s.filament_vendor = v;
    }
    if let Some(v) = num(map, "chamber_temperatures") {
        s.chamber_temperature_c = v.round() as i32;
    }
    if let Some(v) = num(map, "temperature_vitrification") {
        s.temperature_vitrification_c = v.round() as i32;
    }
    if let Some(v) = bool_val(map, "cooling_filter_enabled") {
        s.cooling_filter_enabled = v;
    }
    if let Some(v) = text(map, "curr_bed_type") {
        s.curr_bed_type = v;
    }
    overlay_temp_c(&mut s.temperature_c, map, "nozzle_temperature");
    overlay_temp_c(
        &mut s.temperature_initial_layer_c,
        map,
        "nozzle_temperature_initial_layer",
    );
    overlay_plate(map, &mut s.cool_plate, "cool_plate_temp");
    overlay_plate(map, &mut s.eng_plate, "eng_plate_temp");
    overlay_plate(map, &mut s.hot_plate, "hot_plate_temp");
    overlay_plate(map, &mut s.textured_plate, "textured_plate_temp");
    overlay_plate(map, &mut s.supertack_plate, "supertack_plate_temp");
    s.resolve_bed_temps_from_plate();
    if let Some(v) = num(map, "nozzle_temperature_range_high") {
        s.nozzle_temperature_range_high = v.round().clamp(0.0, 500.0) as u16;
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
    if let Some(v) = filament_or_printer(map, "filament_z_hop", "z_hop") {
        s.z_hop_mm = v.max(0.0);
    }
    if let Some(name) = filament_or_printer_text(map, "filament_z_hop_types", "z_hop_types") {
        if let Some(t) = ZHopType::from_name(&name) {
            s.z_hop_type = t;
        }
    }
    if let Some(v) = num(map, "retract_lift_above") {
        s.retract_lift_above_mm = v.max(0.0);
    }
    if let Some(v) = num(map, "retract_lift_below") {
        s.retract_lift_below_mm = v.max(0.0);
    }
    if let Some(v) = num(map, "travel_speed_z") {
        s.travel_speed_z_mm_s = v.max(0.0);
    }
    if let Some((min_x, min_y, max_x, max_y)) = bed_bbox_from_map(map) {
        s.bed_bbox_valid = true;
        s.bed_min_x = min_x;
        s.bed_min_y = min_y;
        s.bed_max_x = max_x;
        s.bed_max_y = max_y;
    }
}

fn text(map: &serde_json::Map<String, Value>, key: &str) -> Option<String> {
    map.get(key).and_then(value_text)
}

fn ints(map: &serde_json::Map<String, Value>, key: &str) -> Option<Vec<i32>> {
    match map.get(key)? {
        Value::Array(items) => {
            let v: Vec<i32> = items.iter().filter_map(int_from_value).collect();
            (!v.is_empty()).then_some(v)
        }
        other => {
            let raw = value_text(other)?;
            let v: Vec<i32> = raw
                .split([',', ' '])
                .filter(|p| !p.is_empty())
                .filter_map(|p| p.parse().ok())
                .collect();
            (!v.is_empty()).then_some(v)
        }
    }
}

fn nums(map: &serde_json::Map<String, Value>, key: &str) -> Option<Vec<f64>> {
    match map.get(key)? {
        Value::Array(items) => {
            let v: Vec<f64> = items.iter().filter_map(num_from_value).collect();
            (!v.is_empty()).then_some(v)
        }
        other => {
            let raw = value_text(other)?;
            let v: Vec<f64> = raw
                .split([',', ';', ' '])
                .filter(|p| !p.is_empty())
                .filter_map(|p| p.trim_end_matches('%').trim().parse().ok())
                .collect();
            (!v.is_empty()).then_some(v)
        }
    }
}

fn num_from_value(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim_end_matches('%').trim().parse().ok(),
        _ => None,
    }
}

fn int_from_value(v: &Value) -> Option<i32> {
    match v {
        Value::Number(n) => n
            .as_i64()
            .map(|i| i as i32)
            .or_else(|| n.as_f64().map(|f| f.round() as i32)),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

pub(super) fn value_text(v: &Value) -> Option<String> {
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

fn overlay_temp_c(dst: &mut u16, map: &serde_json::Map<String, Value>, key: &str) {
    if let Some(v) = num(map, key) {
        *dst = v.round().clamp(0.0, 500.0) as u16;
    }
}

fn overlay_machine_limit(dst: &mut f64, map: &serde_json::Map<String, Value>, key: &str) {
    if let Some(v) = num(map, key) {
        *dst = v.max(0.0);
    }
}

fn overlay_plate(
    map: &serde_json::Map<String, Value>,
    plate: &mut crate::PlateBedTemps,
    later_key: &str,
) {
    overlay_temp_c(&mut plate.later_c, map, later_key);
    overlay_temp_c(
        &mut plate.initial_c,
        map,
        &format!("{later_key}_initial_layer"),
    );
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

fn filament_or_printer_text(
    map: &serde_json::Map<String, Value>,
    filament: &str,
    printer: &str,
) -> Option<String> {
    text(map, filament)
        .filter(|s| s != "nil")
        .or_else(|| text(map, printer))
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

fn i32_val(map: &serde_json::Map<String, Value>, key: &str) -> Option<i32> {
    num(map, key).map(|v| v.round() as i32)
}

fn bool_val(map: &serde_json::Map<String, Value>, key: &str) -> Option<bool> {
    let raw = text(map, key)?;
    match raw.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" => Some(true),
        "0" | "false" | "no" => Some(false),
        _ => None,
    }
}

fn parse_xy_list(s: &str) -> Vec<(f64, f64)> {
    s.split([',', ' '])
        .filter(|p| !p.is_empty())
        .filter_map(|p| {
            let mut it = p.trim().split('x');
            let x = it.next()?.parse().ok()?;
            let y = it.next()?.parse().ok()?;
            Some((x, y))
        })
        .collect()
}

/// One polygon per array element when elements are multi-point strings;
/// otherwise the whole array is a single polygon of `NxN` points.
fn polygons_from_area_value(v: &Value) -> Vec<Vec<(f64, f64)>> {
    match v {
        Value::Array(items) => {
            let parts: Vec<Vec<(f64, f64)>> = items.iter().map(xy_points_from_value).collect();
            if parts.iter().any(|p| p.len() >= 3) {
                parts.into_iter().filter(|p| p.len() >= 3).collect()
            } else {
                let flat: Vec<(f64, f64)> = parts.into_iter().flatten().collect();
                if flat.len() >= 3 {
                    vec![flat]
                } else {
                    Vec::new()
                }
            }
        }
        other => {
            let pts = xy_points_from_value(other);
            if pts.len() >= 3 {
                vec![pts]
            } else {
                Vec::new()
            }
        }
    }
}

fn xy_points_from_value(v: &Value) -> Vec<(f64, f64)> {
    match v {
        Value::String(s) => parse_xy_list(s),
        Value::Array(a) => a.iter().flat_map(xy_points_from_value).collect(),
        _ => Vec::new(),
    }
}

fn bbox_of(pts: &[(f64, f64)]) -> Option<(f64, f64, f64, f64)> {
    if pts.len() < 3 {
        return None;
    }
    let mut min_x = f64::INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    for &(x, y) in pts {
        min_x = min_x.min(x);
        min_y = min_y.min(y);
        max_x = max_x.max(x);
        max_y = max_y.max(y);
    }
    Some((min_x, min_y, max_x, max_y))
}

/// Prefer the first extruder's printable area, else the whole-bed polygon.
fn bed_bbox_from_map(map: &serde_json::Map<String, Value>) -> Option<(f64, f64, f64, f64)> {
    if let Some(v) = map.get("extruder_printable_area") {
        let pts = match v {
            Value::Array(a) => a.first().map(xy_points_from_value).unwrap_or_default(),
            other => xy_points_from_value(other),
        };
        if let Some(bb) = bbox_of(&pts) {
            return Some(bb);
        }
    }
    map.get("printable_area")
        .and_then(|v| bbox_of(&xy_points_from_value(v)))
}
