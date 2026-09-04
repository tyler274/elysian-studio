//! G-code processor: trapezoid time estimate and filament usage.
//!
//! Ports the C++ `GCodeProcessor::TimeBlock` kinematics (Marlin-style forward /
//! reverse look-ahead with XY jerk junctions) used to fill
//! `; model printing time` / `; total filament …` placeholders.

use bambu_config::SliceSettings;

/// C++ `machine_max_jerk_e` on X1 Carbon.
const E_JERK_MM_S: f64 = 2.5;
const PREVIOUS_FEEDRATE_THRESHOLD: f64 = 1e-4;

#[derive(Debug, Clone, PartialEq)]
pub struct ProcessorResult {
    pub time_s: f64,
    pub filament_mm: f64,
    pub filament_cm3: f64,
    pub filament_g: f64,
    pub move_count: usize,
}

impl ProcessorResult {
    pub fn footer_lines(&self) -> String {
        let clock = format_time_dhms(self.time_s);
        format!(
            "; model printing time: {clock}; total estimated time: {clock}\n\
             ; total filament weight [g] : {:.2}\n\
             ; total filament volume [cm^3] : {:.2}\n\
             ; total filament length [mm] : {:.2}\n",
            self.filament_g, self.filament_cm3, self.filament_mm
        )
    }
}

/// C++ `Slic3r::get_time_dhms`.
pub fn format_time_dhms(mut time_s: f64) -> String {
    if !time_s.is_finite() || time_s < 0.0 {
        time_s = 0.0;
    }
    let days = (time_s / 86400.0) as i32;
    time_s -= f64::from(days) * 86400.0;
    let hours = (time_s / 3600.0) as i32;
    time_s -= f64::from(hours) * 3600.0;
    let minutes = (time_s / 60.0) as i32;
    time_s -= f64::from(minutes) * 60.0;
    if days > 0 {
        format!("{days}d {hours}h {minutes}m {}s", time_s as i32)
    } else if hours > 0 {
        format!("{hours}h {minutes}m {}s", time_s as i32)
    } else if minutes > 0 {
        format!("{minutes}m {}s", time_s as i32)
    } else if time_s > 1.0 {
        format!("{}s", time_s as i32)
    } else {
        format!("{time_s}s")
    }
}

pub fn process_gcode(gcode: &str, settings: &SliceSettings) -> ProcessorResult {
    let mut x = 0.0_f64;
    let mut y = 0.0_f64;
    let mut z = 0.0_f64;
    let mut e = 0.0_f64;
    let mut f_mm_min = 0.0_f64;
    let mut filament_mm = 0.0_f64;
    let mut wiping = false;
    let mut blocks = Vec::new();
    let mut prev_state: Option<PrevMove> = None;

    for line in gcode.lines() {
        if line.contains("WIPE_START") {
            wiping = true;
        } else if line.contains("WIPE_END") {
            wiping = false;
        }
        let trimmed = strip_comment(line).trim();
        if trimmed.is_empty() {
            continue;
        }
        let upper = trimmed.to_ascii_uppercase();
        if upper.starts_with("G28") {
            x = 0.0;
            y = 0.0;
            z = 0.0;
            prev_state = None;
            continue;
        }
        if upper.starts_with("G92") {
            if let Some(v) = parse_axis(&upper, b'E') {
                e = v;
            }
            if let Some(v) = parse_axis(&upper, b'X') {
                x = v;
            }
            if let Some(v) = parse_axis(&upper, b'Y') {
                y = v;
            }
            if let Some(v) = parse_axis(&upper, b'Z') {
                z = v;
            }
            continue;
        }
        let is_travel = upper.starts_with("G0");
        let is_linear = is_travel || upper.starts_with("G1");
        let is_arc = is_arc_cmd(&upper);
        if !is_linear && !is_arc {
            continue;
        }
        if let Some(v) = parse_axis(&upper, b'F') {
            f_mm_min = v.max(0.0);
        }
        let nx = parse_axis(&upper, b'X').unwrap_or(x);
        let ny = parse_axis(&upper, b'Y').unwrap_or(y);
        let nz = parse_axis(&upper, b'Z').unwrap_or(z);
        let ne = parse_axis(&upper, b'E').unwrap_or(e);
        let dx = nx - x;
        let dy = ny - y;
        let dz = nz - z;
        let de = ne - e;
        let distance = if is_arc {
            arc_move_length(&upper, dx, dy, dz)
        } else {
            (dx * dx + dy * dy + dz * dz).sqrt()
        };
        if de > 0.0 && !wiping && distance > 1e-9 {
            filament_mm += de;
        }
        x = nx;
        y = ny;
        z = nz;
        e = ne;
        if distance < 1e-9 {
            continue;
        }
        let cruise = f_mm_min / 60.0;
        if cruise <= 0.0 {
            continue;
        }
        let inv = 1.0 / distance;
        let dir = [dx * inv, dy * inv, dz * inv];
        let accel = if is_travel || is_arc {
            settings.travel_acceleration_mm_s2
        } else {
            settings.default_acceleration_mm_s2
        }
        .max(1.0);
        let block = build_block(
            distance,
            cruise,
            dir,
            [dx, dy, de],
            accel,
            settings,
            &mut prev_state,
        );
        blocks.push(block);
    }

    plan_blocks(&mut blocks);
    let time_s: f64 = blocks.iter().map(TimeBlock::time).sum();
    let filament_area = std::f64::consts::PI * (settings.filament_diameter_mm * 0.5).powi(2);
    let filament_cm3 = if filament_area > 0.0 {
        filament_mm * filament_area * 0.001
    } else {
        0.0
    };
    let filament_g = filament_cm3 * settings.filament_density_g_cm3;

    ProcessorResult {
        time_s,
        filament_mm,
        filament_cm3,
        filament_g,
        move_count: blocks.len(),
    }
}

