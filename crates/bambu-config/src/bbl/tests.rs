use super::parse::value_text;
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
fn vertical_shell_region_keys() {
    let mut s = SliceSettings::default();
    let mut pairs = BTreeMap::new();
    pairs.insert("ensure_vertical_shell_thickness".into(), "disabled".into());
    pairs.insert("top_shell_thickness".into(), "0.6".into());
    pairs.insert("bottom_shell_thickness".into(), "0.8".into());
    pairs.insert("minimum_sparse_infill_area".into(), "12".into());
    pairs.insert("infill_direction".into(), "30".into());
    pairs.insert("bridge_angle".into(), "90".into());
    pairs.insert("infill_wall_overlap".into(), "25%".into());
    pairs.insert("bridge_flow".into(), "0.95".into());
    pairs.insert("top_solid_infill_flow_ratio".into(), "0.92".into());
    pairs.insert("initial_layer_flow_ratio".into(), "1.05".into());
    pairs.insert("top_surface_density".into(), "40%".into());
    pairs.insert("bottom_surface_density".into(), "60".into());
    apply_config_pairs(&mut s, &pairs, true);
    assert_eq!(
        s.ensure_vertical_shell_thickness,
        crate::EnsureVerticalShellThickness::Disabled
    );
    assert!((s.top_shell_thickness_mm - 0.6).abs() < 1e-9);
    assert!((s.bottom_shell_thickness_mm - 0.8).abs() < 1e-9);
    assert!((s.minimum_sparse_infill_area_mm2 - 12.0).abs() < 1e-9);
    assert!((s.infill_direction_deg - 30.0).abs() < 1e-9);
    assert!((s.bridge_angle_deg - 90.0).abs() < 1e-9);
    assert!((s.infill_wall_overlap - 0.25).abs() < 1e-9);
    assert!((s.bridge_flow - 0.95).abs() < 1e-9);
    assert!((s.top_solid_infill_flow_ratio - 0.92).abs() < 1e-9);
    assert!((s.initial_layer_flow_ratio - 1.05).abs() < 1e-9);
    assert!((s.top_surface_density - 0.40).abs() < 1e-9);
    assert!((s.bottom_surface_density - 0.60).abs() < 1e-9);
    pairs.insert("seam_gap".into(), "20%".into());
    apply_config_pairs(&mut s, &pairs, true);
    assert!((s.seam_gap - 0.20).abs() < 1e-9);
    assert!((s.seam_gap_mm() - 0.08).abs() < 1e-9);
    pairs.insert("only_one_wall_first_layer".into(), "1".into());
    apply_config_pairs(&mut s, &pairs, true);
    assert!(s.only_one_wall_first_layer);
    pairs.insert("skirt_height".into(), "3".into());
    apply_config_pairs(&mut s, &pairs, true);
    assert_eq!(s.skirt_height, 1);
    apply_config_pairs(&mut s, &pairs, false);
    assert_eq!(s.skirt_height, 3);
    pairs.insert("draft_shield".into(), "enabled".into());
    apply_config_pairs(&mut s, &pairs, true);
    assert_eq!(s.draft_shield, crate::DraftShield::Disabled);
    apply_config_pairs(&mut s, &pairs, false);
    assert_eq!(s.draft_shield, crate::DraftShield::Enabled);
    pairs.insert("ooze_prevention".into(), "1".into());
    apply_config_pairs(&mut s, &pairs, true);
    assert!(!s.ooze_prevention);
    apply_config_pairs(&mut s, &pairs, false);
    assert!(s.ooze_prevention);
    pairs.insert("reduce_infill_retraction_mode".into(), "Enabled".into());
    apply_config_pairs(&mut s, &pairs, false);
    assert_eq!(
        s.reduce_infill_retraction_mode,
        crate::ReduceInfillRetractionMode::Enabled
    );
    pairs.insert("filament_metal_stickiness".into(), "High".into());
    apply_config_pairs(&mut s, &pairs, false);
    assert_eq!(
        s.filament_metal_stickiness,
        crate::FilamentMetalStickiness::High
    );
    assert!(s.should_reduce_infill_retraction());
    s.reduce_infill_retraction_mode = crate::ReduceInfillRetractionMode::Auto;
    assert!(!s.should_reduce_infill_retraction());
    s.filament_metal_stickiness = crate::FilamentMetalStickiness::None;
    assert!(s.should_reduce_infill_retraction());
}

