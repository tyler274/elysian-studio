//! Part cooling fan (`M106`) from C++ `GCodeEditor::write_layer_gcode`.
//!
//! Layer-time interpolation is applied; overhang/ironing fan overrides are not.

use bambu_config::SliceSettings;

use crate::processor::process_gcode;

/// C++ `change_extruder_set_fan` percent for one layer.
pub fn part_fan_percent(layer_id: usize, layer_time_s: f64, settings: &SliceSettings) -> u32 {
    let mut close = settings.close_fan_the_first_x_layers;
    let full = settings.full_fan_speed_layer;
    if close == 0 && full > 0 {
        close = 1;
    }
    let layer_id = u32::try_from(layer_id).unwrap_or(u32::MAX);
    if layer_id < close {
        return settings.first_x_layer_part_fan_speed.min(100);
    }
    let min = settings.fan_min_speed.min(100);
    let max = settings.fan_max_speed.min(100);
    let mut fan = if settings.reduce_fan_stop_start_freq {
        min
    } else {
        0
    };
    let slow = settings.slow_down_layer_time_s;
    let cool = settings.fan_cooling_layer_time_s;
    if layer_time_s < slow {
        fan = max;
    } else if layer_time_s < cool && cool > slow {
        let t = (layer_time_s - slow) / (cool - slow);
        fan = (t * f64::from(min) + (1.0 - t) * f64::from(max)).floor() as u32;
    }
    if layer_id + 1 < full && full > close {
        let factor = f64::from(layer_id + 1 - close) / f64::from(full - close);
        let first = f64::from(settings.first_x_layer_part_fan_speed);
        fan = (first * (1.0 - factor) + f64::from(fan) * factor + 0.5)
            .floor()
            .clamp(0.0, 100.0) as u32;
    }
    fan.min(100)
}

/// C++ `GCodeWriter::set_fan` for Marlin / Bambu (`M106 S` PWM).
pub fn set_fan_gcode(percent: u32) -> String {
    if percent == 0 {
        return "M106 S0\n".into();
    }
    let pwm = 255.0 * f64::from(percent.min(100)) / 100.0;
    if (pwm - pwm.round()).abs() < 1e-9 {
        format!("M106 S{:.0}\n", pwm.round())
    } else {
        format!("M106 S{pwm}\n")
    }
}

/// Insert `M106` after each layer's first `G1 Z` when the fan percent changes.
pub fn apply_part_cooling(gcode: &str, settings: &SliceSettings) -> String {
    let mut out = String::with_capacity(gcode.len() + 64);
    let mut layer = String::new();
    let mut in_layer = false;
    let mut layer_id = 0usize;
    let mut last_fan: Option<u32> = None;
    for line in gcode.lines() {
        if line.trim_start().starts_with("; CHANGE_LAYER") || line.trim() == ";CHANGE_LAYER" {
            if in_layer {
                flush_layer(&mut out, &layer, layer_id, settings, &mut last_fan);
                layer_id += 1;
                layer.clear();
            }
            in_layer = true;
            layer.push_str(line);
            layer.push('\n');
            continue;
        }
        if in_layer {
            layer.push_str(line);
            layer.push('\n');
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    if in_layer {
        flush_layer(&mut out, &layer, layer_id, settings, &mut last_fan);
    }
    out
}

fn flush_layer(
    out: &mut String,
    layer: &str,
    layer_id: usize,
    settings: &SliceSettings,
    last_fan: &mut Option<u32>,
) {
    let time_s = process_gcode(layer, settings).time_s;
    let fan = part_fan_percent(layer_id, time_s, settings);
    if *last_fan == Some(fan) {
        out.push_str(layer);
        return;
    }
    *last_fan = Some(fan);
    out.push_str(&insert_fan_after_z(layer, &set_fan_gcode(fan)));
}

fn insert_fan_after_z(layer: &str, fan_line: &str) -> String {
    let mut out = String::with_capacity(layer.len() + fan_line.len());
    let mut inserted = false;
    for line in layer.lines() {
        out.push_str(line);
        out.push('\n');
        if !inserted && is_layer_z(line) {
            out.push_str(fan_line);
            inserted = true;
        }
    }
    if !inserted {
        out.push_str(fan_line);
    }
    out
}

fn is_layer_z(line: &str) -> bool {
    let t = line.trim();
    let upper = t.to_ascii_uppercase();
    (upper.starts_with("G0") || upper.starts_with("G1")) && upper.contains('Z')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_layers_use_closed_fan() {
        let mut s = SliceSettings::default();
        s.close_fan_the_first_x_layers = 2;
        s.first_x_layer_part_fan_speed = 0;
        s.fan_min_speed = 100;
        s.fan_max_speed = 100;
        s.reduce_fan_stop_start_freq = true;
        assert_eq!(part_fan_percent(0, 1.0, &s), 0);
        assert_eq!(part_fan_percent(1, 1.0, &s), 0);
        assert_eq!(part_fan_percent(2, 1.0, &s), 100);
    }

    #[test]
    fn pla_full_fan_after_first_layer() {
        let s = SliceSettings::bbl_0_20();
        assert_eq!(part_fan_percent(0, 30.0, &s), 0);
        assert_eq!(part_fan_percent(1, 30.0, &s), 100);
        assert_eq!(part_fan_percent(50, 200.0, &s), 100);
    }

    #[test]
    fn interpolates_between_min_and_max() {
        let mut s = SliceSettings::default();
        s.close_fan_the_first_x_layers = 0;
        s.fan_min_speed = 20;
        s.fan_max_speed = 100;
        s.reduce_fan_stop_start_freq = true;
        s.slow_down_layer_time_s = 8.0;
        s.fan_cooling_layer_time_s = 60.0;
        assert_eq!(part_fan_percent(1, 4.0, &s), 100);
        assert_eq!(part_fan_percent(1, 80.0, &s), 20);
        let mid = part_fan_percent(1, 34.0, &s);
        // t = (34-8)/(60-8) = 0.5 → 0.5*20 + 0.5*100 = 60
        assert_eq!(mid, 60);
    }

    #[test]
    fn set_fan_pwm_matches_percent() {
        assert_eq!(set_fan_gcode(0), "M106 S0\n");
        assert_eq!(set_fan_gcode(100), "M106 S255\n");
        assert_eq!(set_fan_gcode(50), "M106 S127.5\n");
    }
}
