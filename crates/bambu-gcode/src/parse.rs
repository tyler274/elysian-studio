//! Read G-code comments and moves for tests and the C++ golden cube.

use std::collections::{BTreeMap, BTreeSet};

/// Count layer-change comments without double-counting `; CHANGE_LAYER` + `;LAYER:`.
pub fn layer_stats(gcode: &str) -> LayerStats {
    let report = parse_gcode(gcode);
    LayerStats {
        layer_comments: report.layer_changes,
        unique_z: report.unique_z,
    }
}

#[derive(Debug, Clone, Copy)]
pub struct LayerStats {
    pub layer_comments: usize,
    pub unique_z: usize,
}

/// Structured view of G-code for comparing the rewrite to C++ Bambu Studio.
#[derive(Debug, Clone)]
pub struct GcodeReport {
    pub layer_changes: usize,
    pub unique_z: usize,
    pub z_min: f64,
    pub z_max: f64,
    pub max_e: f64,
    pub total_layer_number: Option<u32>,
    pub features: BTreeSet<String>,
    pub estimated_seconds: Option<f64>,
    pub filament_g: Option<f64>,
}

pub fn parse_gcode(gcode: &str) -> GcodeReport {
    let mut change_layer = 0usize;
    let mut layer_colon = 0usize;
    let mut layer_z = Vec::new();
    let mut pending_layer_z: Option<f64> = None;
    let mut in_layer_header = false;
    let mut max_e = 0.0_f64;
    let mut features = BTreeSet::new();
    let mut total_layer_number = None;
    let mut max_z_height = None;
    let mut estimated_seconds = None;
    let mut filament_g = None;
    for line in gcode.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("; CHANGE_LAYER") || trimmed == ";CHANGE_LAYER" {
            if let Some(z) = pending_layer_z.take() {
                layer_z.push(z);
            }
            change_layer += 1;
            in_layer_header = true;
            continue;
        }
        if trimmed.starts_with(";LAYER:") {
            if change_layer == 0 && pending_layer_z.is_none() && !in_layer_header {
                in_layer_header = true;
            }
            layer_colon += 1;
        }
        if let Some(rest) = trimmed.strip_prefix("; total layer number:") {
            if let Ok(n) = rest.trim().parse::<u32>() {
                total_layer_number = Some(n);
            }
        }
        if let Some(rest) = trimmed.strip_prefix("; max_z_height:") {
            if let Ok(z) = rest.trim().parse::<f64>() {
                max_z_height = Some(z);
            }
        }
        if let Some(rest) = trimmed.strip_prefix("; model printing time:") {
            if let Some((clock, _)) = rest.split_once(';') {
                estimated_seconds = parse_time_dhms(clock.trim());
            }
        }
        if let Some(rest) = trimmed.strip_prefix("; total filament weight [g]") {
            let value = rest.trim().trim_start_matches(':').trim();
            if let Ok(g) = value.split(',').next().unwrap_or("").trim().parse::<f64>() {
                filament_g = Some(g);
            }
        }
        if let Some(rest) = trimmed
            .strip_prefix("; FEATURE:")
            .or_else(|| trimmed.strip_prefix(";FEATURE:"))
        {
            if let Some(z) = pending_layer_z.take() {
                layer_z.push(z);
            }
            in_layer_header = false;
            features.insert(rest.trim().to_string());
        }
        if in_layer_header {
            if let Some(z) = parse_g1_z(trimmed) {
                pending_layer_z = Some(z);
            }
        }
        if let Some(e) = parse_axis_g1(trimmed, b'E') {
            if e > max_e {
                max_e = e;
            }
        }
    }
    if let Some(z) = pending_layer_z {
        layer_z.push(z);
    }
    let layer_changes = if change_layer > 0 {
        change_layer
    } else {
        layer_colon
    };
    let z_min = layer_z.iter().copied().fold(f64::INFINITY, f64::min);
    let z_max =
        max_z_height.unwrap_or_else(|| layer_z.iter().copied().fold(f64::NEG_INFINITY, f64::max));
    GcodeReport {
        layer_changes,
        unique_z: layer_z.len(),
        z_min: if z_min.is_finite() { z_min } else { 0.0 },
        z_max: if z_max.is_finite() { z_max } else { 0.0 },
        max_e,
        total_layer_number,
        features,
        estimated_seconds,
        filament_g,
    }
}

/// Key/value pairs from C++ `; CONFIG_BLOCK` comments (`; wall_loops = 2`).
pub fn parse_config_comments(gcode: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for line in gcode.lines() {
        let trimmed = line.trim();
        let Some(rest) = trimmed.strip_prefix("; ") else {
            continue;
        };
        if rest.starts_with("FEATURE:") || rest.starts_with("CHANGE_LAYER") {
            continue;
        }
        let Some((key, value)) = rest.split_once(" = ") else {
            continue;
        };
        if key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            out.insert(key.to_string(), value.to_string());
        }
    }
    out
}

