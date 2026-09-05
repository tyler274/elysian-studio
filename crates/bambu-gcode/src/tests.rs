use super::*;
use crate::parse::parse_axis;
use bambu_config::SliceSettings;
use bambu_geom::TriangleMesh;
use bambu_slicer::slice_mesh;

#[test]
fn cube_gcode_has_layers() {
    let mesh = TriangleMesh::cube(20.0);
    let settings = SliceSettings::default();
    let sliced = slice_mesh(&mesh, &settings).unwrap();
    let gcode = write_gcode(&settings, &sliced).unwrap();
    let stats = layer_stats(&gcode);
    assert!(stats.layer_comments >= 90, "{stats:?}");
    assert!(gcode.contains("G1 X"));
    assert!(gcode.contains("; FEATURE: Outer wall"));
    assert!(gcode.contains("; FEATURE: Sparse infill"));
    assert!(gcode.contains("; FEATURE: Skirt"));
    assert!(gcode.contains("; FEATURE: Bottom surface"));
    assert!(gcode.contains("; FEATURE: Top surface"));
    assert!(gcode.contains("; FEATURE: Internal solid infill"));
    assert!(!gcode.contains("; FEATURE: Floating vertical shell"));
    assert!(!gcode.contains("; FEATURE: Ironing"));
    assert!(gcode.contains("; CHANGE_LAYER"));
    assert!(
        !executable_block(&gcode).lines().any(|l| is_rapid_g0(l)),
        "C++ GCodeWriter travel is G1, not G0"
    );
    let report = parse_gcode(&gcode);
    assert_eq!(report.layer_changes, stats.layer_comments);
    assert!(report.features.contains("Outer wall"));
}

#[test]
fn cube_gcode_has_time_and_filament_estimate() {
    let mesh = TriangleMesh::cube(20.0);
    let settings = SliceSettings::default();
    let sliced = slice_mesh(&mesh, &settings).unwrap();
    let gcode = write_gcode(&settings, &sliced).unwrap();
    assert!(gcode.contains("; model printing time:"));
    assert!(gcode.contains("; total filament weight [g] :"));
    assert!(gcode.contains("; total filament length [mm] :"));
    let report = parse_gcode(&gcode);
    let seconds = report.estimated_seconds.expect("estimated time");
    assert!(
        (60.0..7200.0).contains(&seconds),
        "20 mm cube estimate out of range: {seconds}s"
    );
    let grams = report.filament_g.expect("filament g");
    assert!(
        (1.0..40.0).contains(&grams),
        "20 mm cube filament out of range: {grams}g"
    );
    let processed = process_gcode(&gcode, &settings);
    assert!(processed.move_count > 100);
    assert!((processed.filament_g - grams).abs() < 0.02);
    assert!(gcode.contains("; total layer number:"));
    assert_eq!(
        report.total_layer_number.map(|n| n as usize),
        Some(sliced.layers.len())
    );
}

#[test]
fn cube_gcode_z_is_print_z() {
    let mesh = TriangleMesh::cube(20.0);
    let settings = SliceSettings::default();
    let sliced = slice_mesh(&mesh, &settings).unwrap();
    let gcode = write_gcode(&settings, &sliced).unwrap();
    assert!(gcode.contains("G1 Z0.200 F600"));
    assert!(gcode.contains("; Z_HEIGHT: 0.2\n"));
    assert!(gcode.contains("; Z_HEIGHT: 0.4\n"));
    assert!(gcode.contains("; LAYER_HEIGHT: 0.2\n"));
    assert!(gcode.contains("; LINE_WIDTH: 0.42\n"));
    assert!(gcode.contains("; max_z_height: 20.00"));
    let report = parse_gcode(&gcode);
    assert!((report.z_min - 0.2).abs() < 1e-6, "z_min={}", report.z_min);
    assert!((report.z_max - 20.0).abs() < 0.05, "z_max={}", report.z_max);
}

#[test]
fn cube_gcode_raft_lifts_object() {
    let mesh = TriangleMesh::cube(20.0);
    let mut settings = SliceSettings::default();
    settings.raft_layers = 2;
    let sliced = slice_mesh(&mesh, &settings).unwrap();
    let gcode = write_gcode(&settings, &sliced).unwrap();
    assert!(gcode.contains("; FEATURE: Support"));
    assert!(gcode.contains("; FEATURE: Support interface"));
    assert!(gcode.contains("G1 Z0.200 F600"));
    assert!(gcode.contains("G1 Z0.800 F600"));
    assert!(gcode.contains("; max_z_height: 20.60"));
    let report = parse_gcode(&gcode);
    assert!((report.z_min - 0.2).abs() < 1e-6, "z_min={}", report.z_min);
    assert!((report.z_max - 20.6).abs() < 0.05, "z_max={}", report.z_max);
}

#[test]
fn parse_does_not_double_count_layer_markers() {
    let gcode = "; CHANGE_LAYER\n;LAYER:0\nG1 Z0.200\n; FEATURE: Outer wall\nG1 X1 Y1 E0.1\n; CHANGE_LAYER\n;LAYER:1\nG1 Z0.400\n";
    let report = parse_gcode(gcode);
    assert_eq!(report.layer_changes, 2);
    assert_eq!(layer_stats(gcode).layer_comments, 2);
    assert!((report.z_max - 0.4).abs() < 1e-9);
}

#[test]
fn cube_bbl_gcode_has_brim_not_skirt() {
    let mesh = TriangleMesh::cube(20.0);
    let settings = SliceSettings::bbl_0_20();
    let sliced = slice_mesh(&mesh, &settings).unwrap();
    let gcode = write_gcode(&settings, &sliced).unwrap();
    assert!(gcode.contains("; FEATURE: Brim"));
    assert!(!gcode.contains("; FEATURE: Skirt"));
    let stats = layer_stats(&gcode);
    assert!((90..=105).contains(&stats.layer_comments), "{stats:?}");
}

#[test]
fn table_gcode_has_support() {
    let mesh = TriangleMesh::overhang_table(8.0, 8.0, 24.0, 4.0);
    let mut settings = SliceSettings::default();
    settings.enable_support = true;
    let sliced = slice_mesh(&mesh, &settings).unwrap();
    let gcode = write_gcode(&settings, &sliced).unwrap();
    assert!(gcode.contains("; FEATURE: Support") || gcode.contains("; FEATURE: Support interface"));
}

#[test]
fn support_interface_uses_interface_speed() {
    let mesh = TriangleMesh::overhang_table(8.0, 8.0, 24.0, 4.0);
    let mut settings = SliceSettings::default();
    settings.enable_support = true;
    settings.support_speed_mm_s = 40.0;
    settings.support_interface_speed_mm_s = 80.0;
    settings.first_layer_speed_mm_s = 20.0;
    settings.first_layer_infill_speed_mm_s = 20.0;
    settings.infill_speed_mm_s = 90.0;
    settings.solid_infill_speed_mm_s = 90.0;
    settings.filament_max_volumetric_speed_mm3_s = 0.0;
    let sliced = slice_mesh(&mesh, &settings).unwrap();
    assert!(sliced.layers.iter().any(|l| !l.support.is_empty()));
    assert!(sliced
        .layers
        .iter()
        .any(|l| !l.support_interface.is_empty()));
    let gcode = write_gcode(&settings, &sliced).unwrap();
    assert!(gcode.contains("; FEATURE: Support"));
    assert!(gcode.contains("; FEATURE: Support interface"));
    assert!(gcode.contains(" F2400"), "support 40 mm/s");
    assert!(gcode.contains(" F4800"), "support interface 80 mm/s");
}

#[test]
fn cube_ironing_gcode_feature() {
    let mesh = TriangleMesh::cube(20.0);
    let mut settings = SliceSettings::default();
    settings.ironing_type = bambu_config::IroningType::TopSurfaces;
    let sliced = slice_mesh(&mesh, &settings).unwrap();
    let gcode = write_gcode(&settings, &sliced).unwrap();
    assert!(gcode.contains("; FEATURE: Ironing"));
    let report = parse_gcode(&gcode);
    assert!(report.features.contains("Ironing"));
}

#[test]
fn cube_has_no_overhang_wall_feature() {
    let mesh = TriangleMesh::cube(20.0);
    let settings = SliceSettings::default();
    let sliced = slice_mesh(&mesh, &settings).unwrap();
    let gcode = write_gcode(&settings, &sliced).unwrap();
    assert!(!gcode.contains("; FEATURE: Overhang wall"));
}