#[test]
fn wall_infill_order_remaps_to_wall_sequence() {
    let mut s = SliceSettings::default();
    let mut pairs = BTreeMap::new();
    pairs.insert(
        "wall_infill_order".into(),
        "outer wall/inner wall/infill".into(),
    );
    apply_config_pairs(&mut s, &pairs, true);
    assert_eq!(s.wall_sequence, crate::WallSequence::OuterInner);
    assert!(!s.is_infill_first);

    let mut s = SliceSettings::default();
    let mut pairs = BTreeMap::new();
    pairs.insert(
        "wall_infill_order".into(),
        "infill/inner wall/outer wall".into(),
    );
    apply_config_pairs(&mut s, &pairs, false);
    assert_eq!(s.wall_sequence, crate::WallSequence::InnerOuter);
    assert!(s.is_infill_first);

    let mut s = SliceSettings::default();
    let mut pairs = BTreeMap::new();
    pairs.insert(
        "wall_infill_order".into(),
        "inner-outer-inner wall/infill".into(),
    );
    apply_config_pairs(&mut s, &pairs, true);
    assert_eq!(s.wall_sequence, crate::WallSequence::InnerOuterInner);
    assert!(s.weaves_inner_outer_inner());

    let mut s = SliceSettings::default();
    let mut pairs = BTreeMap::new();
    pairs.insert("wall_sequence".into(), "outer wall/inner wall".into());
    apply_config_pairs(&mut s, &pairs, true);
    assert_eq!(s.wall_sequence, crate::WallSequence::OuterInner);
    assert!(s.outer_walls_first());
    assert!(!s.weaves_inner_outer_inner());
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
    assert!((s.top_shell_thickness_mm - 1.0).abs() < 1e-9);
    assert!(s.bottom_shell_thickness_mm.abs() < 1e-9);
    assert_eq!(
        s.ensure_vertical_shell_thickness,
        crate::EnsureVerticalShellThickness::Enabled
    );
    assert_eq!(s.skirt_loops, 0);
    assert_eq!(s.skirt_height, 1);
    assert_eq!(s.draft_shield, crate::DraftShield::Disabled);
    assert!(!s.ooze_prevention);
    assert!((s.brim_width_mm - 5.0).abs() < 1e-9);
    assert!((s.brim_object_gap_mm - 0.1).abs() < 1e-9);
    assert!((s.line_width_mm - 0.42).abs() < 1e-9);
    assert!((s.initial_layer_line_width_mm - 0.5).abs() < 1e-9);
    assert!((s.initial_layer_infill_line_width_mm - 0.5).abs() < 1e-9);
    assert!((s.inner_wall_line_width_mm - 0.45).abs() < 1e-9);
    assert!((s.outer_wall_line_width_mm - 0.42).abs() < 1e-9);
    assert!((s.sparse_infill_line_width_mm - 0.45).abs() < 1e-9);
    assert!((s.internal_solid_infill_line_width_mm - 0.42).abs() < 1e-9);
    assert!((s.top_surface_line_width_mm - 0.42).abs() < 1e-9);
    assert!((s.support_line_width_mm - 0.42).abs() < 1e-9);
    assert!((s.infill_density - 0.15).abs() < 1e-9);
    assert_eq!(s.infill_pattern, InfillPattern::Grid);
    assert_eq!(s.fill_multiline, 1);
    assert!(!s.infill_combination);
    assert!((s.infill_direction_deg - 45.0).abs() < 1e-9);
    assert!(s.bridge_angle_deg.abs() < 1e-9);
    assert_eq!(s.brim_type, crate::BrimType::AutoBrim);
    assert!(!s.symmetric_infill_y_axis);
    assert!((s.minimum_sparse_infill_area_mm2 - 15.0).abs() < 1e-9);
    assert!((s.infill_wall_overlap - 0.15).abs() < 1e-9);
    assert!((s.bridge_flow - 1.0).abs() < 1e-9);
    assert!((s.top_solid_infill_flow_ratio - 1.0).abs() < 1e-9);
    assert!((s.initial_layer_flow_ratio - 1.0).abs() < 1e-9);
    assert!(!s.thick_bridges);
    assert!(!s.alternate_extra_wall);
    assert!(s.enable_arc_fitting);
    assert!((s.resolution_mm - 0.012).abs() < 1e-9);
    assert!(!s.enable_support);
    assert!(!s.support_on_build_plate_only);
    assert!(s.max_bridge_length_mm.abs() < 1e-9);
    assert!(!s.bridge_no_support);
    assert!(s.support_remove_small_overhang);
    assert!(!s.support_critical_regions_only);
    assert!(!s.support_interface_loop_pattern);
    assert_eq!(s.support_base_pattern, crate::SupportBasePattern::Default);
    assert!((s.support_base_pattern_spacing_mm - 2.5).abs() < 1e-9);
    assert_eq!(
        s.support_interface_pattern,
        crate::SupportInterfacePattern::Auto
    );
    assert!((s.support_interface_spacing_mm - 0.5).abs() < 1e-9);
    assert!(s.support_angle_deg.abs() < 1e-9);
    assert!(!s.enable_support_ironing);
    assert!(s.support_expansion_mm.abs() < 1e-9);
    assert!(!s.enable_wrapping_detection);
    assert_eq!(s.support_type, crate::SupportType::Tree);
    assert_eq!(s.tree_support_wall_count, -1);
    assert!(!s.interface_shells);
    assert_eq!(s.ironing_type, crate::IroningType::NoIroning);
    assert_eq!(
        s.reduce_infill_retraction_mode,
        crate::ReduceInfillRetractionMode::Auto
    );
    assert!((s.ironing_flow - 0.10).abs() < 1e-9);
    assert!((s.elephant_foot_mm - 0.15).abs() < 1e-9);
    assert!(!s.precise_z_height);
    assert_eq!(s.top_surface_pattern, crate::SurfacePattern::MonotonicLine);
    assert_eq!(s.bottom_surface_pattern, crate::SurfacePattern::Monotonic);
    assert_eq!(
        s.internal_solid_infill_pattern,
        crate::SurfacePattern::Rectilinear
    );
    assert!((s.top_surface_density - 1.0).abs() < 1e-9);
    assert!((s.bottom_surface_density - 1.0).abs() < 1e-9);
    assert_eq!(s.raft_layers, 0);
    assert_eq!(s.top_one_wall, crate::TopOneWallType::AllTop);
    assert!(!s.only_one_wall_first_layer);
    assert_eq!(s.fuzzy_skin, crate::FuzzySkinType::None);
    assert_eq!(s.wall_generator, crate::WallGenerator::Classic);
    assert_eq!(s.wall_sequence, crate::WallSequence::InnerOuter);
    assert!(!s.precise_outer_wall);
    assert!(!s.is_infill_first);
    assert_eq!(s.seam, crate::SeamPosition::Aligned);
    assert!(!s.seam_placement_away_from_overhangs);
    assert!((s.seam_gap - 0.15).abs() < 1e-9);
    assert!((s.seam_gap_mm() - 0.06).abs() < 1e-9);
    assert!((s.min_feature_size - 0.25).abs() < 1e-9);
    assert!((s.min_bead_width - 0.85).abs() < 1e-9);
    assert!(s.small_perimeter_speed_is_percent);
    assert!((s.small_perimeter_speed - 50.0).abs() < 1e-9);
    assert!(s.small_perimeter_threshold_mm.abs() < 1e-9);
    assert!((s.small_perimeter_speed_mm_s() - 100.0).abs() < 1e-9);
    assert!(s.enable_prime_tower);
    assert!((s.prime_tower_width_mm - 35.0).abs() < 1e-9);
    assert!((s.prime_tower_brim_width_mm - 3.0).abs() < 1e-9);
    assert!(
        !s.has_wipe_tower(),
        "single-filament 0.20 has no wipe tower"
    );
    assert!(s.detect_floating_vertical_shell);
    assert!(s.detect_narrow_internal_solid_infill);
    assert!(s.vertical_shell_speed_is_percent);
    assert!((s.vertical_shell_speed - 80.0).abs() < 1e-9);
    assert!((s.support_speed_mm_s - 150.0).abs() < 1e-9);
    assert!((s.support_interface_speed_mm_s - 80.0).abs() < 1e-9);
    assert!((s.gap_infill_speed_mm_s - 250.0).abs() < 1e-9);
    let baked = SliceSettings::bbl_0_20();
    assert_eq!(baked.top_shell_layers, s.top_shell_layers);
    assert!((baked.top_shell_thickness_mm - s.top_shell_thickness_mm).abs() < 1e-9);
    assert_eq!(baked.wall_loops, s.wall_loops);
    assert_eq!(baked.infill_pattern, s.infill_pattern);
    assert_eq!(baked.top_surface_pattern, s.top_surface_pattern);
    assert_eq!(baked.bottom_surface_pattern, s.bottom_surface_pattern);
    assert!((baked.infill_density - s.infill_density).abs() < 1e-9);
    assert!((baked.brim_width_mm - s.brim_width_mm).abs() < 1e-9);
    assert_eq!(baked.elephant_foot_mm, s.elephant_foot_mm);
    assert!((baked.brim_object_gap_mm - s.brim_object_gap_mm).abs() < 1e-9);
    assert!((baked.initial_layer_line_width_mm - s.initial_layer_line_width_mm).abs() < 1e-9);
    assert!(
        (baked.initial_layer_infill_line_width_mm - s.initial_layer_infill_line_width_mm).abs()
            < 1e-9
    );
    assert_eq!(
        baked.internal_solid_infill_pattern,
        s.internal_solid_infill_pattern
    );
    assert!((baked.inner_wall_line_width_mm - s.inner_wall_line_width_mm).abs() < 1e-9);
    assert!((baked.sparse_infill_line_width_mm - s.sparse_infill_line_width_mm).abs() < 1e-9);
    assert_eq!(baked.top_one_wall, s.top_one_wall);
    assert_eq!(baked.support_type, crate::SupportType::Tree);
}

