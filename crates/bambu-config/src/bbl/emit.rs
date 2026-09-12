//! Serialize [`SliceSettings`] to Bambu project_settings JSON and CONFIG_BLOCK comments.

use serde_json::Value;

use crate::SliceSettings;

use super::xy::format_xy_list;
use super::ConfigError;

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
    insert(
        &mut map,
        "initial_layer_line_width",
        num_str(settings.initial_layer_line_width_mm),
    );
    insert(
        &mut map,
        "initial_layer_infill_line_width",
        num_str(settings.initial_layer_infill_line_width_mm),
    );
    insert(
        &mut map,
        "outer_wall_line_width",
        num_str(settings.outer_wall_line_width_mm),
    );
    insert(
        &mut map,
        "inner_wall_line_width",
        num_str(settings.inner_wall_line_width_mm),
    );
    insert(
        &mut map,
        "sparse_infill_line_width",
        num_str(settings.sparse_infill_line_width_mm),
    );
    insert(
        &mut map,
        "internal_solid_infill_line_width",
        num_str(settings.internal_solid_infill_line_width_mm),
    );
    insert(
        &mut map,
        "top_surface_line_width",
        num_str(settings.top_surface_line_width_mm),
    );
    insert(
        &mut map,
        "support_line_width",
        num_str(settings.support_line_width_mm),
    );
    insert(&mut map, "wall_loops", settings.wall_loops.to_string());
    insert_bool(
        &mut map,
        "alternate_extra_wall",
        settings.alternate_extra_wall,
    );
    insert(
        &mut map,
        "wall_filament",
        settings.wall_filament.to_string(),
    );
    insert(
        &mut map,
        "sparse_infill_filament",
        settings.sparse_infill_filament.to_string(),
    );
    insert(
        &mut map,
        "solid_infill_filament",
        settings.solid_infill_filament.to_string(),
    );
    insert(
        &mut map,
        "top_one_wall_type",
        settings.top_one_wall.as_str(),
    );
    insert_bool(
        &mut map,
        "only_one_wall_first_layer",
        settings.only_one_wall_first_layer,
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
    insert(
        &mut map,
        "fill_multiline",
        settings.fill_multiline.to_string(),
    );
    insert_bool(&mut map, "infill_combination", settings.infill_combination);
    insert(
        &mut map,
        "infill_direction",
        num_str(settings.infill_direction_deg),
    );
    insert(&mut map, "bridge_angle", num_str(settings.bridge_angle_deg));
    insert_bool(
        &mut map,
        "symmetric_infill_y_axis",
        settings.symmetric_infill_y_axis,
    );
    insert(
        &mut map,
        "minimum_sparse_infill_area",
        num_str(settings.minimum_sparse_infill_area_mm2),
    );
    insert(
        &mut map,
        "infill_wall_overlap",
        pct_str(settings.infill_wall_overlap),
    );
    insert(&mut map, "seam_position", settings.seam.as_str());
    insert_bool(
        &mut map,
        "seam_placement_away_from_overhangs",
        settings.seam_placement_away_from_overhangs,
    );
    insert(&mut map, "seam_gap", pct_str(settings.seam_gap));
    insert(&mut map, "wall_generator", settings.wall_generator.as_str());
    insert(&mut map, "wall_sequence", settings.wall_sequence.as_str());
    insert_bool(&mut map, "precise_outer_wall", settings.precise_outer_wall);
    insert_bool(&mut map, "is_infill_first", settings.is_infill_first);
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
    insert(&mut map, "skirt_height", settings.skirt_height.to_string());
    insert(&mut map, "draft_shield", settings.draft_shield.as_str());
    insert_bool(&mut map, "ooze_prevention", settings.ooze_prevention);
    insert(
        &mut map,
        "skirt_distance",
        num_str(settings.skirt_distance_mm),
    );
    insert(&mut map, "brim_width", num_str(settings.brim_width_mm));
    insert(&mut map, "brim_type", settings.brim_type.as_str());
    insert(
        &mut map,
        "brim_object_gap",
        num_str(settings.brim_object_gap_mm),
    );
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
    insert_bool(
        &mut map,
        "support_on_build_plate_only",
        settings.support_on_build_plate_only,
    );
    insert(
        &mut map,
        "max_bridge_length",
        num_str(settings.max_bridge_length_mm),
    );
    insert_bool(&mut map, "bridge_no_support", settings.bridge_no_support);
    insert_bool(
        &mut map,
        "support_remove_small_overhang",
        settings.support_remove_small_overhang,
    );
    insert_bool(
        &mut map,
        "support_critical_regions_only",
        settings.support_critical_regions_only,
    );
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
        "tree_support_branch_diameter_angle",
        num_str(settings.tree_branch_diameter_angle_deg),
    );
    insert(
        &mut map,
        "tree_support_branch_distance",
        num_str(settings.tree_branch_distance_mm),
    );
    insert(
        &mut map,
        "tree_support_wall_count",
        settings.tree_support_wall_count.to_string(),
    );
    insert_bool(&mut map, "interface_shells", settings.interface_shells);
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
        "support_object_first_layer_gap",
        num_str(settings.support_object_first_layer_gap_mm),
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
    insert_bool(
        &mut map,
        "support_interface_loop_pattern",
        settings.support_interface_loop_pattern,
    );
    insert(
        &mut map,
        "support_base_pattern",
        settings.support_base_pattern.as_str(),
    );
    insert(
        &mut map,
        "support_base_pattern_spacing",
        num_str(settings.support_base_pattern_spacing_mm),
    );
    insert(
        &mut map,
        "support_interface_pattern",
        settings.support_interface_pattern.as_str(),
    );
    insert(
        &mut map,
        "support_interface_spacing",
        num_str(settings.support_interface_spacing_mm),
    );
    insert(
        &mut map,
        "support_angle",
        num_str(settings.support_angle_deg),
    );
    insert_bool(
        &mut map,
        "enable_support_ironing",
        settings.enable_support_ironing,
    );
    insert(
        &mut map,
        "support_ironing_pattern",
        settings.support_ironing_pattern.as_str(),
    );
    insert(
        &mut map,
        "support_ironing_spacing",
        num_str(settings.support_ironing_spacing_mm),
    );
    insert(
        &mut map,
        "support_ironing_inset",
        num_str(settings.support_ironing_inset_mm),
    );
    insert(
        &mut map,
        "support_ironing_direction",
        num_str(settings.support_ironing_direction_deg),
    );
    insert(
        &mut map,
        "support_ironing_flow",
        pct_str(settings.support_ironing_flow),
    );
    insert(
        &mut map,
        "support_ironing_speed",
        num_str(settings.support_ironing_speed_mm_s),
    );
    insert(
        &mut map,
        "support_expansion",
        num_str(settings.support_expansion_mm),
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
        "top_shell_thickness",
        num_str(settings.top_shell_thickness_mm),
    );
    insert(
        &mut map,
        "bottom_shell_thickness",
        num_str(settings.bottom_shell_thickness_mm),
    );
    insert(
        &mut map,
        "ensure_vertical_shell_thickness",
        settings.ensure_vertical_shell_thickness.as_str(),
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
        "internal_solid_infill_pattern",
        settings.internal_solid_infill_pattern.as_str(),
    );
    insert(
        &mut map,
        "sub_top_surface_pattern",
        settings.sub_top_surface_pattern.as_str(),
    );
    insert(
        &mut map,
        "top_surface_density",
        pct_str(settings.top_surface_density),
    );
    insert(
        &mut map,
        "bottom_surface_density",
        pct_str(settings.bottom_surface_density),
    );
    insert_bool(
        &mut map,
        "detect_narrow_internal_solid_infill",
        settings.detect_narrow_internal_solid_infill,
    );
    insert_bool(
        &mut map,
        "detect_floating_vertical_shell",
        settings.detect_floating_vertical_shell,
    );
    insert(
        &mut map,
        "vertical_shell_speed",
        if settings.vertical_shell_speed_is_percent {
            format!("{}%", num_str(settings.vertical_shell_speed))
        } else {
            num_str(settings.vertical_shell_speed)
        },
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
    insert(&mut map, "bridge_flow", num_str(settings.bridge_flow));
    insert(
        &mut map,
        "top_solid_infill_flow_ratio",
        num_str(settings.top_solid_infill_flow_ratio),
    );
    insert(
        &mut map,
        "initial_layer_flow_ratio",
        num_str(settings.initial_layer_flow_ratio),
    );
    insert(
        &mut map,
        "print_flow_ratio",
        num_str(settings.print_flow_ratio),
    );
    insert_bool(&mut map, "thick_bridges", settings.thick_bridges);
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
        "reduce_infill_retraction_mode",
        settings.reduce_infill_retraction_mode.as_str(),
    );
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
        "ironing_direction",
        num_str(settings.ironing_direction_deg),
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
        "outer_wall_acceleration",
        num_str(settings.outer_wall_acceleration_mm_s2),
    );
    insert(
        &mut map,
        "inner_wall_acceleration",
        num_str(settings.inner_wall_acceleration_mm_s2),
    );
    insert(
        &mut map,
        "initial_layer_acceleration",
        num_str(settings.initial_layer_acceleration_mm_s2),
    );
    insert(
        &mut map,
        "top_surface_acceleration",
        num_str(settings.top_surface_acceleration_mm_s2),
    );
    insert(
        &mut map,
        "sparse_infill_acceleration",
        if settings.sparse_infill_acceleration_is_percent {
            format!("{}%", num_str(settings.sparse_infill_acceleration))
        } else {
            num_str(settings.sparse_infill_acceleration)
        },
    );
    insert(
        &mut map,
        "travel_acceleration",
        num_str(settings.travel_acceleration_mm_s2),
    );
    insert(
        &mut map,
        "initial_layer_travel_acceleration",
        num_str(settings.initial_layer_travel_acceleration_mm_s2),
    );
    insert(
        &mut map,
        "travel_short_distance_acceleration",
        num_str(settings.travel_short_distance_acceleration_mm_s2),
    );
    insert(
        &mut map,
        "machine_max_acceleration_retracting",
        num_str(settings.retract_acceleration_mm_s2),
    );
    insert(
        &mut map,
        "machine_max_acceleration_x",
        num_str(settings.machine_limits.acceleration_x_mm_s2),
    );
    insert(
        &mut map,
        "machine_max_acceleration_y",
        num_str(settings.machine_limits.acceleration_y_mm_s2),
    );
    insert(
        &mut map,
        "machine_max_acceleration_z",
        num_str(settings.machine_limits.acceleration_z_mm_s2),
    );
    insert(
        &mut map,
        "machine_max_acceleration_e",
        num_str(settings.machine_limits.acceleration_e_mm_s2),
    );
    insert(
        &mut map,
        "machine_max_acceleration_extruding",
        num_str(settings.machine_limits.acceleration_extruding_mm_s2),
    );
    insert(
        &mut map,
        "machine_max_acceleration_travel",
        num_str(settings.machine_limits.acceleration_travel_mm_s2),
    );
    insert(
        &mut map,
        "machine_max_speed_x",
        num_str(settings.machine_limits.speed_x_mm_s),
    );
    insert(
        &mut map,
        "machine_max_speed_y",
        num_str(settings.machine_limits.speed_y_mm_s),
    );
    insert(
        &mut map,
        "machine_max_speed_z",
        num_str(settings.machine_limits.speed_z_mm_s),
    );
    insert(
        &mut map,
        "machine_max_speed_e",
        num_str(settings.machine_limits.speed_e_mm_s),
    );
    insert(
        &mut map,
        "filament_density",
        num_str(settings.filament_density_g_cm3),
    );
    insert(
        &mut map,
        "filament_metal_stickiness",
        settings.filament_metal_stickiness.as_str(),
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
    insert_bool(&mut map, "auxiliary_fan", settings.auxiliary_fan);
    insert(
        &mut map,
        "additional_cooling_fan_speed",
        settings.additional_cooling_fan_speed.to_string(),
    );
    insert(
        &mut map,
        "close_additional_fan_first_x_layers",
        settings.close_additional_fan_first_x_layers.to_string(),
    );
    insert(
        &mut map,
        "additional_fan_full_speed_layer",
        settings.additional_fan_full_speed_layer.to_string(),
    );
    insert(
        &mut map,
        "first_x_layer_fan_speed",
        settings.first_x_layer_fan_speed.to_string(),
    );
    insert(
        &mut map,
        "pre_start_fan_time",
        num_str(settings.pre_start_fan_time_s),
    );
    insert_bool(
        &mut map,
        "support_air_filtration",
        settings.support_air_filtration,
    );
    insert_bool(
        &mut map,
        "activate_air_filtration",
        settings.activate_air_filtration,
    );
    insert(
        &mut map,
        "during_print_exhaust_fan_speed",
        settings.during_print_exhaust_fan_speed.to_string(),
    );
    insert(
        &mut map,
        "complete_print_exhaust_fan_speed",
        settings.complete_print_exhaust_fan_speed.to_string(),
    );
    insert_bool(
        &mut map,
        "slow_down_for_layer_cooling",
        settings.slow_down_for_layer_cooling,
    );
    insert_bool(
        &mut map,
        "no_slow_down_for_cooling_on_outwalls",
        settings.no_slow_down_for_cooling_on_outwalls,
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
    insert(&mut map, "z_hop", num_str(settings.z_hop_mm));
    insert(&mut map, "z_hop_types", settings.z_hop_type.as_str());
    if !settings.layer_change_gcode.is_empty() {
        insert(
            &mut map,
            "layer_change_gcode",
            settings.layer_change_gcode.clone(),
        );
    }
    if !settings.machine_end_gcode.is_empty() {
        insert(
            &mut map,
            "machine_end_gcode",
            settings.machine_end_gcode.clone(),
        );
    }
    if !settings.filament_end_gcode.is_empty() {
        insert(
            &mut map,
            "filament_end_gcode",
            settings.filament_end_gcode.clone(),
        );
    }
    insert_bool(
        &mut map,
        "long_retractions_when_cut",
        settings.long_retraction_when_cut,
    );
    insert_bool(
        &mut map,
        "long_retractions_when_ec",
        settings.long_retraction_when_ec,
    );
    insert(
        &mut map,
        "retraction_distances_when_cut",
        num_str(settings.retraction_distance_when_cut),
    );
    insert(
        &mut map,
        "retraction_distances_when_ec",
        num_str(settings.retraction_distance_when_ec),
    );
    if !settings.machine_start_gcode.is_empty() {
        insert(
            &mut map,
            "machine_start_gcode",
            settings.machine_start_gcode.clone(),
        );
    }
    if !settings.filament_start_gcode.is_empty() {
        insert(
            &mut map,
            "filament_start_gcode",
            settings.filament_start_gcode.clone(),
        );
    }
    if !settings.time_lapse_gcode.is_empty() {
        insert(
            &mut map,
            "time_lapse_gcode",
            settings.time_lapse_gcode.clone(),
        );
    }
    insert(
        &mut map,
        "timelapse_type",
        settings.timelapse_type.to_string(),
    );
    insert_bool(
        &mut map,
        "farthest_point_timelapse",
        settings.farthest_point_timelapse,
    );
    insert_bool(&mut map, "spiral_mode", settings.spiral_mode);
    insert_bool(&mut map, "enable_arc_fitting", settings.enable_arc_fitting);
    insert(&mut map, "resolution", num_str(settings.resolution_mm));
    insert_bool(
        &mut map,
        "enable_wrapping_detection",
        settings.enable_wrapping_detection,
    );
    insert_bool(&mut map, "enable_prime_tower", settings.enable_prime_tower);
    insert(&mut map, "wipe_tower_x", num_str(settings.wipe_tower_x_mm));
    insert(&mut map, "wipe_tower_y", num_str(settings.wipe_tower_y_mm));
    insert(
        &mut map,
        "prime_tower_width",
        num_str(settings.prime_tower_width_mm),
    );
    insert(
        &mut map,
        "prime_tower_brim_width",
        num_str(settings.prime_tower_brim_width_mm),
    );
    insert(
        &mut map,
        "filament_diameter",
        (0..settings.filament_count.max(1))
            .map(|_| num_str(settings.filament_diameter_mm))
            .collect::<Vec<_>>()
            .join(","),
    );
    insert(
        &mut map,
        "nozzle_diameter",
        settings
            .nozzle_diameters_mm
            .iter()
            .copied()
            .map(num_str)
            .collect::<Vec<_>>()
            .join(","),
    );
    insert(
        &mut map,
        "filament_map",
        settings
            .filament_map
            .iter()
            .map(i32::to_string)
            .collect::<Vec<_>>()
            .join(","),
    );
    insert(
        &mut map,
        "physical_extruder_map",
        settings
            .physical_extruder_map
            .iter()
            .map(i32::to_string)
            .collect::<Vec<_>>()
            .join(","),
    );
    insert_bool(&mut map, "scan_first_layer", settings.scan_first_layer);
    insert(
        &mut map,
        "printer_structure",
        settings.printer_structure.clone(),
    );
    insert(&mut map, "print_sequence", settings.print_sequence.clone());
    insert(
        &mut map,
        "printable_height",
        num_str(settings.printable_height_mm),
    );
    if !settings.wrapping_detection_gcode.is_empty() {
        insert(
            &mut map,
            "wrapping_detection_gcode",
            settings.wrapping_detection_gcode.clone(),
        );
    }
    insert(&mut map, "filament_type", settings.filament_type.clone());
    insert(
        &mut map,
        "filament_vendor",
        settings.filament_vendor.clone(),
    );
    insert(
        &mut map,
        "chamber_temperatures",
        settings.chamber_temperature_c.to_string(),
    );
    insert(
        &mut map,
        "temperature_vitrification",
        settings.temperature_vitrification_c.to_string(),
    );
    insert_bool(
        &mut map,
        "cooling_filter_enabled",
        settings.cooling_filter_enabled,
    );
    insert(&mut map, "curr_bed_type", settings.curr_bed_type.clone());
    insert(
        &mut map,
        "nozzle_temperature",
        settings.temperature_c.to_string(),
    );
    insert(
        &mut map,
        "nozzle_temperature_initial_layer",
        settings.temperature_initial_layer_c.to_string(),
    );
    insert(
        &mut map,
        "cool_plate_temp",
        settings.cool_plate.later_c.to_string(),
    );
    insert(
        &mut map,
        "cool_plate_temp_initial_layer",
        settings.cool_plate.initial_c.to_string(),
    );
    insert(
        &mut map,
        "eng_plate_temp",
        settings.eng_plate.later_c.to_string(),
    );
    insert(
        &mut map,
        "eng_plate_temp_initial_layer",
        settings.eng_plate.initial_c.to_string(),
    );
    insert(
        &mut map,
        "hot_plate_temp",
        settings.hot_plate.later_c.to_string(),
    );
    insert(
        &mut map,
        "hot_plate_temp_initial_layer",
        settings.hot_plate.initial_c.to_string(),
    );
    insert(
        &mut map,
        "textured_plate_temp",
        settings.textured_plate.later_c.to_string(),
    );
    insert(
        &mut map,
        "textured_plate_temp_initial_layer",
        settings.textured_plate.initial_c.to_string(),
    );
    insert(
        &mut map,
        "supertack_plate_temp",
        settings.supertack_plate.later_c.to_string(),
    );
    insert(
        &mut map,
        "supertack_plate_temp_initial_layer",
        settings.supertack_plate.initial_c.to_string(),
    );
    insert(
        &mut map,
        "nozzle_temperature_range_high",
        settings.nozzle_temperature_range_high.to_string(),
    );
    insert(
        &mut map,
        "retract_lift_above",
        num_str(settings.retract_lift_above_mm),
    );
    insert(
        &mut map,
        "retract_lift_below",
        num_str(settings.retract_lift_below_mm),
    );
    insert(
        &mut map,
        "travel_speed_z",
        num_str(settings.travel_speed_z_mm_s),
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
    insert(
        &mut map,
        "machine_max_jerk_e",
        num_str(settings.e_jerk_mm_s),
    );
    insert(&mut map, "gcode_flavor", settings.gcode_flavor.as_str());
    if settings.printable_area.len() >= 3 {
        insert(
            &mut map,
            "printable_area",
            format_xy_list(&settings.printable_area),
        );
    }
    if !settings.extruder_printable_areas.is_empty() {
        insert(
            &mut map,
            "extruder_printable_area",
            settings
                .extruder_printable_areas
                .iter()
                .map(|p| format_xy_list(p))
                .collect::<Vec<_>>()
                .join(";"),
        );
    }
    insert(
        &mut map,
        "extruder_clearance_max_radius",
        num_str(settings.extruder_clearance_max_radius_mm),
    );
    Ok(serde_json::to_string_pretty(&Value::Object(map))?)
}

/// C++ `GCode::append_full_config` as `; key = value` comments for printer firmware.
pub fn config_block_gcode(settings: &SliceSettings) -> Result<String, ConfigError> {
    let json = project_settings_json(settings)?;
    let value: Value = serde_json::from_str(&json)?;
    let Value::Object(map) = value else {
        return Err(ConfigError::Message(
            "project settings is not a JSON object".into(),
        ));
    };
    let mut out = String::from("; CONFIG_BLOCK_START\n");
    for (k, v) in &map {
        if matches!(k.as_str(), "version" | "name" | "from") {
            continue;
        }
        let Some(text) = config_comment_value(v) else {
            continue;
        };
        out.push_str("; ");
        out.push_str(k);
        out.push_str(" = ");
        out.push_str(&text);
        out.push('\n');
    }
    out.push_str("; CONFIG_BLOCK_END\n");
    Ok(out)
}

fn config_comment_value(v: &Value) -> Option<String> {
    match v {
        Value::Null => None,
        Value::Bool(b) => Some(if *b { "1".into() } else { "0".into() }),
        Value::Number(n) => Some(n.to_string()),
        Value::String(s) => Some(escape_config_comment(s)),
        Value::Array(items) => {
            let parts: Vec<String> = items.iter().filter_map(config_comment_value).collect();
            Some(parts.join(","))
        }
        Value::Object(_) => None,
    }
}

fn escape_config_comment(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('\n', "\\n")
        .replace('\r', "")
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