#[test]
fn overhang_table_slows_unsupported_walls() {
    let mesh = TriangleMesh::overhang_table(8.0, 8.0, 24.0, 4.0);
    let mut settings = SliceSettings::default();
    settings.overhang_speed_mm_s = 25.0;
    let sliced = slice_mesh(&mesh, &settings).unwrap();
    let gcode = write_gcode(&settings, &sliced).unwrap();
    assert!(gcode.contains("; FEATURE: Overhang wall"));
    assert!(gcode.contains(" F1500\n") || gcode.contains(" F1500"));
    let report = parse_gcode(&gcode);
    assert!(report.features.contains("Overhang wall"));
}

#[test]
fn bbl_inner_walls_faster_than_outer() {
    let mesh = TriangleMesh::cube(20.0);
    let mut settings = SliceSettings::bbl_0_20();
    settings.filament_max_volumetric_speed_mm3_s = 0.0;
    settings.slow_down_for_layer_cooling = false;
    let sliced = slice_mesh(&mesh, &settings).unwrap();
    let gcode = write_gcode(&settings, &sliced).unwrap();
    assert!(gcode.contains(" F12000"), "outer walls should use 200 mm/s");
    assert!(gcode.contains(" F18000"), "inner walls should use 300 mm/s");
}

#[test]
fn volumetric_cap_slows_bbl_walls() {
    let mesh = TriangleMesh::cube(20.0);
    let mut settings = SliceSettings::bbl_0_20();
    settings.slow_down_for_layer_cooling = false;
    let mm3 = settings.line_width_mm * settings.layer_height_mm * settings.flow_ratio;
    let cap_f = settings.cap_extrude_feed_mm_min(12_000.0, mm3);
    assert!(cap_f < 12_000.0, "12 mm³/s should cap 200 mm/s walls");
    let sliced = slice_mesh(&mesh, &settings).unwrap();
    let gcode = write_gcode(&settings, &sliced).unwrap();
    let token = format!(" F{cap_f:.0}");
    assert!(
        gcode.contains(&token),
        "expected capped feed {token} in gcode"
    );
    assert!(
        !gcode.contains(" F18000"),
        "inner 300 mm/s should not survive the volumetric cap"
    );
}

#[test]
fn first_layer_uses_initial_speed() {
    let mesh = TriangleMesh::cube(20.0);
    let mut settings = SliceSettings::default();
    settings.first_layer_speed_mm_s = 20.0;
    let sliced = slice_mesh(&mesh, &settings).unwrap();
    let gcode = write_gcode(&settings, &sliced).unwrap();
    assert!(gcode.contains(" F1200"), "first layer 20 mm/s");
    assert!(gcode.contains(" F3000"), "later walls 50 mm/s");
}

#[test]
fn bridge_uses_bridge_speed() {
    let mesh = TriangleMesh::overhang_table(8.0, 8.0, 24.0, 4.0);
    let mut settings = SliceSettings::default();
    settings.enable_support = false;
    settings.infill_pattern = bambu_config::InfillPattern::Rectilinear;
    settings.bridge_speed_mm_s = 35.0;
    let sliced = slice_mesh(&mesh, &settings).unwrap();
    assert!(sliced.layers.iter().any(|l| !l.bridge.is_empty()));
    let gcode = write_gcode(&settings, &sliced).unwrap();
    assert!(gcode.contains("; FEATURE: Bridge"));
    assert!(gcode.contains(" F2100"), "bridge 35 mm/s");
}

#[test]
fn top_surface_uses_top_speed() {
    let mesh = TriangleMesh::cube(20.0);
    let mut settings = SliceSettings::default();
    settings.top_surface_speed_mm_s = 40.0;
    let sliced = slice_mesh(&mesh, &settings).unwrap();
    let gcode = write_gcode(&settings, &sliced).unwrap();
    assert!(gcode.contains("; FEATURE: Top surface"));
    assert!(gcode.contains(" F2400"), "top 40 mm/s");
}

#[test]
fn small_perimeter_slows_tiny_walls() {
    let mesh = TriangleMesh::cube(4.0);
    let mut settings = SliceSettings::default();
    settings.small_perimeter_threshold_mm = 6.0;
    settings.small_perimeter_speed = 50.0;
    settings.small_perimeter_speed_is_percent = true;
    settings.skirt_loops = 0;
    settings.brim_width_mm = 0.0;
    let sliced = slice_mesh(&mesh, &settings).unwrap();
    let gcode = write_gcode(&settings, &sliced).unwrap();
    assert!(
        gcode.contains(" F1500"),
        "later walls should be 50% of 50 mm/s"
    );
    assert!(
        gcode.contains(" F3000"),
        "first layer keeps initial_layer_speed"
    );
}

#[test]
fn small_perimeter_skips_when_threshold_zero() {
    let mesh = TriangleMesh::cube(4.0);
    let mut settings = SliceSettings::default();
    settings.small_perimeter_threshold_mm = 0.0;
    settings.skirt_loops = 0;
    settings.brim_width_mm = 0.0;
    let sliced = slice_mesh(&mesh, &settings).unwrap();
    let gcode = write_gcode(&settings, &sliced).unwrap();
    assert!(gcode.contains(" F3000"), "walls stay at 50 mm/s");
    assert!(
        !gcode.contains(" F1500"),
        "threshold 0 disables small-perimeter slowdown"
    );
}

#[test]
fn small_perimeter_skips_large_loops() {
    let mesh = TriangleMesh::cube(20.0);
    let mut settings = SliceSettings::default();
    settings.small_perimeter_threshold_mm = 6.5;
    settings.small_perimeter_speed = 50.0;
    settings.small_perimeter_speed_is_percent = true;
    settings.skirt_loops = 0;
    settings.brim_width_mm = 0.0;
    let sliced = slice_mesh(&mesh, &settings).unwrap();
    let gcode = write_gcode(&settings, &sliced).unwrap();
    assert!(
        !gcode.contains(" F1500"),
        "20 mm cube walls are larger than a 6.5 mm radius"
    );
}

#[test]
fn flow_ratio_scales_extrusion() {
    let mesh = TriangleMesh::cube(20.0);
    let mut full = SliceSettings::default();
    full.flow_ratio = 1.0;
    let sliced = slice_mesh(&mesh, &full).unwrap();
    let e_full = parse_gcode(&write_gcode(&full, &sliced).unwrap()).max_e;
    let mut scaled = full.clone();
    scaled.flow_ratio = 0.98;
    let e_scaled = parse_gcode(&write_gcode(&scaled, &sliced).unwrap()).max_e;
    assert!(e_full > 0.0);
    assert!((e_scaled / e_full - 0.98).abs() < 1e-6);
}

#[test]
fn layer_cooling_slows_short_bbl_layers() {
    let mesh = TriangleMesh::cube(20.0);
    let mut settings = SliceSettings::bbl_0_20();
    settings.filament_max_volumetric_speed_mm3_s = 0.0;
    let sliced = slice_mesh(&mesh, &settings).unwrap();
    let gcode = write_gcode(&settings, &sliced).unwrap();
    let fast_inner = gcode.lines().any(|line| {
        line.contains(" F18000") && !line.contains("_WIPE") && !line.contains("retract")
    });
    assert!(
        !fast_inner,
        "300 mm/s inner walls should be stretched for layer cooling"
    );
}

#[test]
fn pla_emits_part_fan_after_first_layer() {
    let mesh = TriangleMesh::cube(20.0);
    let settings = SliceSettings::bbl_0_20();
    let sliced = slice_mesh(&mesh, &settings).unwrap();
    let gcode = write_gcode(&settings, &sliced).unwrap();
    assert!(gcode.contains("M106 S0\n"), "first layer fan off");
    assert!(gcode.contains("M106 S255\n"), "later layers full PLA fan");
    let first_fan = gcode.find("M106 S0\n").expect("closed fan");
    let full_fan = gcode.find("M106 S255\n").expect("full fan");
    assert!(first_fan < full_fan);
}