#[test]
fn line_width_for_matches_cpp_print_region_flow() {
    let mut s = SliceSettings::default();
    s.line_width_mm = 0.42;
    assert!((s.line_width_for(crate::FlowRole::Perimeter, false) - 0.42).abs() < 1e-9);
    assert!((s.line_width_for(crate::FlowRole::Perimeter, true) - 0.42).abs() < 1e-9);
    s.initial_layer_line_width_mm = 0.5;
    s.inner_wall_line_width_mm = 0.45;
    s.outer_wall_line_width_mm = 0.42;
    s.sparse_infill_line_width_mm = 0.45;
    assert!((s.line_width_for(crate::FlowRole::Perimeter, true) - 0.5).abs() < 1e-9);
    assert!((s.line_width_for(crate::FlowRole::SparseInfill, true) - 0.5).abs() < 1e-9);
    s.initial_layer_infill_line_width_mm = 0.8;
    assert!((s.line_width_for(crate::FlowRole::Perimeter, true) - 0.5).abs() < 1e-9);
    assert!((s.line_width_for(crate::FlowRole::ExternalPerimeter, true) - 0.5).abs() < 1e-9);
    assert!((s.line_width_for(crate::FlowRole::SparseInfill, true) - 0.8).abs() < 1e-9);
    assert!((s.line_width_for(crate::FlowRole::SolidInfill, true) - 0.8).abs() < 1e-9);
    assert!((s.line_width_for(crate::FlowRole::TopSolidInfill, true) - 0.8).abs() < 1e-9);
    assert!((s.line_width_for(crate::FlowRole::SupportMaterial, true) - 0.5).abs() < 1e-9);
    assert!((s.line_width_for(crate::FlowRole::Perimeter, false) - 0.45).abs() < 1e-9);
    assert!((s.line_width_for(crate::FlowRole::ExternalPerimeter, false) - 0.42).abs() < 1e-9);
    assert!((s.line_width_for(crate::FlowRole::SparseInfill, false) - 0.45).abs() < 1e-9);
    let baked = SliceSettings::bbl_0_20();
    assert!((baked.arc_fit_tolerance_mm(crate::FlowRole::SparseInfill) - 0.04).abs() < 1e-9);
    assert!((baked.arc_fit_tolerance_mm(crate::FlowRole::SupportMaterial) - 0.0375).abs() < 1e-9);
    assert!((baked.arc_fit_tolerance_mm(crate::FlowRole::ExternalPerimeter) - 0.012).abs() < 1e-9);
}

#[test]
fn gcode_path_flow_factor_matches_cpp_extrude() {
    let mut s = SliceSettings::default();
    s.top_solid_infill_flow_ratio = 0.8;
    s.initial_layer_flow_ratio = 1.2;
    assert!(
        (s.gcode_path_flow_factor(crate::FlowRole::TopSolidInfill, true) - 0.8).abs() < 1e-9,
        "top solid wins over the first-layer factor"
    );
    assert!((s.gcode_path_flow_factor(crate::FlowRole::TopSolidInfill, false) - 0.8).abs() < 1e-9);
    assert!((s.gcode_path_flow_factor(crate::FlowRole::Perimeter, true) - 1.2).abs() < 1e-9);
    assert!((s.gcode_path_flow_factor(crate::FlowRole::SolidInfill, true) - 1.2).abs() < 1e-9);
    assert!((s.gcode_path_flow_factor(crate::FlowRole::Perimeter, false) - 1.0).abs() < 1e-9);
}

#[test]
fn brim_type_and_bridge_angle_match_cpp() {
    let mut s = SliceSettings::default();
    s.brim_width_mm = 5.0;
    assert!(s.has_outer_brim());
    s.brim_type = crate::BrimType::NoBrim;
    assert!(!s.has_outer_brim());
    s.brim_type = crate::BrimType::InnerOnly;
    assert!(!s.has_outer_brim());
    s.brim_type = crate::BrimType::OuterOnly;
    assert!(s.has_outer_brim());
    s.infill_direction_deg = 45.0;
    s.bridge_angle_deg = 90.0;
    assert!((s.bridge_fill_angle_deg(false) - 45.0).abs() < 1e-9);
    assert!((s.bridge_fill_angle_deg(true) - 90.0).abs() < 1e-9);
    s.bridge_angle_deg = 0.0;
    assert!((s.bridge_fill_angle_deg(true) - 45.0).abs() < 1e-9);
}

