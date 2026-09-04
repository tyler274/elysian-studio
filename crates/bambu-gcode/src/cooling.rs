//! Part cooling fan (`M106`) and layer-time slowdown from C++ `GCodeEditor`.
//!
//! Overhang/ironing fan overrides are not applied.

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

/// Stretch extrusion `F` when a layer is shorter than `slow_down_layer_time`.
///
/// C++ `CoolingBuffer` uses cruise time (`length / feedrate`). Travel and
/// Z-only moves stay at their original feeds. External-perimeter exceptions
/// (`no_slow_down_for_cooling_on_outwalls`) are not applied.
pub fn apply_layer_cooling_slowdown(gcode: &str, settings: &SliceSettings) -> String {
    if !settings.slow_down_for_layer_cooling || settings.slow_down_layer_time_s <= 0.0 {
        return gcode.to_string();
    }
    let mut out = String::with_capacity(gcode.len());
    let mut layer = String::new();
    let mut in_layer = false;
    let mut head = Head::default();
    for line in gcode.lines() {
        if line.trim_start().starts_with("; CHANGE_LAYER") || line.trim() == ";CHANGE_LAYER" {
            if in_layer {
                out.push_str(&slowdown_layer(&layer, settings, &mut head));
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
        out.push_str(&slowdown_layer(&layer, settings, &mut head));
    }
    out
}

#[derive(Default)]
struct Head {
    x: f64,
    y: f64,
    z: f64,
    f_mm_min: f64,
}

fn slowdown_layer(layer: &str, settings: &SliceSettings, head: &mut Head) -> String {
    let target = settings.slow_down_layer_time_s * 1.001;
    let min_feed = settings.slow_down_min_speed_mm_s;
    let lines: Vec<&str> = layer.lines().collect();
    let mut x = head.x;
    let mut y = head.y;
    let mut z = head.z;
    let mut f_mm_min = head.f_mm_min;
    let mut adjustable = Vec::new();
    let mut non_adj_time = 0.0_f64;
    for (idx, line) in lines.iter().enumerate() {
        let upper = strip_comment(line).to_ascii_uppercase();
        let is_g0 = upper.starts_with("G0");
        let is_g1 = upper.starts_with("G1");
        if !is_g0 && !is_g1 {
            continue;
        }
        if let Some(v) = parse_axis(&upper, b'F') {
            f_mm_min = v.max(0.0);
        }
        let nx = parse_axis(&upper, b'X').unwrap_or(x);
        let ny = parse_axis(&upper, b'Y').unwrap_or(y);
        let nz = parse_axis(&upper, b'Z').unwrap_or(z);
        let length = ((nx - x).powi(2) + (ny - y).powi(2) + (nz - z).powi(2)).sqrt();
        let feed_mm_s = f_mm_min / 60.0;
        let has_e = parse_axis(&upper, b'E').is_some();
        let is_adj = is_g1 && has_e && length > 1e-9 && feed_mm_s > 1e-9;
        if is_adj {
            adjustable.push(AdjMove {
                idx,
                length,
                feed_mm_s,
            });
        } else {
            non_adj_time += cruise_time(length, feed_mm_s);
        }
        x = nx;
        y = ny;
        z = nz;
    }
    head.x = x;
    head.y = y;
    head.z = z;
    head.f_mm_min = f_mm_min;
    let mut feeds: Vec<f64> = adjustable.iter().map(|m| m.feed_mm_s).collect();
    let adj_time: f64 = adjustable
        .iter()
        .zip(feeds.iter())
        .map(|(m, f)| cruise_time(m.length, *f))
        .sum();
    if non_adj_time + adj_time >= target {
        return layer.to_string();
    }
    for _ in 0..5 {
        let mut locked_time = non_adj_time;
        let mut stretch_time = 0.0;
        for (m, feed) in adjustable.iter().zip(feeds.iter()) {
            let time = cruise_time(m.length, *feed);
            if min_feed > 0.0 && *feed <= min_feed + 1e-9 {
                locked_time += time;
            } else {
                stretch_time += time;
            }
        }
        if locked_time + stretch_time >= 0.95 * target {
            break;
        }
        if stretch_time <= 1e-9 {
            break;
        }
        let factor = ((target - locked_time) / stretch_time).max(1.0);
        for feed in &mut feeds {
            if min_feed > 0.0 && *feed <= min_feed + 1e-9 {
                continue;
            }
            *feed /= factor;
            if min_feed > 0.0 {
                *feed = feed.max(min_feed);
            }
        }
    }
    let mut out = String::with_capacity(layer.len());
    let mut adj_i = 0;
    for (idx, line) in lines.iter().enumerate() {
        if adj_i < adjustable.len() && adjustable[adj_i].idx == idx {
            out.push_str(&replace_feed(line, feeds[adj_i] * 60.0));
            adj_i += 1;
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    out
}

struct AdjMove {
    idx: usize,
    length: f64,
    feed_mm_s: f64,
}

fn cruise_time(length: f64, feed_mm_s: f64) -> f64 {
    if length <= 1e-12 || feed_mm_s <= 1e-12 {
        0.0
    } else {
        length / feed_mm_s
    }
}

fn strip_comment(line: &str) -> &str {
    match line.find(';') {
        Some(i) => &line[..i],
        None => line,
    }
}

fn parse_axis(upper: &str, axis: u8) -> Option<f64> {
    let bytes = upper.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == axis && (i == 0 || bytes[i - 1].is_ascii_whitespace()) {
            i += 1;
            let start = i;
            while i < bytes.len()
                && (bytes[i] == b'+'
                    || bytes[i] == b'-'
                    || bytes[i] == b'.'
                    || bytes[i].is_ascii_digit())
            {
                i += 1;
            }
            return upper[start..i].parse().ok();
        }
        i += 1;
    }
    None
}

fn replace_feed(line: &str, f_mm_min: f64) -> String {
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b';' {
            break;
        }
        let c = bytes[i].to_ascii_uppercase();
        if c == b'F' && (i == 0 || bytes[i - 1].is_ascii_whitespace()) {
            let start = i;
            i += 1;
            while i < bytes.len()
                && (bytes[i] == b'+'
                    || bytes[i] == b'-'
                    || bytes[i] == b'.'
                    || bytes[i].is_ascii_digit())
            {
                i += 1;
            }
            return format!("{}F{:.0}{}", &line[..start], f_mm_min.round(), &line[i..]);
        }
        i += 1;
    }
    format!("{} F{:.0}", line.trim_end(), f_mm_min.round())
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

    #[test]
    fn slows_short_layer_toward_target_time() {
        let mut s = SliceSettings::default();
        s.slow_down_for_layer_cooling = true;
        s.slow_down_layer_time_s = 8.0;
        s.slow_down_min_speed_mm_s = 20.0;
        let gcode =
            "; CHANGE_LAYER\n;LAYER:1\nG1 Z0.400 F600\nG0 X0 Y0 F24000\nG1 X400 Y0 E10 F12000\n";
        let out = apply_layer_cooling_slowdown(gcode, &s);
        assert!(!out.contains(" F12000"), "{out}");
        let feed = extrusion_feed_mm_min(&out);
        // 400 mm in ~8 s → ~50 mm/s
        assert!(
            (45.0..55.0).contains(&(feed / 60.0)),
            "got {} mm/s\n{out}",
            feed / 60.0
        );
    }

    #[test]
    fn cooling_slowdown_respects_min_speed() {
        let mut s = SliceSettings::default();
        s.slow_down_for_layer_cooling = true;
        s.slow_down_layer_time_s = 8.0;
        s.slow_down_min_speed_mm_s = 20.0;
        let gcode = "; CHANGE_LAYER\n;LAYER:1\nG1 X10 Y0 E1 F6000\n";
        let out = apply_layer_cooling_slowdown(gcode, &s);
        assert!(out.contains(" F1200"), "{out}");
    }

    #[test]
    fn cooling_slowdown_skips_when_disabled() {
        let mut s = SliceSettings::default();
        s.slow_down_for_layer_cooling = false;
        s.slow_down_layer_time_s = 8.0;
        let gcode = "; CHANGE_LAYER\n;LAYER:1\nG1 X400 Y0 E10 F12000\n";
        let out = apply_layer_cooling_slowdown(gcode, &s);
        assert!(out.contains(" F12000"), "{out}");
    }

    fn extrusion_feed_mm_min(gcode: &str) -> f64 {
        for line in gcode.lines() {
            let upper = strip_comment(line).to_ascii_uppercase();
            if upper.starts_with("G1") && parse_axis(&upper, b'E').is_some() {
                if let Some(f) = parse_axis(&upper, b'F') {
                    return f;
                }
            }
        }
        0.0
    }
}