#[test]
fn thin_wall_emits_gap_infill() {
    let mesh = TriangleMesh::box_mm(0.7, 20.0, 4.0);
    let mut settings = SliceSettings::default();
    settings.wall_loops = 2;
    settings.gap_infill_speed_mm_s = 45.0;
    settings.first_layer_speed_mm_s = 20.0;
    settings.filament_max_volumetric_speed_mm3_s = 0.0;
    settings.slow_down_for_layer_cooling = false;
    let sliced = slice_mesh(&mesh, &settings).unwrap();
    assert!(sliced.layers.iter().any(|l| !l.gap_infill.is_empty()));
    let gcode = write_gcode(&settings, &sliced).unwrap();
    assert!(gcode.contains("; FEATURE: Gap infill"));
    assert!(gcode.contains(" F2700"), "gap infill 45 mm/s");
}

#[test]
fn floating_vertical_shell_feature() {
    let mut mesh = TriangleMesh::box_mm(20.0, 20.0, 8.0);
    let mut rib = TriangleMesh::box_mm(4.0, 20.0, 12.0);
    for v in &mut rib.vertices {
        v.x += 8.0;
        v.z += 8.0;
    }
    mesh.append(&rib);
    let mut settings = SliceSettings::default();
    settings.wall_loops = 1;
    settings.infill_density = 0.15;
    settings.infill_pattern = bambu_config::InfillPattern::Rectilinear;
    settings.top_shell_layers = 3;
    settings.solid_infill_speed_mm_s = 80.0;
    settings.vertical_shell_speed = 80.0;
    settings.vertical_shell_speed_is_percent = true;
    settings.first_layer_speed_mm_s = 20.0;
    settings.filament_max_volumetric_speed_mm3_s = 0.0;
    settings.slow_down_for_layer_cooling = false;
    let sliced = slice_mesh(&mesh, &settings).unwrap();
    assert!(sliced
        .layers
        .iter()
        .any(|l| !l.floating_vertical_shell.is_empty()));
    let gcode = write_gcode(&settings, &sliced).unwrap();
    assert!(gcode.contains("; FEATURE: Floating vertical shell"));
    assert!(gcode.contains(" F3840"), "vertical shell 80% of 80 mm/s");
}

#[test]
fn default_cube_closes_fan_on_layer_zero() {
    let mesh = TriangleMesh::cube(20.0);
    let settings = SliceSettings::default();
    let sliced = slice_mesh(&mesh, &settings).unwrap();
    let gcode = write_gcode(&settings, &sliced).unwrap();
    assert!(gcode.contains("M106 S0\n"));
    assert!(gcode.contains("M106 S"));
}

#[test]
fn default_cube_skips_start_fan_when_close_is_zero() {
    let mesh = TriangleMesh::cube(20.0);
    let mut settings = SliceSettings::default();
    settings.close_fan_the_first_x_layers = 0;
    let sliced = slice_mesh(&mesh, &settings).unwrap();
    let gcode = write_gcode(&settings, &sliced).unwrap();
    let exec = executable_block(&gcode);
    let header_end = exec.find("; CHANGE_LAYER").expect("layers");
    let header = &exec[..header_end];
    assert!(
        !header.contains("M106 "),
        "start fan init is gated on close_fan_the_first_x_layers\n{header}"
    );
}

#[test]
fn overhang_table_boosts_part_fan() {
    let mesh = TriangleMesh::overhang_table(8.0, 8.0, 24.0, 4.0);
    let mut settings = SliceSettings::default();
    settings.enable_support = false;
    settings.fan_min_speed = 20;
    settings.fan_max_speed = 20;
    settings.close_fan_the_first_x_layers = 0;
    settings.reduce_fan_stop_start_freq = true;
    settings.overhang_fan_speed = 100;
    settings.overhang_fan_threshold = bambu_config::OverhangFanThreshold::Bridge;
    settings.slow_down_for_layer_cooling = false;
    let sliced = slice_mesh(&mesh, &settings).unwrap();
    let gcode = write_gcode(&settings, &sliced).unwrap();
    assert!(gcode.contains("; FEATURE: Overhang wall") || gcode.contains("; FEATURE: Bridge"));
    assert!(!gcode.contains(";_OVERHANG_FAN"));
    assert!(gcode.contains("M106 S51\n"), "layer fan 20%\n{gcode}");
    assert!(gcode.contains("M106 S255\n"), "overhang fan 100%");
}

#[test]
fn ironing_uses_ironing_fan_speed() {
    let mesh = TriangleMesh::cube(20.0);
    let mut settings = SliceSettings::default();
    settings.ironing_type = bambu_config::IroningType::TopSurfaces;
    settings.fan_min_speed = 100;
    settings.fan_max_speed = 100;
    settings.close_fan_the_first_x_layers = 1;
    settings.reduce_fan_stop_start_freq = true;
    settings.ironing_fan_speed = 40;
    settings.slow_down_for_layer_cooling = false;
    let sliced = slice_mesh(&mesh, &settings).unwrap();
    let gcode = write_gcode(&settings, &sliced).unwrap();
    assert!(gcode.contains("; FEATURE: Ironing"));
    assert!(!gcode.contains(";_IRONING_FAN"));
    assert!(gcode.contains("M106 S102\n"), "ironing 40%\n{gcode}");
    assert!(gcode.contains("M106 S255\n"), "layer 100%");
}

#[test]
fn long_travel_emits_retract_and_unretract() {
    let mesh = TriangleMesh::cube(20.0);
    let mut settings = SliceSettings::default();
    settings.wipe = false;
    settings.retract_when_changing_layer = false;
    settings.slow_down_for_layer_cooling = false;
    let sliced = slice_mesh(&mesh, &settings).unwrap();
    let gcode = write_gcode(&settings, &sliced).unwrap();
    assert!(gcode.contains(" ; retract"), "{gcode}");
    assert!(gcode.contains(" ; unretract"));
    assert!(
        gcode.contains("G1 E") && gcode.contains(" F1800"),
        "30 mm/s retract"
    );
    assert!(!gcode.contains("; WIPE_START"));
}

#[test]
fn short_travel_skips_retract() {
    let mesh = TriangleMesh::cube(20.0);
    let mut settings = SliceSettings::default();
    settings.retraction_minimum_travel_mm = 1.0e6;
    settings.retract_when_changing_layer = false;
    settings.wipe = false;
    settings.slow_down_for_layer_cooling = false;
    let sliced = slice_mesh(&mesh, &settings).unwrap();
    let gcode = write_gcode(&settings, &sliced).unwrap();
    assert!(!gcode.contains(" ; retract"), "{gcode}");
    assert!(!gcode.contains(" ; unretract"));
}

#[test]
fn zero_retract_length_disables() {
    let mesh = TriangleMesh::cube(20.0);
    let mut settings = SliceSettings::bbl_0_20();
    settings.retraction_length_mm = 0.0;
    settings.filament_max_volumetric_speed_mm3_s = 0.0;
    settings.slow_down_for_layer_cooling = false;
    let sliced = slice_mesh(&mesh, &settings).unwrap();
    let gcode = write_gcode(&settings, &sliced).unwrap();
    assert!(!gcode.contains(" ; retract"));
    assert!(!gcode.contains("; WIPE_START"));
}

#[test]
fn bbl_wipe_uses_reverse_path() {
    let mesh = TriangleMesh::cube(20.0);
    let mut settings = SliceSettings::bbl_0_20();
    settings.filament_max_volumetric_speed_mm3_s = 0.0;
    settings.slow_down_for_layer_cooling = false;
    let sliced = slice_mesh(&mesh, &settings).unwrap();
    let gcode = write_gcode(&settings, &sliced).unwrap();
    assert!(gcode.contains("; WIPE_START"));
    assert!(gcode.contains("; WIPE_END"));
    assert!(gcode.contains(";_WIPE"));
    assert!(gcode.contains("; unretract"));
    let start = gcode.find("; WIPE_START").unwrap();
    let end = gcode.find("; WIPE_END").unwrap();
    assert!(start < end);
    let wipe = &gcode[start..end];
    assert!(wipe.contains("G1 X"), "wipe travels along the last path");
}