#[test]
fn bridging_flow_matches_cpp_thick_and_thin() {
    let mut s = SliceSettings::default();
    s.line_width_mm = 0.42;
    s.nozzle_diameter_mm = 0.4;
    s.bridge_flow = 1.0;
    let thin = crate::Flow::bridging_flow(&s, crate::FlowRole::SolidInfill, 0.2, false, false);
    assert!(!thin.bridge);
    assert!((thin.width_mm - 0.42).abs() < 1e-9);
    assert!((thin.mm3_per_mm() - 0.42 * 0.2).abs() < 1e-9);
    assert!((thin.spacing_mm() - 0.42).abs() < 1e-9);
    s.bridge_flow = 0.5;
    let thin_half = crate::Flow::bridging_flow(&s, crate::FlowRole::SolidInfill, 0.2, false, false);
    assert!((thin_half.mm3_per_mm() / thin.mm3_per_mm() - 0.5).abs() < 1e-9);
    s.bridge_flow = 1.0;
    let thick = crate::Flow::bridging_flow(&s, crate::FlowRole::SolidInfill, 0.2, false, true);
    assert!(thick.bridge);
    assert!((thick.width_mm - 0.4).abs() < 1e-9);
    let expected_mm3 = std::f64::consts::PI * 0.2 * 0.2;
    assert!((thick.mm3_per_mm() - expected_mm3).abs() < 1e-9);
    assert!((thick.spacing_mm() - (0.4 + crate::BRIDGE_EXTRA_SPACING_MM)).abs() < 1e-9);
    s.bridge_flow = 4.0;
    let thick_wide = crate::Flow::bridging_flow(&s, crate::FlowRole::SolidInfill, 0.2, false, true);
    assert!((thick_wide.width_mm - 0.8).abs() < 1e-9);
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
        value_text(obj.get("infill_wall_overlap").unwrap()).as_deref(),
        Some("15%")
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
        value_text(obj.get("skirt_height").unwrap()).as_deref(),
        Some("1")
    );
    assert_eq!(
        value_text(obj.get("draft_shield").unwrap()).as_deref(),
        Some("disabled")
    );
    assert_eq!(
        value_text(obj.get("reduce_infill_retraction_mode").unwrap()).as_deref(),
        Some("Auto")
    );
    assert_eq!(
        value_text(obj.get("wall_infill_order").unwrap()).as_deref(),
        Some("inner wall/outer wall/infill")
    );
    assert_eq!(
        value_text(obj.get("brim_width").unwrap()).as_deref(),
        Some("5")
    );
    assert_eq!(
        value_text(obj.get("brim_object_gap").unwrap()).as_deref(),
        Some("0.1")
    );
    assert_eq!(
        value_text(obj.get("inner_wall_line_width").unwrap()).as_deref(),
        Some("0.45")
    );
    assert_eq!(
        value_text(obj.get("initial_layer_line_width").unwrap()).as_deref(),
        Some("0.5")
    );
    assert!(!obj.contains_key("inherits"));
    let s = load_bbl_process(&paths.process).unwrap();
    assert!((s.infill_density - 0.15).abs() < 1e-9);
    assert_eq!(s.infill_pattern, InfillPattern::Grid);
    assert!((s.minimum_sparse_infill_area_mm2 - 15.0).abs() < 1e-9);
    assert_eq!(s.top_shell_layers, 5);
    assert_eq!(s.skirt_loops, 0);
    assert_eq!(s.skirt_height, 1);
    assert_eq!(s.draft_shield, crate::DraftShield::Disabled);
    assert!(!s.ooze_prevention);
    assert_eq!(
        s.reduce_infill_retraction_mode,
        crate::ReduceInfillRetractionMode::Auto
    );
    assert!((s.default_acceleration_mm_s2 - 8000.0).abs() < 1.0);
    assert!((s.outer_wall_acceleration_mm_s2 - 5000.0).abs() < 1.0);
    assert!((s.initial_layer_acceleration_mm_s2 - 500.0).abs() < 1.0);
    assert!((s.travel_short_distance_acceleration_mm_s2 - 250.0).abs() < 1.0);
    assert!(s.enable_prime_tower);
    assert!((s.prime_tower_width_mm - 60.0).abs() < 1e-9);
    assert!((s.prime_tower_brim_width_mm + 1.0).abs() < 1e-9);
    assert!(!s.has_wipe_tower());
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
    assert_eq!(
        s.filament_metal_stickiness,
        crate::FilamentMetalStickiness::None
    );
    assert!((s.filament_max_volumetric_speed_mm3_s - 12.0).abs() < 1e-9);
    assert!((s.flow_ratio - 0.99).abs() < 1e-9);
    assert!(s.slow_down_for_layer_cooling);
    assert!((s.slow_down_min_speed_mm_s - 20.0).abs() < 1e-9);
    assert_eq!(s.overhang_fan_speed, 100);
    assert_eq!(
        s.overhang_fan_threshold,
        crate::OverhangFanThreshold::ThreeFour
    );
    assert_eq!(s.ironing_fan_speed, -1);
    assert_eq!(s.additional_cooling_fan_speed, 75);
    assert_eq!(s.close_additional_fan_first_x_layers, 1);
    assert_eq!(s.first_x_layer_fan_speed, 0);
    assert!((s.pre_start_fan_time_s - 2.0).abs() < 1e-9);
    assert!(!s.activate_air_filtration);
    assert_eq!(s.temperature_c, 220);
    assert_eq!(s.temperature_initial_layer_c, 220);
    assert_eq!(s.cool_plate.later_c, 35);
    assert_eq!(s.cool_plate.initial_c, 35);
    assert_eq!(s.hot_plate.later_c, 55);
    assert_eq!(s.textured_plate.later_c, 55);
    assert_eq!(s.eng_plate.later_c, 55);
    assert_eq!(s.bed_temperature_c, 35);
    assert_eq!(s.bed_temperature_initial_layer_c, 35);
    s.curr_bed_type = String::from("Textured PEI Plate");
    s.resolve_bed_temps_from_plate();
    assert_eq!(s.bed_temperature_c, 55);
    assert_eq!(s.bed_temperature_initial_layer_c, 55);
    s.curr_bed_type = String::from("High Temp Plate");
    s.resolve_bed_temps_from_plate();
    assert_eq!(s.bed_temperature_c, 55);
}

