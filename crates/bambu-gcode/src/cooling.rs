//! Part cooling fan (`M106`), auxiliary fan (`M106 P2`), and layer-time slowdown
//! from C++ `GCodeEditor`.
//!
//! Overhang/ironing markers become PWM. `pre_start_fan_time` spins the overhang
//! fan up early. Exhaust (`M106 P3`) is emitted from the G-code writer.

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

/// C++ `change_extruder_set_fan` additional/aux fan percent for one layer.
pub fn additional_fan_percent(layer_id: usize, settings: &SliceSettings) -> u32 {
    let close = settings.close_additional_fan_first_x_layers;
    let full = settings.additional_fan_full_speed_layer;
    let first = settings.first_x_layer_fan_speed.min(100);
    let target = settings.additional_cooling_fan_speed.min(100);
    let layer_id = u32::try_from(layer_id).unwrap_or(u32::MAX);
    if layer_id < close {
        return first;
    }
    if layer_id + 1 < full && full > close {
        let factor = f64::from(layer_id + 1 - close) / f64::from(full - close);
        return (f64::from(first) * (1.0 - factor) + f64::from(target) * factor + 0.5)
            .floor()
            .clamp(0.0, 100.0) as u32;
    }
    target
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

/// C++ `GCodeWriter::set_additional_fan` (`M106 P2` PWM, truncated).
pub fn set_additional_fan_gcode(percent: u32) -> String {
    let pwm = (255.0 * f64::from(percent.min(100)) / 100.0) as i32;
    format!("M106 P2 S{pwm}\n")
}

/// C++ `GCodeWriter::set_exhaust_fan` (`M106 P3` PWM, truncated).
pub fn set_exhaust_fan_gcode(percent: u32) -> String {
    let pwm = (f64::from(percent.min(100)) / 100.0 * 255.0) as i32;
    format!("M106 P3 S{pwm}\n")
}

/// Stretch extrusion `F` when a layer is shorter than `slow_down_layer_time`.
///
/// C++ `CoolingBuffer` uses cruise time (`length / feedrate`). Travel and
/// Z-only moves stay at their original feeds. Outer walls marked
/// `_EXTERNAL_PERIMETER` skip stretch when `no_slow_down_for_cooling_on_outwalls`.
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
        let is_arc = is_arc_move(&upper);
        if !is_g0 && !is_g1 && !is_arc {
            continue;
        }
        if let Some(v) = parse_axis(&upper, b'F') {
            f_mm_min = v.max(0.0);
        }
        let nx = parse_axis(&upper, b'X').unwrap_or(x);
        let ny = parse_axis(&upper, b'Y').unwrap_or(y);
        let nz = parse_axis(&upper, b'Z').unwrap_or(z);
        let length = if is_arc {
            cooling_arc_length(&upper, x, y, z, nx, ny, nz)
        } else {
            ((nx - x).powi(2) + (ny - y).powi(2) + (nz - z).powi(2)).sqrt()
        };
        let feed_mm_s = f_mm_min / 60.0;
        let has_e = parse_axis(&upper, b'E').is_some();
        let is_adj = is_g1
            && has_e
            && length > 1e-9
            && feed_mm_s > 1e-9
            && !line.contains("_WIPE")
            && !(settings.no_slow_down_for_cooling_on_outwalls
                && line.contains("_EXTERNAL_PERIMETER"));
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

fn is_arc_move(upper: &str) -> bool {
    matches!(upper.split_whitespace().next(), Some("G2" | "G3"))
}

fn cooling_arc_length(upper: &str, x: f64, y: f64, z: f64, nx: f64, ny: f64, nz: f64) -> f64 {
    let i = parse_axis(upper, b'I').unwrap_or(0.0);
    let j = parse_axis(upper, b'J').unwrap_or(0.0);
    if parse_axis(upper, b'P').is_some_and(|p| (p - 1.0).abs() < 1e-9) {
        2.0 * std::f64::consts::PI * (i * i + j * j).sqrt()
    } else {
        ((nx - x).powi(2) + (ny - y).powi(2) + (nz - z).powi(2)).sqrt()
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

/// Insert `M106` / `M106 P2` after each layer's first `G1 Z` when the speed changes.
pub fn apply_part_cooling(gcode: &str, settings: &SliceSettings) -> String {
    let mut out = String::with_capacity(gcode.len() + 64);
    let mut layer = String::new();
    let mut in_layer = false;
    let mut layer_id = 0usize;
    let mut last_fan: Option<u32> = None;
    let mut last_aux: Option<u32> = None;
    for line in gcode.lines() {
        if line.trim_start().starts_with("; CHANGE_LAYER") || line.trim() == ";CHANGE_LAYER" {
            if in_layer {
                flush_layer(
                    &mut out,
                    &layer,
                    layer_id,
                    settings,
                    &mut last_fan,
                    &mut last_aux,
                );
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
        flush_layer(
            &mut out,
            &layer,
            layer_id,
            settings,
            &mut last_fan,
            &mut last_aux,
        );
    }
    out
}

fn flush_layer(
    out: &mut String,
    layer: &str,
    layer_id: usize,
    settings: &SliceSettings,
    last_fan: &mut Option<u32>,
    last_aux: &mut Option<u32>,
) {
    let time_s = process_gcode(layer, settings).time_s;
    let fan = part_fan_percent(layer_id, time_s, settings);
    let mut inject = String::new();
    if *last_fan != Some(fan) {
        *last_fan = Some(fan);
        inject.push_str(&set_fan_gcode(fan));
    }
    if settings.auxiliary_fan {
        let aux = additional_fan_percent(layer_id, settings);
        if *last_aux != Some(aux) {
            *last_aux = Some(aux);
            inject.push_str(&set_additional_fan_gcode(aux));
        }
    }
    let mut text = if inject.is_empty() {
        layer.to_string()
    } else {
        insert_fan_after_z(layer, &inject)
    };
    text = rewrite_feature_fans(&text, fan, layer_id, settings);
    out.push_str(&text);
}

/// C++ `GCodeEditor` consumes `;_OVERHANG_FAN_*` / `;_IRONING_FAN_*` markers.
fn rewrite_feature_fans(
    layer: &str,
    layer_fan: u32,
    layer_id: usize,
    settings: &SliceSettings,
) -> String {
    let allow = layer_allows_feature_fan(layer_id, settings);
    let overhang = settings.overhang_fan_speed.min(100);
    let overhang_ctrl = allow && settings.enable_overhang_bridge_fan && overhang > layer_fan;
    let pre_start = if overhang_ctrl {
        settings.pre_start_fan_time_s
    } else {
        0.0
    };
    let ironing_ctrl = allow && settings.ironing_fan_speed >= 0;
    let ironing = u32::try_from(settings.ironing_fan_speed)
        .unwrap_or(0)
        .min(100);
    let lines: Vec<&str> = layer.lines().collect();
    let times = line_cruise_times(&lines);
    let mut current = layer_fan;
    let mut out = String::with_capacity(layer.len());
    let mut cumulative = 0.0_f64;
    let mut search_time = 0.0_f64;
    let mut j = 0usize;
    for (i, line) in lines.iter().enumerate() {
        if pre_start > 0.0 && overhang > layer_fan {
            cumulative += times[i];
            if j < i {
                j = i;
            }
            if search_time < cumulative {
                search_time = cumulative;
            }
            while j < lines.len()
                && search_time - cumulative < pre_start
                && overhang_ctrl
                && current < overhang
            {
                let look = lines[j].trim();
                if look.starts_with(";_FORCE_RESUME_FAN") {
                    break;
                }
                search_time += times[j];
                if look.starts_with(";_OVERHANG_FAN_START") {
                    out.push_str(&set_fan_gcode(overhang));
                    current = overhang;
                    break;
                }
                j += 1;
            }
        }
        let t = line.trim();
        if t.starts_with(";_OVERHANG_FAN_START") {
            if overhang_ctrl && current < overhang {
                out.push_str(&set_fan_gcode(overhang));
                current = overhang;
            }
            continue;
        }
        if t.starts_with(";_OVERHANG_FAN_END") {
            if overhang_ctrl && current != layer_fan {
                out.push_str(&set_fan_gcode(layer_fan));
                current = layer_fan;
            }
            continue;
        }
        if t.starts_with(";_IRONING_FAN_START") {
            if ironing_ctrl && current != ironing {
                out.push_str(&set_fan_gcode(ironing));
                current = ironing;
            }
            continue;
        }
        if t.starts_with(";_IRONING_FAN_END") {
            if ironing_ctrl && current != layer_fan {
                out.push_str(&set_fan_gcode(layer_fan));
                current = layer_fan;
            }
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

fn line_cruise_times(lines: &[&str]) -> Vec<f64> {
    let mut x = 0.0;
    let mut y = 0.0;
    let mut z = 0.0;
    let mut f_mm_min = 0.0;
    lines
        .iter()
        .map(|line| {
            let upper = strip_comment(line).to_ascii_uppercase();
            let is_arc = is_arc_move(&upper);
            let is_move = upper.starts_with("G0") || upper.starts_with("G1") || is_arc;
            if !is_move {
                return 0.0;
            }
            if let Some(v) = parse_axis(&upper, b'F') {
                f_mm_min = v.max(0.0);
            }
            let nx = parse_axis(&upper, b'X').unwrap_or(x);
            let ny = parse_axis(&upper, b'Y').unwrap_or(y);
            let nz = parse_axis(&upper, b'Z').unwrap_or(z);
            let length = if is_arc {
                cooling_arc_length(&upper, x, y, z, nx, ny, nz)
            } else {
                ((nx - x).powi(2) + (ny - y).powi(2) + (nz - z).powi(2)).sqrt()
            };
            x = nx;
            y = ny;
            z = nz;
            cruise_time(length, f_mm_min / 60.0)
        })
        .collect()
}

fn layer_allows_feature_fan(layer_id: usize, settings: &SliceSettings) -> bool {
    let mut close = settings.close_fan_the_first_x_layers;
    if close == 0 && settings.full_fan_speed_layer > 0 {
        close = 1;
    }
    u32::try_from(layer_id).unwrap_or(u32::MAX) >= close
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

    #[test]
    fn cooling_skips_outer_walls_when_flag_set() {
        let mut s = SliceSettings::default();
        s.slow_down_for_layer_cooling = true;
        s.slow_down_layer_time_s = 8.0;
        s.slow_down_min_speed_mm_s = 20.0;
        s.no_slow_down_for_cooling_on_outwalls = true;
        let gcode = "; CHANGE_LAYER\n;LAYER:1\nG1 X400 Y0 E10 F12000;_EXTRUDE_SET_SPEED;_EXTERNAL_PERIMETER\nG1 X400 Y400 E20 F18000\n";
        let out = apply_layer_cooling_slowdown(gcode, &s);
        assert!(
            out.contains(" F12000"),
            "outer wall feed should stay\n{out}"
        );
        assert!(!out.contains(" F18000"), "inner wall should stretch\n{out}");
    }

    #[test]
    fn overhang_fan_markers_become_pwm() {
        let mut s = SliceSettings::default();
        s.close_fan_the_first_x_layers = 0;
        s.fan_min_speed = 20;
        s.fan_max_speed = 20;
        s.reduce_fan_stop_start_freq = true;
        s.enable_overhang_bridge_fan = true;
        s.overhang_fan_speed = 100;
        s.slow_down_layer_time_s = 0.0;
        let gcode = "; CHANGE_LAYER\n;LAYER:1\nG1 Z0.400 F600\n;_OVERHANG_FAN_START\nG1 X1 Y0 E1 F3000\n;_OVERHANG_FAN_END\n";
        let out = apply_part_cooling(gcode, &s);
        assert!(!out.contains(";_OVERHANG_FAN"), "{out}");
        assert!(out.contains("M106 S51\n"), "layer fan 20%\n{out}");
        assert!(out.contains("M106 S255\n"), "overhang fan 100%\n{out}");
        let layer = out.find("M106 S51\n").expect("layer fan");
        let boost = out.find("M106 S255\n").expect("boost");
        let restore = out.rfind("M106 S51\n").expect("restore");
        assert!(layer < boost && boost < restore, "{out}");
    }

    #[test]
    fn ironing_fan_markers_become_pwm() {
        let mut s = SliceSettings::default();
        s.close_fan_the_first_x_layers = 0;
        s.fan_min_speed = 100;
        s.fan_max_speed = 100;
        s.reduce_fan_stop_start_freq = true;
        s.ironing_fan_speed = 40;
        s.slow_down_layer_time_s = 0.0;
        let gcode = "; CHANGE_LAYER\n;LAYER:1\nG1 Z0.400 F600\n;_IRONING_FAN_START\nG1 X1 Y0 E1 F1800\n;_IRONING_FAN_END\n";
        let out = apply_part_cooling(gcode, &s);
        assert!(!out.contains(";_IRONING_FAN"), "{out}");
        assert!(out.contains("M106 S102\n"), "ironing 40%\n{out}");
        assert!(out.contains("M106 S255\n"), "layer 100%\n{out}");
    }

    #[test]
    fn additional_fan_closed_then_full() {
        let s = SliceSettings::bbl_0_20();
        assert_eq!(additional_fan_percent(0, &s), 0);
        assert_eq!(additional_fan_percent(1, &s), 75);
        assert_eq!(additional_fan_percent(20, &s), 75);
    }

    #[test]
    fn additional_fan_ramps_to_full() {
        let mut s = SliceSettings::default();
        s.close_additional_fan_first_x_layers = 1;
        s.additional_fan_full_speed_layer = 5;
        s.first_x_layer_fan_speed = 0;
        s.additional_cooling_fan_speed = 100;
        assert_eq!(additional_fan_percent(0, &s), 0);
        assert_eq!(additional_fan_percent(1, &s), 25);
        assert_eq!(additional_fan_percent(4, &s), 100);
    }

    #[test]
    fn additional_fan_pwm_truncates() {
        assert_eq!(set_additional_fan_gcode(0), "M106 P2 S0\n");
        assert_eq!(set_additional_fan_gcode(100), "M106 P2 S255\n");
        assert_eq!(set_additional_fan_gcode(75), "M106 P2 S191\n");
        assert_eq!(set_exhaust_fan_gcode(60), "M106 P3 S153\n");
        assert_eq!(set_exhaust_fan_gcode(70), "M106 P3 S178\n");
    }

    #[test]
    fn auxiliary_fan_emits_p2_after_layer_z() {
        let mut s = SliceSettings::default();
        s.auxiliary_fan = true;
        s.additional_cooling_fan_speed = 75;
        s.close_additional_fan_first_x_layers = 1;
        s.first_x_layer_fan_speed = 0;
        s.close_fan_the_first_x_layers = 1;
        s.fan_min_speed = 100;
        s.fan_max_speed = 100;
        s.reduce_fan_stop_start_freq = true;
        s.slow_down_layer_time_s = 0.0;
        let gcode = "; CHANGE_LAYER\n;LAYER:0\nG1 Z0.200 F600\nG1 X1 Y0 E1 F3000\n\
                     ; CHANGE_LAYER\n;LAYER:1\nG1 Z0.400 F600\nG1 X2 Y0 E1 F3000\n";
        let out = apply_part_cooling(gcode, &s);
        assert!(
            out.contains("M106 P2 S0\n"),
            "closed aux on first layers\n{out}"
        );
        assert!(
            out.contains("M106 P2 S191\n"),
            "75% aux after first layers\n{out}"
        );
        let first = out.find("M106 P2 S0\n").expect("first aux");
        let later = out.find("M106 P2 S191\n").expect("later aux");
        assert!(first < later, "{out}");
        let z0 = out.find("G1 Z0.200").expect("z0");
        assert!(z0 < first && first < out.find("G1 X1").expect("x1"));
    }

    #[test]
    fn no_p2_without_auxiliary_fan() {
        let mut s = SliceSettings::default();
        s.additional_cooling_fan_speed = 75;
        s.slow_down_layer_time_s = 0.0;
        let gcode = "; CHANGE_LAYER\n;LAYER:0\nG1 Z0.200 F600\nG1 X1 Y0 E1 F3000\n";
        let out = apply_part_cooling(gcode, &s);
        assert!(!out.contains("M106 P2"), "{out}");
    }

    #[test]
    fn pre_start_overhang_fan_spins_up_early() {
        let mut s = SliceSettings::default();
        s.close_fan_the_first_x_layers = 0;
        s.fan_min_speed = 20;
        s.fan_max_speed = 20;
        s.reduce_fan_stop_start_freq = true;
        s.enable_overhang_bridge_fan = true;
        s.overhang_fan_speed = 100;
        s.pre_start_fan_time_s = 2.0;
        s.slow_down_layer_time_s = 0.0;
        // 75 mm at 50 mm/s = 1.5 s of travel before the overhang marker.
        let gcode = "; CHANGE_LAYER\n;LAYER:1\nG1 Z0.400 F600\nG1 X0 Y0 E1 F3000\n\
                     G1 X75 Y0 E1 F3000\n;_OVERHANG_FAN_START\nG1 X76 Y0 E0.1 F3000\n;_OVERHANG_FAN_END\n";
        let out = apply_part_cooling(gcode, &s);
        assert!(!out.contains(";_OVERHANG_FAN"), "{out}");
        let boost = out.find("M106 S255\n").expect("overhang fan");
        let travel = out.find("G1 X75 Y0").expect("pre-overhang travel");
        assert!(
            boost < travel,
            "fan should start before the 1.5s travel\n{out}"
        );
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