#[test]
fn h2c_spiral_hops_on_travel() {
    let mesh = TriangleMesh::cube(20.0);
    let mut settings = SliceSettings::bbl_0_20();
    settings.filament_max_volumetric_speed_mm3_s = 0.0;
    settings.slow_down_for_layer_cooling = false;
    let sliced = slice_mesh(&mesh, &settings).unwrap();
    let gcode = write_gcode(&settings, &sliced).unwrap();
    assert!(gcode.contains("G17"), "{gcode}");
    assert!(gcode.contains("; spiral lift Z"));
    assert!(gcode.contains("; restore layer Z"));
    assert!(gcode.contains("G2 Z") && gcode.contains(" P1 F"));
    let lines: Vec<&str> = gcode.lines().collect();
    assert!(
        lines
            .windows(2)
            .any(|w| { w[0].contains("; spiral lift Z") && w[1].starts_with("G1 X") }),
        "lazy spiral should sit on the following XY travel\n{gcode}"
    );
    let mut saw_perp = false;
    for line in &lines {
        if !line.contains("; spiral lift Z") {
            continue;
        }
        let upper = line.to_ascii_uppercase();
        let i = parse_axis(&upper, b'I').unwrap_or(0.0);
        let j = parse_axis(&upper, b'J').unwrap_or(0.0);
        if j.abs() > 1e-6 {
            saw_perp = true;
        }
        assert!(
            (i * i + j * j).sqrt() > 0.5,
            "spiral radius should match 0.4 mm hop\n{line}"
        );
    }
    assert!(
        saw_perp,
        "lazy spiral I/J should follow travel, not +X only\n{gcode}"
    );
}

#[test]
fn default_cube_skips_z_hop() {
    let mesh = TriangleMesh::cube(20.0);
    let mut settings = SliceSettings::default();
    settings.wipe = false;
    settings.slow_down_for_layer_cooling = false;
    let sliced = slice_mesh(&mesh, &settings).unwrap();
    let gcode = write_gcode(&settings, &sliced).unwrap();
    assert!(!gcode.contains("; spiral lift Z"));
    assert!(!gcode.contains("; normal lift Z"));
    assert!(!gcode.contains("; restore layer Z"));
}

#[test]
fn h2c_emits_auxiliary_fan() {
    let mesh = TriangleMesh::cube(20.0);
    let mut settings = SliceSettings::bbl_0_20();
    settings.filament_max_volumetric_speed_mm3_s = 0.0;
    settings.slow_down_for_layer_cooling = false;
    let sliced = slice_mesh(&mesh, &settings).unwrap();
    let gcode = write_gcode(&settings, &sliced).unwrap();
    assert!(
        gcode.contains("M106 P2 S0\n"),
        "closed aux on first layers\n{gcode}"
    );
    assert!(
        gcode.contains("M106 P2 S191\n"),
        "H2C PLA 75% aux fan\n{gcode}"
    );
    assert!(!gcode.contains("M106 P3"), "filtration off on Generic PLA");
}

#[test]
fn exhaust_fan_when_filtration_enabled() {
    let mesh = TriangleMesh::cube(20.0);
    let mut settings = SliceSettings::default();
    settings.support_air_filtration = true;
    settings.activate_air_filtration = true;
    settings.during_print_exhaust_fan_speed = 70;
    settings.complete_print_exhaust_fan_speed = 80;
    settings.slow_down_for_layer_cooling = false;
    settings.wipe = false;
    let sliced = slice_mesh(&mesh, &settings).unwrap();
    let gcode = write_gcode(&settings, &sliced).unwrap();
    assert!(
        gcode.contains("M106 P3 S178\n"),
        "70% during print\n{gcode}"
    );
    assert!(gcode.contains("M106 P3 S204\n"), "80% after print\n{gcode}");
    let start = gcode.find("M106 P3 S178\n").expect("start exhaust");
    let end = gcode.find("M106 P3 S204\n").expect("end exhaust");
    assert!(start < end, "{gcode}");
}

#[test]
fn normal_lift_uses_g1_z() {
    let mesh = TriangleMesh::cube(20.0);
    let mut settings = SliceSettings::bbl_0_20();
    settings.z_hop_type = bambu_config::ZHopType::Normal;
    settings.filament_max_volumetric_speed_mm3_s = 0.0;
    settings.slow_down_for_layer_cooling = false;
    let sliced = slice_mesh(&mesh, &settings).unwrap();
    let gcode = write_gcode(&settings, &sliced).unwrap();
    assert!(gcode.contains("; normal lift Z"));
    assert!(!gcode.contains("; spiral lift Z"));
    assert!(gcode.contains("; restore layer Z"));
}

#[test]
fn slope_lift_emits_diagonal() {
    let mesh = TriangleMesh::cube(20.0);
    let mut settings = SliceSettings::bbl_0_20();
    settings.z_hop_type = bambu_config::ZHopType::Slope;
    settings.filament_max_volumetric_speed_mm3_s = 0.0;
    settings.slow_down_for_layer_cooling = false;
    let sliced = slice_mesh(&mesh, &settings).unwrap();
    let gcode = write_gcode(&settings, &sliced).unwrap();
    assert!(gcode.contains("; slope lift Z"), "{gcode}");
    assert!(!gcode.contains("; spiral lift Z"));
    assert!(gcode.contains("; restore layer Z"));
}

#[test]
fn spiral_falls_back_when_off_bed() {
    let mesh = TriangleMesh::cube(20.0);
    let mut settings = SliceSettings::bbl_0_20();
    settings.filament_max_volumetric_speed_mm3_s = 0.0;
    settings.slow_down_for_layer_cooling = false;
    settings.bed_min_x = 0.0;
    settings.bed_min_y = 0.0;
    settings.bed_max_x = 0.5;
    settings.bed_max_y = 0.5;
    let sliced = slice_mesh(&mesh, &settings).unwrap();
    let gcode = write_gcode(&settings, &sliced).unwrap();
    assert!(gcode.contains("; normal lift Z"), "{gcode}");
    assert!(!gcode.contains("; spiral lift Z"), "{gcode}");
    assert!(gcode.contains("; restore layer Z"));
}

#[test]
fn auto_hop_cube_uses_slope() {
    let mesh = TriangleMesh::cube(20.0);
    let mut settings = SliceSettings::bbl_0_20();
    settings.z_hop_type = bambu_config::ZHopType::Auto;
    settings.filament_max_volumetric_speed_mm3_s = 0.0;
    settings.slow_down_for_layer_cooling = false;
    settings.brim_width_mm = 0.0;
    settings.skirt_loops = 0;
    let sliced = slice_mesh(&mesh, &settings).unwrap();
    let gcode = write_gcode(&settings, &sliced).unwrap();
    assert!(gcode.contains("; slope lift Z"), "{gcode}");
    assert!(!gcode.contains("; spiral lift Z"), "{gcode}");
    assert!(gcode.contains("; restore layer Z"));
}

#[test]
fn auto_hop_over_air_uses_spiral() {
    let mesh = TriangleMesh::overhang_table(8.0, 8.0, 24.0, 4.0);
    let mut settings = SliceSettings::bbl_0_20();
    settings.z_hop_type = bambu_config::ZHopType::Auto;
    settings.filament_max_volumetric_speed_mm3_s = 0.0;
    settings.slow_down_for_layer_cooling = false;
    settings.enable_support = false;
    settings.brim_width_mm = 0.0;
    settings.skirt_loops = 0;
    settings.wipe = false;
    let sliced = slice_mesh(&mesh, &settings).unwrap();
    let gcode = write_gcode(&settings, &sliced).unwrap();
    assert!(
        gcode.contains("; spiral lift Z"),
        "travel over the table wing should spiral\n{gcode}"
    );
}

#[test]
fn no_slow_down_keeps_outer_wall_feed() {
    let mesh = TriangleMesh::cube(20.0);
    let mut settings = SliceSettings::bbl_0_20();
    settings.filament_max_volumetric_speed_mm3_s = 0.0;
    settings.no_slow_down_for_cooling_on_outwalls = true;
    let sliced = slice_mesh(&mesh, &settings).unwrap();
    let gcode = write_gcode(&settings, &sliced).unwrap();
    assert!(
        gcode.contains("_EXTERNAL_PERIMETER"),
        "outer walls should carry the C++ cooling marker\n{gcode}"
    );
    let outer_kept = gcode
        .lines()
        .any(|line| line.contains("_EXTERNAL_PERIMETER") && line.contains(" F12000"));
    assert!(
        outer_kept,
        "200 mm/s outer walls should not be stretched\n{gcode}"
    );
    let fast_inner = gcode.lines().any(|line| {
        line.contains(" F18000") && !line.contains("_WIPE") && !line.contains("retract")
    });
    assert!(
        !fast_inner,
        "inner walls should still stretch for layer cooling"
    );
}