#[test]
fn h2c_machine_sets_retraction() {
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
    assert!((s.z_hop_mm - 0.4).abs() < 1e-9);
    assert_eq!(s.z_hop_type, crate::ZHopType::Auto);
    assert!((s.retract_lift_below_mm - 319.0).abs() < 1e-9);
    assert!(s.auxiliary_fan);
    assert!(!s.support_air_filtration);
    assert!((s.retract_acceleration_mm_s2 - 5000.0).abs() < 1.0);
    assert_eq!(s.gcode_flavor, crate::GCodeFlavor::Marlin);
    assert!((s.machine_limits.acceleration_x_mm_s2 - 20000.0).abs() < 1.0);
    assert!((s.machine_limits.acceleration_y_mm_s2 - 20000.0).abs() < 1.0);
    assert!((s.machine_limits.acceleration_z_mm_s2 - 500.0).abs() < 1.0);
    assert!((s.machine_limits.acceleration_e_mm_s2 - 5000.0).abs() < 1.0);
    assert!((s.machine_limits.acceleration_extruding_mm_s2 - 20000.0).abs() < 1.0);
    assert!((s.machine_limits.speed_x_mm_s - 1000.0).abs() < 1.0);
    assert!((s.machine_limits.speed_y_mm_s - 1000.0).abs() < 1.0);
    assert!((s.machine_limits.speed_z_mm_s - 30.0).abs() < 1.0);
    assert!((s.machine_limits.speed_e_mm_s - 50.0).abs() < 1.0);
    assert!((s.e_jerk_mm_s - 2.5).abs() < 1e-9);
    assert_eq!(
            s.print_machine_envelope().as_deref(),
            Some(
                "M201 X20000 Y20000 Z500 E5000\nM203 X1000 Y1000 Z30 E50\nM204 P20000 R5000 T20000\nM205 X9.00 Y9.00 Z3.00 E2.50\n"
            )
        );
    assert!(s.layer_change_gcode.contains("M73 L{layer_num+1}"));
    assert!(s.layer_change_gcode.contains("M991 S0 P{layer_num}"));
    assert!(s
        .machine_end_gcode
        .contains(";===== machine: H2C end ====="));
    assert!(s
        .machine_start_gcode
        .contains(";===== machine: H2C ========================="));
    assert!(s
        .time_lapse_gcode
        .contains(";===== machine: H2C timelapse ====="));
    assert!(s.time_lapse_gcode.contains("SKIPTYPE: timelapse"));
    assert!((s.extruder_clearance_max_radius_mm - 96.0).abs() < 1e-9);
    assert!(s.printable_area.len() >= 4, "{:?}", s.printable_area);
    assert_eq!(s.extruder_printable_areas.len(), 2);
    assert!(s.farthest_point_timelapse);
    assert_eq!(s.timelapse_type, 0);
    assert!(!s.spiral_mode);
    assert!(!s.enable_wrapping_detection);
    assert!(s.wrapping_detection_gcode.contains("G39"));
    assert!(s.wrapping_detection_gcode.contains("layer_num == 3"));
    assert_eq!(s.filament_map, vec![1]);
    assert_eq!(s.physical_extruder_map, vec![1, 0]);
    assert_eq!(s.physical_extruder_id(0), 1);
    assert_eq!(s.nozzle_diameters_mm, vec![0.4, 0.4]);
    assert_eq!(s.nozzle_count(), 2);
    assert_eq!(s.first_filaments(), vec![-1, 0]);
    assert_eq!(s.first_non_support_filaments(), vec![-1, 0]);
    assert_eq!(s.first_non_support_hotends(), vec![-1, 0]);
    assert!(!s.scan_first_layer);
    assert_eq!(s.printer_structure, "corexy");
    assert_eq!(s.print_sequence, "by layer");
    assert!((s.printable_height_mm - 325.0).abs() < 1e-9);
    assert!(s
        .machine_end_gcode
        .contains("{if long_retraction_when_cut}"));
    assert!(s.long_retraction_when_cut);
    assert!((s.retraction_distance_when_cut - 14.0).abs() < 1e-9);
    assert!(s.bed_bbox_valid);
    assert!((s.bed_min_x).abs() < 1e-9);
    assert!((s.bed_max_x - 325.0).abs() < 1e-9);
    assert!((s.bed_max_y - 320.0).abs() < 1e-9);
    overlay_bbl_profile(&mut s, &paths.filament).unwrap();
    assert!(s.filament_end_gcode.contains("; filament end gcode"));
    assert!(s.filament_start_gcode.contains("; filament start gcode"));
    assert_eq!(s.filament_type, "PLA");
    assert_eq!(s.filament_vendor, "Generic");
    assert_eq!(s.temperature_vitrification_c, 45);
    assert_eq!(s.chamber_temperature_c, 0);
    assert!(s.long_retraction_when_cut);
    assert!(s.long_retraction_when_ec);
    assert!((s.retraction_distance_when_ec - 10.0).abs() < 1e-9);
    assert!((s.retraction_length_mm - 0.4).abs() < 1e-9);
    assert!((s.wipe_distance_mm - 1.0).abs() < 1e-9);
    assert_eq!(s.z_hop_type, crate::ZHopType::Spiral);
    assert_eq!(s.temperature_c, 220);
    assert_eq!(s.temperature_initial_layer_c, 220);
    assert_eq!(s.bed_temperature_c, 35);
    assert_eq!(s.bed_temperature_initial_layer_c, 35);
    assert!(s.farthest_point_timelapse_enabled());
    let baked = SliceSettings::bbl_0_20();
    assert!((baked.retraction_length_mm - 0.4).abs() < 1e-9);
    assert!(baked.wipe);
    assert_eq!(baked.z_hop_type, crate::ZHopType::Spiral);
    assert!(baked.layer_change_gcode.contains("M73 L{layer_num+1}"));
    assert!(baked.auxiliary_fan);
    assert!(baked.bed_bbox_valid);
    assert!((baked.bed_max_x - 325.0).abs() < 1e-9);
    assert_eq!(baked.additional_cooling_fan_speed, 75);
    assert!((baked.outer_wall_acceleration_mm_s2 - 5000.0).abs() < 1.0);
    assert!((baked.travel_short_distance_acceleration_mm_s2 - 250.0).abs() < 1.0);
    assert!((baked.retract_acceleration_mm_s2 - 5000.0).abs() < 1.0);
    assert!((baked.pre_start_fan_time_s - 2.0).abs() < 1e-9);
    assert!((baked.travel_speed_mm_s - 1000.0).abs() < 1e-9);
    assert!((baked.flow_ratio - 0.99).abs() < 1e-9);
}

#[test]
fn start_gcode_context_sets_cpp_export_keys() {
    let mut s = SliceSettings::default();
    s.during_print_exhaust_fan_speed = 70;
    s.activate_air_filtration = true;
    s.support_air_filtration = true;
    s.filament_max_volumetric_speed_mm3_s = 12.0;
    s.print_speed_mm_s = 200.0;
    let ctx = s.placeholder_custom_gcode_context(0, 1, 20.0, (0.0, 0.0), (20.0, 20.0));
    assert_eq!(
        crate::expand_placeholders("{first_layer_center_no_wipe_tower[1]}", &ctx),
        "10"
    );
    assert_eq!(
        crate::expand_placeholders("{during_print_exhaust_fan_speed_num}", &ctx),
        "178"
    );
    assert_eq!(
        crate::expand_placeholders(
            "{if activate_air_filtration && support_air_filtration}ON{else}OFF{endif}",
            &ctx
        ),
        "ON"
    );
    assert_eq!(
        crate::expand_placeholders("{print_sequence}", &ctx),
        "by layer"
    );
    assert_eq!(
        crate::expand_placeholders("{printer_structure}", &ctx),
        "undefine"
    );
    assert!((s.outer_wall_volumetric_speed() - 12.0).abs() < 1e-9);
    assert_eq!(
        crate::expand_placeholders("{first_non_support_filaments[0]}", &ctx),
        "0"
    );
    assert_eq!(
        crate::expand_placeholders(
            "{if (first_non_support_filaments[0] != -1)}yes{else}no{endif}",
            &ctx
        ),
        "yes"
    );
}