fn strip_comment(line: &str) -> &str {
    match line.find(';') {
        Some(i) => &line[..i],
        None => line,
    }
}

fn is_arc_cmd(upper: &str) -> bool {
    matches!(upper.split_whitespace().next(), Some("G2" | "G3"))
}

/// C++ `GCodeProcessor::process_G2_G3` length. `P1` is a full XY circle.
fn arc_move_length(upper: &str, dx: f64, dy: f64, dz: f64) -> f64 {
    let i = parse_axis(upper, b'I').unwrap_or(0.0);
    let j = parse_axis(upper, b'J').unwrap_or(0.0);
    if i.abs() <= 1e-12 && j.abs() <= 1e-12 {
        return (dx * dx + dy * dy + dz * dz).sqrt();
    }
    if parse_axis(upper, b'P').is_some_and(|p| (p - 1.0).abs() < 1e-9) {
        2.0 * std::f64::consts::PI * (i * i + j * j).sqrt()
    } else {
        (dx * dx + dy * dy + dz * dz).sqrt()
    }
}

fn parse_axis(upper: &str, axis: u8) -> Option<f64> {
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

#[derive(Clone, Copy)]
struct PrevMove {
    feedrate: f64,
    dir: [f64; 3],
    axis_feedrate: [f64; 3],
    safe_feedrate: f64,
}

struct TimeBlock {
    distance: f64,
    acceleration: f64,
    cruise: f64,
    entry: f64,
    exit: f64,
    max_entry: f64,
    safe: f64,
    nominal_length: bool,
}

impl TimeBlock {
    fn calculate_trapezoid(&self) -> (f64, f64, f64) {
        let mut cruise = self.cruise;
        let mut accelerate_until =
            estimated_acceleration_distance(self.entry, cruise, self.acceleration).max(0.0);
        let decelerate_distance =
            estimated_acceleration_distance(cruise, self.exit, -self.acceleration).max(0.0);
        let mut cruise_distance = self.distance - accelerate_until - decelerate_distance;
        if cruise_distance < 0.0 {
            accelerate_until =
                intersection_distance(self.entry, self.exit, self.acceleration, self.distance)
                    .clamp(0.0, self.distance);
            cruise_distance = 0.0;
            cruise = speed_from_distance(self.entry, accelerate_until, self.acceleration);
        }
        let decelerate_after = accelerate_until + cruise_distance;
        (accelerate_until, decelerate_after, cruise)
    }

    fn time(&self) -> f64 {
        let (accelerate_until, decelerate_after, cruise) = self.calculate_trapezoid();
        acceleration_time_from_distance(self.entry, accelerate_until, self.acceleration)
            + if cruise != 0.0 {
                (decelerate_after - accelerate_until) / cruise
            } else {
                0.0
            }
            + acceleration_time_from_distance(
                cruise,
                self.distance - decelerate_after,
                -self.acceleration,
            )
    }
}

fn build_block(
    distance: f64,
    cruise: f64,
    dir: [f64; 3],
    delta_xye: [f64; 3],
    acceleration: f64,
    settings: &SliceSettings,
    prev_state: &mut Option<PrevMove>,
) -> TimeBlock {
    let inv = 1.0 / distance;
    let axis_feedrate = [
        cruise * delta_xye[0] * inv,
        cruise * delta_xye[1] * inv,
        cruise * delta_xye[2] * inv,
    ];
    let abs_axis = [
        axis_feedrate[0].abs(),
        axis_feedrate[1].abs(),
        axis_feedrate[2].abs(),
    ];
    let mut safe = cruise;
    if abs_axis[0] > settings.xy_jerk_mm_s || abs_axis[1] > settings.xy_jerk_mm_s {
        safe = safe.min(settings.xy_jerk_mm_s);
    }
    if dir[2].abs() * cruise > settings.z_jerk_mm_s {
        safe = safe.min(settings.z_jerk_mm_s);
    }
    if abs_axis[2] > E_JERK_MM_S {
        safe = safe.min(E_JERK_MM_S);
    }

    let mut vmax_junction = safe;
    if let Some(prev) = prev_state
        .as_ref()
        .copied()
        .filter(|p| p.feedrate > PREVIOUS_FEEDRATE_THRESHOLD)
    {
        let prev_speed_larger = prev.feedrate > cruise;
        let smaller = if prev_speed_larger {
            cruise / prev.feedrate
        } else {
            prev.feedrate / cruise
        };
        vmax_junction = if prev_speed_larger {
            cruise
        } else {
            prev.feedrate
        };

        let mut v_factor = 1.0;
        let mut limited = false;
        let mut exit_v = [
            prev.feedrate * prev.dir[0],
            prev.feedrate * prev.dir[1],
            prev.feedrate * prev.dir[2],
        ];
        if prev_speed_larger {
            exit_v[0] *= smaller;
            exit_v[1] *= smaller;
            exit_v[2] *= smaller;
        }
        let entry_v = [cruise * dir[0], cruise * dir[1], cruise * dir[2]];
        let mut jerk_v = [
            (entry_v[0] - exit_v[0]).abs(),
            (entry_v[1] - exit_v[1]).abs(),
            (entry_v[2] - exit_v[2]).abs(),
        ];
        let max_jerk = [
            settings.xy_jerk_mm_s,
            settings.xy_jerk_mm_s,
            settings.z_jerk_mm_s,
        ];
        for i in 0..3 {
            if jerk_v[i] > max_jerk[i] && max_jerk[i] > 0.0 {
                v_factor *= max_jerk[i] / jerk_v[i];
                jerk_v[0] *= v_factor;
                jerk_v[1] *= v_factor;
                jerk_v[2] *= v_factor;
                limited = true;
            }
        }

        let mut v_exit_e = prev.axis_feedrate[2];
        let mut v_entry_e = axis_feedrate[2];
        if prev_speed_larger {
            v_exit_e *= smaller;
        }
        if limited {
            v_exit_e *= v_factor;
            v_entry_e *= v_factor;
        }
        let e_jerk = axis_jerk(v_exit_e, v_entry_e);
        if e_jerk > E_JERK_MM_S {
            v_factor *= E_JERK_MM_S / e_jerk;
            limited = true;
        }
        if limited {
            vmax_junction *= v_factor;
        }
        let threshold = vmax_junction * 0.99;
        if prev.safe_feedrate > threshold && safe > threshold {
            vmax_junction = safe;
        }
    }

    let v_allowable = max_allowable_speed(-acceleration, safe, distance);
    let entry = vmax_junction.min(v_allowable);
    *prev_state = Some(PrevMove {
        feedrate: cruise,
        dir,
        axis_feedrate,
        safe_feedrate: safe,
    });
    TimeBlock {
        distance,
        acceleration,
        cruise,
        entry,
        exit: safe,
        max_entry: vmax_junction,
        safe,
        nominal_length: cruise <= v_allowable,
    }
}

fn axis_jerk(v_exit: f64, v_entry: f64) -> f64 {
    if v_exit > v_entry {
        if v_entry > 0.0 || v_exit < 0.0 {
            v_exit - v_entry
        } else {
            v_exit.max(-v_entry)
        }
    } else if v_entry < 0.0 || v_exit > 0.0 {
        v_entry - v_exit
    } else {
        (-v_exit).max(v_entry)
    }
}

fn plan_blocks(blocks: &mut [TimeBlock]) {
    if blocks.is_empty() {
        return;
    }
    for i in 0..blocks.len().saturating_sub(1) {
        if !blocks[i].nominal_length && blocks[i].entry < blocks[i + 1].entry {
            let entry = blocks[i + 1].entry.min(max_allowable_speed(
                -blocks[i].acceleration,
                blocks[i].entry,
                blocks[i].distance,
            ));
            blocks[i + 1].entry = entry;
        }
    }
    for i in (1..blocks.len()).rev() {
        if (blocks[i - 1].entry - blocks[i - 1].max_entry).abs() > 1e-9 {
            blocks[i - 1].entry =
                if !blocks[i - 1].nominal_length && blocks[i - 1].max_entry > blocks[i].entry {
                    blocks[i - 1].max_entry.min(max_allowable_speed(
                        -blocks[i - 1].acceleration,
                        blocks[i].entry,
                        blocks[i - 1].distance,
                    ))
                } else {
                    blocks[i - 1].max_entry
                };
        }
    }
    for i in 0..blocks.len().saturating_sub(1) {
        blocks[i].exit = blocks[i + 1].entry;
    }
    if let Some(last) = blocks.last_mut() {
        last.exit = last.safe;
    }
}

fn estimated_acceleration_distance(initial: f64, target: f64, acceleration: f64) -> f64 {
    if acceleration == 0.0 {
        0.0
    } else {
        (target * target - initial * initial) / (2.0 * acceleration)
    }
}

fn intersection_distance(initial: f64, final_rate: f64, acceleration: f64, distance: f64) -> f64 {
    if acceleration == 0.0 {
        0.0
    } else {
        (2.0 * acceleration * distance - initial * initial + final_rate * final_rate)
            / (4.0 * acceleration)
    }
}

fn speed_from_distance(initial: f64, distance: f64, acceleration: f64) -> f64 {
    (initial * initial + 2.0 * acceleration * distance)
        .max(0.0)
        .sqrt()
}

fn max_allowable_speed(acceleration: f64, target: f64, distance: f64) -> f64 {
    (target * target - 2.0 * acceleration * distance)
        .max(0.0)
        .sqrt()
}

fn acceleration_time_from_distance(initial: f64, distance: f64, acceleration: f64) -> f64 {
    if acceleration == 0.0 {
        0.0
    } else {
        (speed_from_distance(initial, distance, acceleration) - initial) / acceleration
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_dhms_matches_cpp_layout() {
        assert_eq!(format_time_dhms(3.0), "3s");
        assert_eq!(format_time_dhms(75.0), "1m 15s");
        assert_eq!(format_time_dhms(3661.0), "1h 1m 1s");
    }

    #[test]
    fn long_extrusion_is_mostly_cruise() {
        let settings = SliceSettings::default();
        let gcode = "G90\nG0 X0 Y0 F6000\nG1 X100 Y0 E2 F3000\n";
        let stats = process_gcode(gcode, &settings);
        assert_eq!(stats.move_count, 1);
        assert!((stats.filament_mm - 2.0).abs() < 1e-9);
        assert!(
            (stats.time_s - 2.0).abs() < 0.05,
            "expected ~2s cruise, got {}",
            stats.time_s
        );
        let area = std::f64::consts::PI * (1.75_f64 * 0.5).powi(2);
        assert!((stats.filament_cm3 - 2.0 * area * 0.001).abs() < 1e-9);
        assert!((stats.filament_g - stats.filament_cm3 * 1.24).abs() < 1e-9);
    }

    #[test]
    fn spiral_p1_counts_full_circle_time() {
        let settings = SliceSettings::default();
        let gcode = "G90\nG0 X0 Y0 F6000\nG17\nG2 Z0.400 I1.000 J0.000 P1 F600\n";
        let stats = process_gcode(gcode, &settings);
        let expect = 2.0 * std::f64::consts::PI / 10.0;
        assert_eq!(stats.move_count, 1);
        assert!(
            (stats.time_s - expect).abs() < 0.05,
            "expected ~{expect}s for 2π mm at 10 mm/s, got {}",
            stats.time_s
        );
    }

    #[test]
    fn footer_uses_bambu_placeholders() {
        let stats = ProcessorResult {
            time_s: 75.0,
            filament_mm: 100.0,
            filament_cm3: 0.24,
            filament_g: 0.30,
            move_count: 1,
        };
        let footer = stats.footer_lines();
        assert!(footer.contains("; model printing time: 1m 15s; total estimated time: 1m 15s"));
        assert!(footer.contains("; total filament weight [g] : 0.30"));
        assert!(footer.contains("; total filament volume [cm^3] : 0.24"));
        assert!(footer.contains("; total filament length [mm] : 100.00"));
    }
}