#[test]
fn h2c_emits_role_accelerations() {
    let mesh = TriangleMesh::cube(20.0);
    let mut settings = SliceSettings::bbl_0_20();
    settings.filament_max_volumetric_speed_mm3_s = 0.0;
    settings.slow_down_for_layer_cooling = false;
    let sliced = slice_mesh(&mesh, &settings).unwrap();
    let gcode = write_gcode(&settings, &sliced).unwrap();
    assert!(
        gcode.contains("M204 S500\n"),
        "first layer print 500\n{gcode}"
    );
    assert!(gcode.contains("M204 S5000\n"), "outer wall 5000\n{gcode}");
    assert!(
        gcode.contains("M204 S6000\n"),
        "first-layer travel 6000\n{gcode}"
    );
    assert!(
        gcode.contains("M204 S10000\n"),
        "later travel 10000\n{gcode}"
    );
    assert!(
        !gcode.contains("; adjust acceleration"),
        "C++ full_gcode_comment is false"
    );
    let first_print = gcode.find("M204 S500\n").expect("first layer");
    let outer = gcode.find("M204 S5000\n").expect("outer");
    assert!(first_print < outer, "{gcode}");
}

#[test]
fn h2c_short_travel_to_outer_wall_uses_250() {
    let mesh = TriangleMesh::cube(20.0);
    let mut settings = SliceSettings::bbl_0_20();
    settings.filament_max_volumetric_speed_mm3_s = 0.0;
    settings.slow_down_for_layer_cooling = false;
    settings.retraction_minimum_travel_mm = 100.0;
    let sliced = slice_mesh(&mesh, &settings).unwrap();
    let gcode = write_gcode(&settings, &sliced).unwrap();
    let layer1 = gcode.find(";LAYER:1").expect("layer 1");
    assert!(
        !gcode[..layer1].contains("M204 S250"),
        "first layer must keep full travel accel\n{}",
        &gcode[..layer1]
    );
    assert!(
        gcode[layer1..].contains("M204 S250\n"),
        "short hop to an outer wall should use 250\n{}",
        &gcode[layer1..]
    );
}

#[test]
fn h2c_emits_layer_change_gcode() {
    let mesh = TriangleMesh::cube(20.0);
    let mut settings = SliceSettings::bbl_0_20();
    settings.filament_max_volumetric_speed_mm3_s = 0.0;
    settings.slow_down_for_layer_cooling = false;
    let sliced = slice_mesh(&mesh, &settings).unwrap();
    let gcode = write_gcode(&settings, &sliced).unwrap();
    let n = sliced.layers.len();
    let exec = executable_block(&gcode);
    assert!(
        exec.contains(&format!("; layer num/total_layer_count: 1/{n}")),
        "{exec}"
    );
    assert!(exec.contains("M73 L1"), "{exec}");
    assert!(exec.contains("M991 S0 P0 ;notify layer change"), "{exec}");
    assert!(exec.contains("M73 L2"), "{exec}");
    assert!(exec.contains("M991 S0 P1 ;notify layer change"), "{exec}");
    assert!(exec.contains("; open powerlost recovery"));
    assert!(exec.contains("M1003 S1"));
    let layer1 = exec.find(";LAYER:1").expect("layer 1");
    assert!(
        !exec[..layer1].contains("M1003 S1"),
        "power-loss recovery opens on the second layer"
    );
    assert!(exec.contains(";_SET_FAN_SPEED_CHANGING_LAYER"));
}

#[test]
fn default_cube_keeps_generic_marlin_footer() {
    let mesh = TriangleMesh::cube(20.0);
    let settings = SliceSettings::default();
    let sliced = slice_mesh(&mesh, &settings).unwrap();
    let gcode = write_gcode(&settings, &sliced).unwrap();
    assert!(gcode.contains("G28 X0 Y0"));
    assert!(gcode.contains("M84"));
    assert!(!gcode.contains("; MACHINE_END_GCODE_START"));
    assert!(!gcode.contains(";===== machine: H2C end ====="));
    assert!(gcode.contains("M104 S220") || gcode.contains("M104 S"));
    assert!(gcode.contains("\nG28\n") || gcode.lines().any(|l| l == "G28"));
}