#[test]
fn h2c_first_non_support_filaments_skip_physical_slot_0() {
    let paths = bbl_oracle_paths().expect("upstream BambuStudio profiles");
    let mut s = SliceSettings::default();
    overlay_bbl_profile(&mut s, &paths.machine).unwrap();
    let ctx = s.placeholder_custom_gcode_context(0, 1, 20.0, (0.0, 0.0), (20.0, 20.0));
    assert_eq!(
        crate::expand_placeholders("{first_non_support_filaments[0]}", &ctx),
        "-1"
    );
    assert_eq!(
        crate::expand_placeholders("{first_non_support_filaments[1]}", &ctx),
        "0"
    );
    assert_eq!(
        crate::expand_placeholders("{first_non_support_hotend[0]}", &ctx),
        "-1"
    );
    assert_eq!(
        crate::expand_placeholders(
            "{if (first_non_support_filaments[0] != -1)}yes{else}no{endif}",
            &ctx
        ),
        "no"
    );
}

#[test]
fn i3_disables_farthest_point_timelapse() {
    let mut s = SliceSettings::default();
    s.farthest_point_timelapse = true;
    s.timelapse_type = 0;
    s.printer_structure = String::from("corexy");
    assert!(s.farthest_point_timelapse_enabled());
    s.printer_structure = String::from("i3");
    assert!(!s.farthest_point_timelapse_enabled());
    s.gcode_flavor = crate::GCodeFlavor::Klipper;
    assert!(s.print_machine_envelope().is_none());
}

#[test]
fn project_settings_json_roundtrip() {
    let mut src = SliceSettings::default();
    src.layer_height_mm = 0.28;
    src.infill_density = 0.15;
    src.fill_multiline = 3;
    src.infill_combination = true;
    src.top_surface_density = 0.4;
    src.bottom_surface_density = 0.6;
    src.wall_loops = 3;
    src.alternate_extra_wall = true;
    src.enable_support = true;
    src.support_on_build_plate_only = true;
    src.max_bridge_length_mm = 10.0;
    src.bridge_no_support = true;
    src.support_remove_small_overhang = false;
    src.support_critical_regions_only = true;
    src.support_interface_loop_pattern = true;
    src.support_base_pattern = crate::SupportBasePattern::Honeycomb;
    src.support_base_pattern_spacing_mm = 1.0;
    src.support_interface_pattern = crate::SupportInterfacePattern::Concentric;
    src.support_interface_spacing_mm = 0.0;
    src.support_angle_deg = 45.0;
    src.enable_support_ironing = true;
    src.support_expansion_mm = 2.0;
    src.tree_support_wall_count = 2;
    src.interface_shells = true;
    src.precise_outer_wall = true;
    src.symmetric_infill_y_axis = true;
    src.bridge_angle_deg = 90.0;
    src.brim_type = crate::BrimType::NoBrim;
    src.seam_placement_away_from_overhangs = true;
    src.internal_solid_infill_pattern = crate::SurfacePattern::Concentric;
    src.initial_layer_infill_line_width_mm = 0.8;
    src.top_solid_infill_flow_ratio = 0.9;
    src.initial_layer_flow_ratio = 1.1;
    src.thick_bridges = true;
    src.support_type = crate::SupportType::Tree;
    src.ironing_type = crate::IroningType::TopSurfaces;
    src.temperature_c = 215;
    src.temperature_initial_layer_c = 230;
    src.cool_plate.later_c = 35;
    src.cool_plate.initial_c = 40;
    src.curr_bed_type = String::from("Cool Plate");
    src.enable_prime_tower = true;
    src.filament_count = 8;
    src.wipe_tower_x_mm = 15.0;
    src.wipe_tower_y_mm = 194.264;
    src.prime_tower_width_mm = 35.0;
    src.prime_tower_brim_width_mm = 3.0;
    let json = crate::project_settings_json(&src).unwrap();
    assert!(json.contains("\"from\": \"project\""));
    let loaded = crate::settings_from_json(&json).unwrap();
    assert!((loaded.layer_height_mm - 0.28).abs() < 1e-9);
    assert!((loaded.infill_density - 0.15).abs() < 1e-9);
    assert_eq!(loaded.fill_multiline, 3);
    assert!(loaded.infill_combination);
    assert!((loaded.top_surface_density - 0.4).abs() < 1e-9);
    assert!((loaded.bottom_surface_density - 0.6).abs() < 1e-9);
    assert_eq!(loaded.wall_loops, 3);
    assert!(loaded.alternate_extra_wall);
    assert!(loaded.enable_support);
    assert!(loaded.support_on_build_plate_only);
    assert!((loaded.max_bridge_length_mm - 10.0).abs() < 1e-9);
    assert!(loaded.bridge_no_support);
    assert!(!loaded.support_remove_small_overhang);
    assert!(loaded.support_critical_regions_only);
    assert!(loaded.support_interface_loop_pattern);
    assert_eq!(
        loaded.support_base_pattern,
        crate::SupportBasePattern::Honeycomb
    );
    assert!((loaded.support_base_pattern_spacing_mm - 1.0).abs() < 1e-9);
    assert_eq!(
        loaded.support_interface_pattern,
        crate::SupportInterfacePattern::Concentric
    );
    assert!(loaded.support_interface_spacing_mm.abs() < 1e-9);
    assert!((loaded.support_angle_deg - 45.0).abs() < 1e-9);
    assert!(loaded.enable_support_ironing);
    assert!((loaded.support_expansion_mm - 2.0).abs() < 1e-9);
    assert_eq!(loaded.tree_support_wall_count, 2);
    assert!(loaded.interface_shells);
    assert!(loaded.precise_outer_wall);
    assert!(loaded.symmetric_infill_y_axis);
    assert!((loaded.bridge_angle_deg - 90.0).abs() < 1e-9);
    assert_eq!(loaded.brim_type, crate::BrimType::NoBrim);
    assert!(loaded.seam_placement_away_from_overhangs);
    assert_eq!(
        loaded.internal_solid_infill_pattern,
        crate::SurfacePattern::Concentric
    );
    assert!((loaded.initial_layer_infill_line_width_mm - 0.8).abs() < 1e-9);
    assert!((loaded.top_solid_infill_flow_ratio - 0.9).abs() < 1e-9);
    assert!((loaded.initial_layer_flow_ratio - 1.1).abs() < 1e-9);
    assert!(loaded.thick_bridges);
    assert_eq!(loaded.support_type, crate::SupportType::Tree);
    assert_eq!(loaded.ironing_type, crate::IroningType::TopSurfaces);
    assert_eq!(loaded.temperature_c, 215);
    assert_eq!(loaded.temperature_initial_layer_c, 230);
    assert_eq!(loaded.cool_plate.later_c, 35);
    assert_eq!(loaded.cool_plate.initial_c, 40);
    assert_eq!(loaded.bed_temperature_c, 35);
    assert_eq!(loaded.bed_temperature_initial_layer_c, 40);
    assert!((loaded.default_acceleration_mm_s2 - 10000.0).abs() < 1e-9);
    assert!((loaded.filament_density_g_cm3 - 1.24).abs() < 1e-9);
    assert!(loaded.small_perimeter_speed_is_percent);
    assert!((loaded.small_perimeter_speed - 50.0).abs() < 1e-9);
    assert_eq!(loaded.fan_min_speed, 20);
    assert!(!loaded.reduce_fan_stop_start_freq);
    assert!(!loaded.slow_down_for_layer_cooling);
    assert!(!loaded.no_slow_down_for_cooling_on_outwalls);
    assert!((loaded.flow_ratio - 1.0).abs() < 1e-9);
    assert_eq!(loaded.enable_prime_tower, src.enable_prime_tower);
    assert!((loaded.wipe_tower_x_mm - src.wipe_tower_x_mm).abs() < 1e-9);
    assert!((loaded.prime_tower_width_mm - src.prime_tower_width_mm).abs() < 1e-9);
    assert_eq!(loaded.filament_count, src.filament_count);
    assert!(!loaded.enable_arc_fitting);
    src.enable_arc_fitting = true;
    src.resolution_mm = 0.012;
    src.wall_sequence = crate::WallSequence::OuterInner;
    src.is_infill_first = true;
    src.skirt_height = 4;
    src.draft_shield = crate::DraftShield::Enabled;
    src.ooze_prevention = true;
    src.reduce_infill_retraction_mode = crate::ReduceInfillRetractionMode::Enabled;
    src.filament_metal_stickiness = crate::FilamentMetalStickiness::High;
    let json = crate::project_settings_json(&src).unwrap();
    let loaded = crate::settings_from_json(&json).unwrap();
    assert!(loaded.enable_arc_fitting);
    assert!((loaded.resolution_mm - 0.012).abs() < 1e-9);
    assert_eq!(loaded.wall_sequence, crate::WallSequence::OuterInner);
    assert!(loaded.is_infill_first);
    assert_eq!(loaded.skirt_height, 4);
    assert_eq!(loaded.draft_shield, crate::DraftShield::Enabled);
    assert!(loaded.ooze_prevention);
    assert_eq!(
        loaded.reduce_infill_retraction_mode,
        crate::ReduceInfillRetractionMode::Enabled
    );
    assert_eq!(
        loaded.filament_metal_stickiness,
        crate::FilamentMetalStickiness::High
    );
}