/// Compare rewrite G-code to C++ Bambu Studio on geometry-level signals.
pub fn assert_matches_cpp(ours: &GcodeReport, cpp: &GcodeReport) {
    let cpp_layers = cpp
        .total_layer_number
        .map(|n| n as usize)
        .unwrap_or(cpp.layer_changes);
    let delta = ours.layer_changes.abs_diff(cpp_layers);
    assert!(
        delta <= 5,
        "layer count diverged: rust={} cpp={} (CHANGE_LAYER={}, header={:?}) ours={ours:?} cpp={cpp:?}",
        ours.layer_changes,
        cpp_layers,
        cpp.layer_changes,
        cpp.total_layer_number
    );
    assert!(
        (ours.z_max - cpp.z_max).abs() <= 0.6,
        "max Z diverged: rust={} cpp={}",
        ours.z_max,
        cpp.z_max
    );
    for role in [
        "Outer wall",
        "Inner wall",
        "Sparse infill",
        "Bottom surface",
        "Top surface",
        "Internal solid infill",
    ] {
        assert!(
            ours.features.contains(role),
            "rewrite missing FEATURE {role}: {:?}",
            ours.features
        );
        assert!(
            cpp.features.contains(role),
            "C++ Bambu Studio missing FEATURE {role}: {:?}",
            cpp.features
        );
    }
    assert!(
        ours.max_e > 0.0,
        "rewrite G-code has no extrusion (max E={})",
        ours.max_e
    );
    assert!(
        cpp.max_e > 0.0 || cpp.features.iter().any(|f| f != "Custom" && f != "Travel"),
        "C++ G-code has no extrusion (max E={}, features={:?})",
        cpp.max_e,
        cpp.features
    );
}

/// Geometry-level C++ compare with caller-chosen FEATURE roles and slop.
pub fn assert_matches_cpp_with(
    ours: &GcodeReport,
    cpp: &GcodeReport,
    required_roles: &[&str],
    layer_slop: usize,
    z_slop_mm: f64,
) {
    let cpp_layers = cpp
        .total_layer_number
        .map(|n| n as usize)
        .unwrap_or(cpp.layer_changes);
    let delta = ours.layer_changes.abs_diff(cpp_layers);
    assert!(
        delta <= layer_slop,
        "layer count diverged: rust={} cpp={} (CHANGE_LAYER={}, header={:?}, slop={layer_slop}) ours={ours:?} cpp={cpp:?}",
        ours.layer_changes,
        cpp_layers,
        cpp.layer_changes,
        cpp.total_layer_number
    );
    assert!(
        (ours.z_max - cpp.z_max).abs() <= z_slop_mm,
        "max Z diverged: rust={} cpp={} (slop={z_slop_mm})",
        ours.z_max,
        cpp.z_max
    );
    for role in required_roles {
        if cpp.features.contains(*role) {
            assert!(
                ours.features.contains(*role),
                "rewrite missing FEATURE {role} (C++ has it): rust={:?} cpp={:?}",
                ours.features,
                cpp.features
            );
        }
    }
    assert!(
        ours.max_e > 0.0,
        "rewrite G-code has no extrusion (max E={})",
        ours.max_e
    );
    assert!(
        cpp.max_e > 0.0 || cpp.features.iter().any(|f| f != "Custom" && f != "Travel"),
        "C++ G-code has no extrusion (max E={}, features={:?})",
        cpp.max_e,
        cpp.features
    );
}

pub(crate) fn parse_axis(upper: &str, axis: u8) -> Option<f64> {
    let bytes = upper.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == axis && (i == 0 || bytes[i - 1].is_ascii_whitespace()) {
            let rest = &upper[i + 1..];
            let num: String = rest
                .chars()
                .take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '-')
                .collect();
            if let Ok(v) = num.parse::<f64>() {
                return Some(v);
            }
        }
        i += 1;
    }
    None
}

fn parse_axis_g1(line: &str, axis: u8) -> Option<f64> {
    let upper = line.to_ascii_uppercase();
    if !(upper.starts_with("G1") || upper.starts_with("G0")) {
        return None;
    }
    parse_axis(&upper, axis)
}

fn parse_g1_z(line: &str) -> Option<f64> {
    parse_axis_g1(line, b'Z')
}

fn parse_time_dhms(text: &str) -> Option<f64> {
    let mut total = 0.0_f64;
    let mut found = false;
    for token in text.split_whitespace() {
        if let Some(n) = token.strip_suffix('d') {
            total += n.parse::<f64>().ok()? * 86400.0;
            found = true;
        } else if let Some(n) = token.strip_suffix('h') {
            total += n.parse::<f64>().ok()? * 3600.0;
            found = true;
        } else if let Some(n) = token.strip_suffix('m') {
            total += n.parse::<f64>().ok()? * 60.0;
            found = true;
        } else if let Some(n) = token.strip_suffix('s') {
            total += n.parse::<f64>().ok()?;
            found = true;
        }
    }
    found.then_some(total)
}