#[test]
fn h2c_emits_machine_end_gcode() {
    let paths = bambu_config::bbl_oracle_paths().expect("upstream BambuStudio profiles");
    let mesh = TriangleMesh::cube(20.0);
    let mut settings = SliceSettings::bbl_0_20();
    settings.filament_max_volumetric_speed_mm3_s = 0.0;
    settings.slow_down_for_layer_cooling = false;
    bambu_config::overlay_bbl_profile(&mut settings, &paths.machine).unwrap();
    bambu_config::overlay_bbl_profile(&mut settings, &paths.filament).unwrap();
    settings.filament_max_volumetric_speed_mm3_s = 0.0;
    settings.slow_down_for_layer_cooling = false;
    let sliced = slice_mesh(&mesh, &settings).unwrap();
    let gcode = write_gcode(&settings, &sliced).unwrap();
    let z = sliced.layers.last().unwrap().print_z_mm;
    assert!(gcode.contains("; MACHINE_END_GCODE_START"), "{gcode}");
    assert!(gcode.contains(";===== machine: H2C end ====="), "{gcode}");
    assert!(gcode.contains("; filament end gcode"), "{gcode}");
    assert!(gcode.contains("M1003 S0"), "{gcode}");
    assert!(
        gcode.contains(&format!("G1 Z{} F900 ; lower z a little", z + 0.4))
            || gcode.contains(&format!("G1 Z{:.1} F900 ; lower z a little", z + 0.4)),
        "expected first Z park at {}+0.4\n{gcode}",
        z
    );
    assert!(
        gcode.contains("M620.11 P1 I0 B0 E-14 F"),
        "long retract-on-cut branch should expand\n{gcode}"
    );
    assert!(
        gcode.contains("M620.11 K1 I0 B0 R10 F"),
        "ec retract-on-cut should expand\n{gcode}"
    );
    let park = z + 100.0 - z / 2.0;
    let park_s = if (park - park.round()).abs() < 1e-6 {
        format!("{}", park.round() as i64)
    } else {
        let s = format!("{park:.5}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    };
    let end = gcode
        .rfind("; MACHINE_END_GCODE_START")
        .map(|i| &gcode[i..])
        .unwrap_or(&gcode);
    assert!(
        end.contains(&format!("G1 Z{park_s} F600")),
        "expected nested Z park at {park_s} (raw {park})\n{end}"
    );
    assert!(!gcode.contains("G28 X0 Y0"), "{gcode}");
    let exec = executable_block(&gcode);
    assert!(!exec.contains("{if"), "unexpanded if in\n{exec}");
    assert!(!exec.contains("{endif}"), "unexpanded endif in\n{exec}");
    assert!(!exec.contains("{max_layer_z"), "unexpanded z in\n{exec}");
    assert!(
        !exec.contains("M620.11 P0 I0 B0 E0"),
        "cut-retract else branch should be skipped\n{exec}"
    );
    assert!(
        !exec.contains("M620.11 K0 I0 B0 R0"),
        "ec retract else branch should be skipped\n{exec}"
    );
}

#[test]
fn h2c_emits_machine_start_gcode() {
    let paths = bambu_config::bbl_oracle_paths().expect("upstream BambuStudio profiles");
    let mesh = TriangleMesh::cube(20.0);
    let mut settings = SliceSettings::bbl_0_20();
    bambu_config::overlay_bbl_profile(&mut settings, &paths.machine).unwrap();
    bambu_config::overlay_bbl_profile(&mut settings, &paths.filament).unwrap();
    settings.filament_max_volumetric_speed_mm3_s = 0.0;
    settings.slow_down_for_layer_cooling = false;
    let sliced = slice_mesh(&mesh, &settings).unwrap();
    let gcode = write_gcode(&settings, &sliced).unwrap();
    let exec = executable_block(&gcode);
    let header_end = exec.find("; CHANGE_LAYER").expect("layers");
    let header = &exec[..header_end];
    assert!(
        header.contains("M201 X20000 Y20000 Z500 E5000"),
        "Marlin machine accel limits\n{header}"
    );
    assert!(
        header.contains("M203 X1000 Y1000 Z30 E50"),
        "H2C 0.4 feedrate limits\n{header}"
    );
    assert!(
        header.contains("M204 P20000 R5000 T20000"),
        "Marlin legacy travel uses extruding accel\n{header}"
    );
    assert!(
        header.contains("M205 X9.00 Y9.00 Z3.00 E2.50"),
        "H2C jerk envelope\n{header}"
    );
    let envelope_at = header.find("M201 X20000").expect("envelope");
    let start_at = header
        .find(";===== machine: H2C =========================")
        .expect("machine start");
    assert!(
        envelope_at < start_at,
        "print_machine_envelope precedes machine start"
    );
    let before_start = &header[..start_at];
    assert!(
        before_start.contains("M106 S0\n"),
        "close_fan_the_first_x_layers shuts the part fan before start\n{before_start}"
    );
    assert!(
        before_start.contains("M106 P2 S0\n"),
        "H2C auxiliary fan is forced off at start\n{before_start}"
    );
    let fan_at = before_start.find("M106 S0\n").expect("start fan");
    assert!(envelope_at < fan_at, "start fan follows machine envelope");
    assert!(
        header.contains(";===== machine: H2C ========================="),
        "{header}"
    );
    assert!(header.contains("; MACHINE_START_GCODE_END"), "{header}");
    assert!(header.contains("G28 X T300"), "{header}");
    assert!(
        header.contains("M145 P0"),
        "PLA uses cooling airduct\n{header}"
    );
    assert!(
        header.contains("M142 P1 R30 S40 T45"),
        "PLA 0.4 chamber autocool\n{header}"
    );
    assert!(header.contains("; filament start gcode"), "{header}");
    assert!(header.contains(";VT0 H0"), "{header}");
    assert!(
        header.contains("M82 ; use absolute distances for extrusion"),
        "{header}"
    );
    assert!(
        !header.lines().any(|l| l == "G28"),
        "generic home should be skipped\n{header}"
    );
    assert!(!header.contains("{if"), "unexpanded if in\n{header}");
    assert!(!header.contains("{endif}"), "unexpanded endif in\n{header}");
    assert!(!header.contains("{filament_type"), "{header}");
    assert!(!header.contains("{first_layer_print_min"), "{header}");
    assert!(!header.contains("{+0.0}"), "{header}");
    assert!(
        header.contains("T1 ; rise temp in advance"),
        "filament_map is 1-based so T1\n{header}"
    );
    assert!(
        header.contains("G151 P1 M"),
        "filament_map % 2 plugs heat nozzle 1\n{header}"
    );
    assert!(
        header.contains("M140 S35"),
        "Cool Plate PLA first-layer bed is 35\n{header}"
    );
    assert!(
        header.contains("M190 S35"),
        "Cool Plate PLA waits for 35\n{header}"
    );
    let filament = header
        .split_once("; MACHINE_START_GCODE_END")
        .map(|(_, rest)| rest)
        .unwrap_or("");
    assert!(
        !filament.contains("M106 P3 S255") && !filament.contains("M106 P3 S180"),
        "Cool Plate 35 skips filament chamber-fan branches\n{filament}"
    );
    assert!(
        header.contains("M640.1 S") && header.contains("M640.4"),
        "H2C always arms the AMS before the first-filament gate\n{header}"
    );
    assert!(
        !header.contains("M640.8") && !header.contains("M640.7 U") && !header.contains("M640.2 R1"),
        "H2C physical remap leaves first_non_support_filaments[0] == -1\n{header}"
    );
}

#[test]
fn default_cube_skips_layer_change_gcode() {
    let mesh = TriangleMesh::cube(20.0);
    let settings = SliceSettings::default();
    let sliced = slice_mesh(&mesh, &settings).unwrap();
    let gcode = write_gcode(&settings, &sliced).unwrap();
    assert!(!gcode.contains("M991 S0 P"));
    assert!(!executable_block(&gcode).contains("M1003 S1"));
    assert!(!gcode.contains("M981 "));
}

#[test]
fn cube_gcode_has_bbl_envelope() {
    let mesh = TriangleMesh::cube(20.0);
    let settings = SliceSettings::default();
    let sliced = slice_mesh(&mesh, &settings).unwrap();
    let gcode = write_gcode(&settings, &sliced).unwrap();
    assert!(gcode.contains("; HEADER_BLOCK_START"));
    assert!(gcode.contains("; HEADER_BLOCK_END"));
    assert!(gcode.contains("; CONFIG_BLOCK_START"));
    assert!(gcode.contains("; wall_loops = "));
    assert!(gcode.contains("; CONFIG_BLOCK_END"));
    assert!(gcode.contains("; EXECUTABLE_BLOCK_START"));
    assert!(gcode.contains("; EXECUTABLE_BLOCK_END"));
    assert!(gcode.contains("M73 P0 R"));
    assert!(gcode.contains("M73 P100 R0"));
    assert!(!gcode.contains("_GP_"));
    let report = parse_gcode(&gcode);
    assert_eq!(
        report.total_layer_number.map(|n| n as usize),
        Some(sliced.layers.len())
    );
    let cfg = parse_config_comments(&gcode);
    assert_eq!(cfg.get("wall_loops").map(String::as_str), Some("2"));
    assert!(gcode.contains("; filament_density: 1.24"));
    assert!(gcode.contains("; filament_diameter: 1.75"));
    assert!(!gcode.contains("M981 "));
    assert!(gcode.contains("M201 X1000 Y1000 Z500 E5000"));
    assert!(gcode.contains("M203 X500 Y500 Z12 E120"));
    assert!(gcode.contains("M204 P1500 R1500 T1500"));
    let exec = executable_block(&gcode);
    let header_end = exec.find("; CHANGE_LAYER").expect("layers");
    let header = &exec[..header_end];
    assert!(
        header.contains("M106 S0\n"),
        "default close_fan_the_first_x_layers shuts the part fan\n{header}"
    );
    assert!(
        !header.contains("M106 P2"),
        "default machine has no aux fan\n{header}"
    );
}

#[test]
fn h2c_opens_and_closes_spaghetti_detector() {
    let paths = bambu_config::bbl_oracle_paths().expect("upstream BambuStudio profiles");
    let mesh = TriangleMesh::cube(20.0);
    let mut settings = SliceSettings::bbl_0_20();
    bambu_config::overlay_bbl_profile(&mut settings, &paths.machine).unwrap();
    bambu_config::overlay_bbl_profile(&mut settings, &paths.filament).unwrap();
    settings.filament_max_volumetric_speed_mm3_s = 0.0;
    settings.slow_down_for_layer_cooling = false;
    let sliced = slice_mesh(&mesh, &settings).unwrap();
    let gcode = write_gcode(&settings, &sliced).unwrap();
    let exec = executable_block(&gcode);
    let open = exec.find("M981 S1 P20000 ;open spaghetti detector");
    let close = exec.find("M981 S0 P20000 ; close spaghetti detector");
    assert!(open.is_some(), "missing spaghetti open\n{exec}");
    assert!(close.is_some(), "missing spaghetti close\n{exec}");
    assert!(
        open.unwrap() < close.unwrap(),
        "spaghetti detector should open before close"
    );
    let start_end = exec.find("; MACHINE_START_GCODE_END").expect("start end");
    let end_start = exec.find("; MACHINE_END_GCODE_START").expect("end start");
    assert!(
        open.unwrap() > start_end,
        "spaghetti open should follow machine start"
    );
    assert!(
        close.unwrap() < end_start,
        "spaghetti close should precede machine end"
    );
    assert!(gcode.contains("; HEADER_BLOCK_START"));
    assert!(gcode.contains("; CONFIG_BLOCK_START"));
    assert!(exec.contains("M73 P0 R"));
    assert!(exec.contains("M73 P100 R0"));
}

#[test]
fn default_cube_skips_time_lapse_gcode() {
    let mesh = TriangleMesh::cube(20.0);
    let settings = SliceSettings::default();
    let sliced = slice_mesh(&mesh, &settings).unwrap();
    let gcode = write_gcode(&settings, &sliced).unwrap();
    assert!(!gcode.contains("SKIPTYPE: timelapse"));
    assert!(!gcode.contains("M9711 "));
    assert!(!gcode.contains(";===== machine: H2C timelapse ====="));
}

#[test]
fn h2c_emits_time_lapse_gcode_each_layer() {
    let paths = bambu_config::bbl_oracle_paths().expect("upstream BambuStudio profiles");
    let mesh = TriangleMesh::cube(20.0);
    let mut settings = SliceSettings::bbl_0_20();
    bambu_config::overlay_bbl_profile(&mut settings, &paths.machine).unwrap();
    bambu_config::overlay_bbl_profile(&mut settings, &paths.filament).unwrap();
    settings.filament_max_volumetric_speed_mm3_s = 0.0;
    settings.slow_down_for_layer_cooling = false;
    let sliced = slice_mesh(&mesh, &settings).unwrap();
    let gcode = write_gcode(&settings, &sliced).unwrap();
    let exec = executable_block(&gcode);
    let n = sliced.layers.len();
    let shots = exec.matches("SKIPTYPE: timelapse").count();
    assert_eq!(shots, n, "one timelapse insert per layer, got {shots}");
    assert!(exec.contains(";===== machine: H2C timelapse ====="));
    assert!(
        exec.contains("M9711 M0 E1 U"),
        "H2C cube should pick a safe pos (physical E1)\n{}",
        exec.lines()
            .filter(|l| l.contains("M9711"))
            .take(3)
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert!(
        exec.lines()
            .any(|l| l.contains("M9711 ") && l.contains(" V")),
        "safe-pos M9711 includes V\n{}",
        exec.lines()
            .filter(|l| l.contains("M9711"))
            .take(3)
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert!(exec.contains("M993 A2 B2 C2"));
    assert!(!exec.contains("{if"), "unexpanded if in timelapse\n{exec}");
    assert!(!exec.contains("{endif}"), "unexpanded endif in timelapse");
    let last_layer = exec.rfind(";LAYER:").expect("last layer");
    let last_tl = exec.rfind("SKIPTYPE: timelapse").expect("last timelapse");
    let close = exec
        .find("M981 S0 P20000 ; close spaghetti detector")
        .expect("spaghetti close");
    assert!(
        last_layer < last_tl && last_tl < close,
        "timelapse should close each layer before spaghetti off"
    );
    let zs = m9711_z_values(exec);
    assert!(!zs.is_empty(), "missing M9711 Z");
    assert!(
        (zs[0] - sliced.layers[0].print_z_mm).abs() < 1e-3,
        "corexy farthest-point uses layer_z, got {}\n{}",
        zs[0],
        exec.lines()
            .filter(|l| l.contains("M9711"))
            .take(2)
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn h2c_i3_timelapse_adds_legacy_z_offset() {
    let paths = bambu_config::bbl_oracle_paths().expect("upstream BambuStudio profiles");
    let mesh = TriangleMesh::cube(20.0);
    let mut settings = SliceSettings::bbl_0_20();
    bambu_config::overlay_bbl_profile(&mut settings, &paths.machine).unwrap();
    bambu_config::overlay_bbl_profile(&mut settings, &paths.filament).unwrap();
    settings.filament_max_volumetric_speed_mm3_s = 0.0;
    settings.slow_down_for_layer_cooling = false;
    settings.printer_structure = String::from("i3");
    assert!(!settings.farthest_point_timelapse_enabled());
    let sliced = slice_mesh(&mesh, &settings).unwrap();
    let gcode = write_gcode(&settings, &sliced).unwrap();
    let exec = executable_block(&gcode);
    let zs = m9711_z_values(exec);
    assert!(!zs.is_empty(), "missing M9711 Z");
    assert!(
        (zs[0] - (sliced.layers[0].print_z_mm + 0.4)).abs() < 1e-3,
        "I3 farthest-point off uses layer_z + 0.4, got {}\n{}",
        zs[0],
        exec.lines()
            .filter(|l| l.contains("M9711"))
            .take(2)
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn h2c_filament_start_emits_exhaust_pwm_when_filtration_on() {
    let paths = bambu_config::bbl_oracle_paths().expect("upstream BambuStudio profiles");
    let mesh = TriangleMesh::cube(20.0);
    let mut settings = SliceSettings::bbl_0_20();
    bambu_config::overlay_bbl_profile(&mut settings, &paths.machine).unwrap();
    bambu_config::overlay_bbl_profile(&mut settings, &paths.filament).unwrap();
    settings.filament_max_volumetric_speed_mm3_s = 0.0;
    settings.slow_down_for_layer_cooling = false;
    settings.support_air_filtration = true;
    settings.activate_air_filtration = true;
    settings.during_print_exhaust_fan_speed = 70;
    let sliced = slice_mesh(&mesh, &settings).unwrap();
    let gcode = write_gcode(&settings, &sliced).unwrap();
    let exec = executable_block(&gcode);
    let header_end = exec.find("; CHANGE_LAYER").expect("layers");
    let header = &exec[..header_end];
    let filament = header
        .split_once("; MACHINE_START_GCODE_END")
        .map(|(_, rest)| rest)
        .unwrap_or("");
    assert!(
        filament.contains("M106 P3 S178"),
        "filament start uses during_print_exhaust_fan_speed_num\n{filament}"
    );
}

#[test]
fn default_cube_skips_wrapping_detection() {
    let mesh = TriangleMesh::cube(20.0);
    let settings = SliceSettings::default();
    let sliced = slice_mesh(&mesh, &settings).unwrap();
    let gcode = write_gcode(&settings, &sliced).unwrap();
    let exec = executable_block(&gcode);
    assert_eq!(exec.lines().filter(|l| l.trim() == "G39").count(), 0);
    assert!(!exec.contains("G1 Y295 F30000"));
}

#[test]
fn h2c_skips_wrapping_detection_when_disabled() {
    let paths = bambu_config::bbl_oracle_paths().expect("upstream BambuStudio profiles");
    let mesh = TriangleMesh::cube(20.0);
    let mut settings = SliceSettings::bbl_0_20();
    bambu_config::overlay_bbl_profile(&mut settings, &paths.machine).unwrap();
    bambu_config::overlay_bbl_profile(&mut settings, &paths.filament).unwrap();
    settings.filament_max_volumetric_speed_mm3_s = 0.0;
    settings.slow_down_for_layer_cooling = false;
    assert!(!settings.enable_wrapping_detection);
    assert!(settings.wrapping_detection_gcode.contains("G39"));
    let sliced = slice_mesh(&mesh, &settings).unwrap();
    let gcode = write_gcode(&settings, &sliced).unwrap();
    let exec = executable_block(&gcode);
    assert!(
        !exec.contains("date: 20251104"),
        "wrapping header should stay out of the executable when disabled"
    );
    assert_eq!(
        wrapping_g39_layers(exec),
        Vec::<usize>::new(),
        "wrapping off should skip layer G39"
    );
}

#[test]
fn h2c_emits_wrapping_detection_on_layers_3_10_19() {
    let paths = bambu_config::bbl_oracle_paths().expect("upstream BambuStudio profiles");
    let mesh = TriangleMesh::cube(20.0);
    let mut settings = SliceSettings::bbl_0_20();
    bambu_config::overlay_bbl_profile(&mut settings, &paths.machine).unwrap();
    bambu_config::overlay_bbl_profile(&mut settings, &paths.filament).unwrap();
    settings.filament_max_volumetric_speed_mm3_s = 0.0;
    settings.slow_down_for_layer_cooling = false;
    settings.enable_wrapping_detection = true;
    let sliced = slice_mesh(&mesh, &settings).unwrap();
    assert!(sliced.layers.len() > 19, "cube should reach layer 19");
    let gcode = write_gcode(&settings, &sliced).unwrap();
    let exec = executable_block(&gcode);
    let n = sliced.layers.len();
    let headers = exec.matches("date: 20251104").count();
    assert_eq!(
        headers, n,
        "wrapping header once per layer, got {headers} for {n} layers"
    );
    assert_eq!(wrapping_g39_layers(exec), vec![3, 10, 19]);
    assert_eq!(
        exec.matches("nozzle cam detection allow status save")
            .count(),
        3
    );
    assert!(!exec.contains("{if"), "unexpanded wrapping if\n{exec}");
    assert!(!exec.contains("{endif}"), "unexpanded wrapping endif");
    for layer in [3usize, 10, 19] {
        let marker = format!(";LAYER:{layer}");
        let start = exec
            .match_indices(&marker)
            .find_map(|(i, _)| {
                let next = exec.as_bytes().get(i + marker.len());
                match next {
                    None | Some(b'\n') | Some(b'\r') => Some(i),
                    Some(c) if !c.is_ascii_digit() => Some(i),
                    _ => None,
                }
            })
            .unwrap_or_else(|| panic!("missing {marker}"));
        let rest = &exec[start..];
        let end = rest[1..]
            .find(";LAYER:")
            .map(|i| i + 1)
            .unwrap_or(rest.len());
        let block = &rest[..end];
        let fan = block.find(";_SET_FAN_SPEED_CHANGING_LAYER");
        let g39 = block.find("G39");
        let paths = block
            .find("; FEATURE: Skirt")
            .or_else(|| block.find("; FEATURE: Brim"))
            .or_else(|| block.find("; FEATURE: Outer wall"));
        assert!(
            fan.is_some() && g39.is_some(),
            "layer {layer} wrapping after fan marker"
        );
        assert!(
            fan.unwrap() < g39.unwrap(),
            "wrapping should follow layer-change fan marker"
        );
        if let Some(paths) = paths {
            assert!(
                g39.unwrap() < paths,
                "wrapping should precede this layer's paths"
            );
        }
    }
}

#[test]
fn default_cube_skips_first_layer_scan() {
    let mesh = TriangleMesh::cube(20.0);
    let settings = SliceSettings::default();
    let sliced = slice_mesh(&mesh, &settings).unwrap();
    let gcode = write_gcode(&settings, &sliced).unwrap();
    assert!(!gcode.contains("M976 S1 P1"));
}

#[test]
fn h2c_skips_first_layer_scan_by_default() {
    let paths = bambu_config::bbl_oracle_paths().expect("upstream BambuStudio profiles");
    let mesh = TriangleMesh::cube(20.0);
    let mut settings = SliceSettings::bbl_0_20();
    bambu_config::overlay_bbl_profile(&mut settings, &paths.machine).unwrap();
    bambu_config::overlay_bbl_profile(&mut settings, &paths.filament).unwrap();
    settings.filament_max_volumetric_speed_mm3_s = 0.0;
    settings.slow_down_for_layer_cooling = false;
    assert!(!settings.scan_first_layer);
    let sliced = slice_mesh(&mesh, &settings).unwrap();
    let gcode = write_gcode(&settings, &sliced).unwrap();
    assert!(!executable_block(&gcode).contains("M976 S1 P1"));
}

#[test]
fn scan_first_layer_emits_m976_on_second_layer() {
    let mesh = TriangleMesh::cube(20.0);
    let mut settings = SliceSettings::default();
    settings.scan_first_layer = true;
    let sliced = slice_mesh(&mesh, &settings).unwrap();
    let gcode = write_gcode(&settings, &sliced).unwrap();
    let exec = executable_block(&gcode);
    let layer1 = exec.find(";LAYER:1").expect("layer 1");
    let scan = exec
        .find("M976 S1 P1 ; scan model before printing 2nd layer")
        .expect("scan");
    assert!(
        !exec[..layer1].contains("M976 S1 P1"),
        "scan should wait for the second layer"
    );
    assert!(scan > layer1);
    let fan = exec[layer1..]
        .find(";_SET_FAN_SPEED_CHANGING_LAYER")
        .expect("fan");
    assert!(
        scan < layer1 + fan,
        "scan should precede this layer's fan marker"
    );
    assert!(exec.contains("M400 P100"));
}

#[test]
fn default_cube_skips_second_layer_temp_transition() {
    let mesh = TriangleMesh::cube(20.0);
    let settings = SliceSettings::default();
    let sliced = slice_mesh(&mesh, &settings).unwrap();
    let gcode = write_gcode(&settings, &sliced).unwrap();
    let exec = executable_block(&gcode);
    let layer1 = layer_block(exec, 1).expect("layer 1");
    assert!(!layer1.contains("; set nozzle temperature"));
    assert!(!layer1.contains("; set bed temperature"));
    assert!(exec.contains("M104 S220"));
    assert!(exec.contains("M140 S60"));
}

#[test]
fn second_layer_emits_m104_m140_when_temps_change() {
    let mesh = TriangleMesh::cube(20.0);
    let mut settings = SliceSettings::default();
    settings.temperature_initial_layer_c = 230;
    settings.temperature_c = 220;
    settings.bed_temperature_initial_layer_c = 65;
    settings.bed_temperature_c = 55;
    let sliced = slice_mesh(&mesh, &settings).unwrap();
    let gcode = write_gcode(&settings, &sliced).unwrap();
    let exec = executable_block(&gcode);
    let header_end = exec.find("; CHANGE_LAYER").expect("layers");
    let header = &exec[..header_end];
    assert!(header.contains("M104 S230"), "{header}");
    assert!(header.contains("M140 S65"), "{header}");
    let layer1 = layer_block(exec, 1).expect("layer 1");
    assert!(
        layer1.contains("M104 S220 ; set nozzle temperature"),
        "{layer1}"
    );
    assert!(
        layer1.contains("M140 S55 ; set bed temperature"),
        "{layer1}"
    );
    let fan = layer1.find(";_SET_FAN_SPEED_CHANGING_LAYER").expect("fan");
    let nozzle = layer1.find("M104 S220 ; set nozzle temperature").unwrap();
    assert!(nozzle < fan, "second-layer temps precede the fan marker");
}

#[test]
fn h2c_cool_plate_skips_second_layer_bed_temp() {
    let paths = bambu_config::bbl_oracle_paths().expect("upstream BambuStudio profiles");
    let mesh = TriangleMesh::cube(20.0);
    let mut settings = SliceSettings::bbl_0_20();
    bambu_config::overlay_bbl_profile(&mut settings, &paths.machine).unwrap();
    bambu_config::overlay_bbl_profile(&mut settings, &paths.filament).unwrap();
    settings.filament_max_volumetric_speed_mm3_s = 0.0;
    settings.slow_down_for_layer_cooling = false;
    assert_eq!(settings.bed_temperature_c, 35);
    assert_eq!(settings.bed_temperature_initial_layer_c, 35);
    let sliced = slice_mesh(&mesh, &settings).unwrap();
    let gcode = write_gcode(&settings, &sliced).unwrap();
    let exec = executable_block(&gcode);
    let layer1 = layer_block(exec, 1).expect("layer 1");
    assert!(!layer1.contains("; set nozzle temperature"), "{layer1}");
    assert!(!layer1.contains("; set bed temperature"), "{layer1}");
}

fn layer_block(exec: &str, layer: usize) -> Option<&str> {
    let tag = format!(";LAYER:{layer}");
    let mut from = 0;
    let start = loop {
        let rel = exec[from..].find(&tag)?;
        let abs = from + rel;
        let after = abs + tag.len();
        let next = exec.as_bytes().get(after);
        if next.map(|c| !c.is_ascii_digit()).unwrap_or(true) {
            break abs;
        }
        from = after;
    };
    let rest = &exec[start + tag.len()..];
    let end = rest
        .find(";LAYER:")
        .map(|i| start + tag.len() + i)
        .unwrap_or(exec.len());
    Some(&exec[start..end])
}

fn m9711_z_values(exec: &str) -> Vec<f64> {
    exec.lines()
        .filter(|l| l.contains("M9711 "))
        .filter_map(|l| {
            l.split_whitespace()
                .find(|w| w.starts_with('Z') && w.len() > 1)
                .and_then(|w| w[1..].parse().ok())
        })
        .collect()
}

fn wrapping_g39_layers(exec: &str) -> Vec<usize> {
    let mut current = None;
    let mut hits = Vec::new();
    for line in exec.lines() {
        if let Some(rest) = line.strip_prefix(";LAYER:") {
            current = rest.trim().parse().ok();
        }
        if line.trim() == "G39" {
            if let Some(n) = current {
                hits.push(n);
            }
        }
    }
    hits
}

fn executable_block(gcode: &str) -> &str {
    let start = gcode.find("; EXECUTABLE_BLOCK_START").unwrap_or(0);
    let end = gcode
        .rfind("; EXECUTABLE_BLOCK_END")
        .map(|i| i + "; EXECUTABLE_BLOCK_END".len())
        .unwrap_or(gcode.len());
    &gcode[start..end]
}

fn is_rapid_g0(line: &str) -> bool {
    let cmd = match line.find(';') {
        Some(i) => line[..i].trim(),
        None => line.trim(),
    };
    let upper = cmd.to_ascii_uppercase();
    upper == "G0" || upper.starts_with("G0 ") || upper.starts_with("G00")
}