#[test]
fn config_block_gcode_emits_understood_keys() {
    let mut src = SliceSettings::default();
    src.wall_loops = 3;
    src.layer_height_mm = 0.2;
    src.machine_start_gcode = "{if 1}G28{endif}\nnext".into();
    let block = crate::config_block_gcode(&src).unwrap();
    assert!(block.starts_with("; CONFIG_BLOCK_START\n"));
    assert!(block.contains("; CONFIG_BLOCK_END\n"));
    assert!(block.contains("; wall_loops = 3\n"));
    assert!(block.contains("; layer_height = 0.2\n"));
    assert!(
        block.contains("; machine_start_gcode = {if 1}G28{endif}\\nnext"),
        "{block}"
    );
    assert_eq!(
        block
            .lines()
            .filter(|l| l.starts_with("; machine_start_gcode = "))
            .count(),
        1
    );
    assert!(block.lines().all(|l| l.starts_with(';')));
    assert!(!block.contains("\n; version = "));
}

#[test]
fn region_overrides_skip_object_keys() {
    let mut s = SliceSettings::default();
    let mut pairs = BTreeMap::new();
    pairs.insert("sparse_infill_density".into(), "100%".into());
    pairs.insert("fill_multiline".into(), "4".into());
    pairs.insert("infill_combination".into(), "1".into());
    pairs.insert("wall_loops".into(), "6".into());
    pairs.insert("alternate_extra_wall".into(), "1".into());
    pairs.insert("extruder".into(), "3".into());
    pairs.insert("layer_height".into(), "0.08".into());
    pairs.insert("enable_support".into(), "1".into());
    pairs.insert("support_on_build_plate_only".into(), "1".into());
    pairs.insert("max_bridge_length".into(), "10".into());
    pairs.insert("bridge_no_support".into(), "1".into());
    pairs.insert("support_remove_small_overhang".into(), "0".into());
    pairs.insert("support_critical_regions_only".into(), "1".into());
    pairs.insert("support_interface_loop_pattern".into(), "1".into());
    pairs.insert("support_base_pattern".into(), "honeycomb".into());
    pairs.insert("support_base_pattern_spacing".into(), "1".into());
    pairs.insert("support_interface_pattern".into(), "concentric".into());
    pairs.insert("support_interface_spacing".into(), "0".into());
    pairs.insert("support_angle".into(), "45".into());
    pairs.insert("enable_support_ironing".into(), "1".into());
    pairs.insert("support_expansion".into(), "2".into());
    pairs.insert("tree_support_wall_count".into(), "2".into());
    pairs.insert("interface_shells".into(), "1".into());
    pairs.insert("precise_outer_wall".into(), "1".into());
    pairs.insert("symmetric_infill_y_axis".into(), "1".into());
    pairs.insert("bridge_angle".into(), "90".into());
    pairs.insert("brim_type".into(), "no_brim".into());
    pairs.insert("seam_placement_away_from_overhangs".into(), "1".into());
    pairs.insert("internal_solid_infill_pattern".into(), "concentric".into());
    pairs.insert("initial_layer_infill_line_width".into(), "0.8".into());
    pairs.insert("top_solid_infill_flow_ratio".into(), "0.9".into());
    pairs.insert("initial_layer_flow_ratio".into(), "1.1".into());
    pairs.insert("thick_bridges".into(), "1".into());
    pairs.insert("ooze_prevention".into(), "1".into());
    apply_config_pairs(&mut s, &pairs, true);
    assert!((s.infill_density - 1.0).abs() < 1e-9);
    assert_eq!(s.fill_multiline, 4);
    assert!(s.infill_combination);
    assert_eq!(s.wall_loops, 6);
    assert!(s.alternate_extra_wall);
    assert_eq!(s.wall_filament, 3);
    assert_eq!(s.sparse_infill_filament, 3);
    assert_eq!(s.solid_infill_filament, 3);
    assert!((s.layer_height_mm - 0.2).abs() < 1e-9);
    assert!(!s.enable_support);
    assert!(!s.support_on_build_plate_only);
    assert!(s.max_bridge_length_mm.abs() < 1e-9);
    assert!(!s.bridge_no_support);
    assert!(s.support_remove_small_overhang);
    assert!(!s.support_critical_regions_only);
    assert!(!s.support_interface_loop_pattern);
    assert_eq!(s.support_base_pattern, crate::SupportBasePattern::Default);
    assert!((s.support_base_pattern_spacing_mm - 2.5).abs() < 1e-9);
    assert_eq!(
        s.support_interface_pattern,
        crate::SupportInterfacePattern::Auto
    );
    assert!((s.support_interface_spacing_mm - 0.5).abs() < 1e-9);
    assert!(s.support_angle_deg.abs() < 1e-9);
    assert!(!s.enable_support_ironing);
    assert!(s.support_expansion_mm.abs() < 1e-9);
    assert_eq!(s.tree_support_wall_count, -1);
    assert!(!s.interface_shells);
    assert!(s.precise_outer_wall);
    assert!(s.symmetric_infill_y_axis);
    assert!((s.bridge_angle_deg - 90.0).abs() < 1e-9);
    assert_eq!(s.brim_type, crate::BrimType::AutoBrim);
    assert!(!s.seam_placement_away_from_overhangs);
    assert_eq!(
        s.internal_solid_infill_pattern,
        crate::SurfacePattern::Concentric
    );
    assert!(s.initial_layer_infill_line_width_mm.abs() < 1e-9);
    assert!((s.top_solid_infill_flow_ratio - 0.9).abs() < 1e-9);
    assert!((s.initial_layer_flow_ratio - 1.1).abs() < 1e-9);
    assert!(!s.thick_bridges);
    assert!(!s.ooze_prevention);
    apply_config_pairs(&mut s, &pairs, false);
    assert!((s.layer_height_mm - 0.08).abs() < 1e-9);
    assert!(s.enable_support);
    assert!(s.support_on_build_plate_only);
    assert!((s.max_bridge_length_mm - 10.0).abs() < 1e-9);
    assert!(s.bridge_no_support);
    assert!(!s.support_remove_small_overhang);
    assert!(s.support_critical_regions_only);
    assert!(s.support_interface_loop_pattern);
    assert_eq!(s.support_base_pattern, crate::SupportBasePattern::Honeycomb);
    assert!((s.support_base_pattern_spacing_mm - 1.0).abs() < 1e-9);
    assert_eq!(
        s.support_interface_pattern,
        crate::SupportInterfacePattern::Concentric
    );
    assert!(s.support_interface_spacing_mm.abs() < 1e-9);
    assert!((s.support_angle_deg - 45.0).abs() < 1e-9);
    assert!(s.enable_support_ironing);
    assert!((s.support_expansion_mm - 2.0).abs() < 1e-9);
    assert_eq!(s.tree_support_wall_count, 2);
    assert!(s.interface_shells);
    assert!(s.precise_outer_wall);
    assert!(s.symmetric_infill_y_axis);
    assert!((s.bridge_angle_deg - 90.0).abs() < 1e-9);
    assert_eq!(s.brim_type, crate::BrimType::NoBrim);
    assert!(s.seam_placement_away_from_overhangs);
    assert_eq!(
        s.internal_solid_infill_pattern,
        crate::SurfacePattern::Concentric
    );
    assert!((s.initial_layer_infill_line_width_mm - 0.8).abs() < 1e-9);
    assert!(s.thick_bridges);
    assert!(s.ooze_prevention);
}

#[test]
fn has_wipe_tower_matches_cpp() {
    let mut s = SliceSettings::default();
    s.enable_prime_tower = true;
    assert!(!s.has_wipe_tower(), "one filament, traditional timelapse");
    s.filament_count = 8;
    assert!(s.has_wipe_tower());
    s.spiral_mode = true;
    assert!(!s.has_wipe_tower());
    s.spiral_mode = false;
    s.filament_count = 1;
    s.timelapse_type = 1;
    assert!(s.has_wipe_tower(), "smooth timelapse keeps the tower");
    s.enable_prime_tower = false;
    assert!(!s.has_wipe_tower());
}

#[test]
fn has_infinite_skirt_matches_cpp() {
    let mut s = SliceSettings::default();
    assert!(!s.has_infinite_skirt());
    s.draft_shield = crate::DraftShield::Enabled;
    assert!(s.has_infinite_skirt());
    s.skirt_loops = 0;
    assert!(!s.has_infinite_skirt(), "enabled shield still needs loops");
    s.draft_shield = crate::DraftShield::Disabled;
    s.skirt_loops = 2;
    s.ooze_prevention = true;
    assert!(
        !s.has_infinite_skirt(),
        "single extruder does not raise the skirt"
    );
    s.filament_count = 2;
    assert!(s.has_infinite_skirt());
    s.ooze_prevention = false;
    assert!(!s.has_infinite_skirt());
}

#[test]
fn wall_loops_for_layer_matches_cpp() {
    let mut s = SliceSettings::default();
    s.wall_loops = 2;
    assert_eq!(s.wall_loops_for_layer(0), 2);
    assert_eq!(s.wall_loops_for_layer(1), 2);
    s.alternate_extra_wall = true;
    assert_eq!(s.wall_loops_for_layer(0), 2);
    assert_eq!(s.wall_loops_for_layer(1), 3);
    assert_eq!(s.wall_loops_for_layer(2), 2);
    s.spiral_mode = true;
    assert_eq!(s.wall_loops_for_layer(1), 2);
}
