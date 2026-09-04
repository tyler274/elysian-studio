//! C++ `GCodeWriter` motion: retract, wipe, lazy Z-hop, travel, one extrusion path.

use std::fmt::Write as _;

use bambu_config::{PrintAccel, SliceSettings, ZHopType};
use bambu_geom::{intersect_polygons, unscale, Point, Polygon};
use bambu_slicer::Layer;

use crate::cooling::{apply_layer_cooling_slowdown, apply_part_cooling};
use crate::processor::process_gcode;
use crate::GcodeError;

pub(crate) const TRAVEL_EPS_MM: f64 = 1e-4;
/// C++ `GCodeWriter::slope_threshold` (3°).
const SLOPE_THRESHOLD_RAD: f64 = 3.0 * std::f64::consts::PI / 180.0;
/// C++ `protect_z` window used by `is_through_overhang`.
const LIFT_PROTECT_Z_MM: f64 = 0.4;
/// Half-width of the travel stroke used to emulate C++ `intersection_pl`.
const TRAVEL_HIT_HALF_WIDTH_MM: f64 = 0.02;

/// Accumulates G-code and the C++ writer kinematics for one export.
pub(crate) struct Writer<'a> {
    pub(crate) settings: &'a SliceSettings,
    pub(crate) out: String,
    pub(crate) state: WriterState,
    pub(crate) travel_f: f64,
}

#[derive(Debug, Default)]
pub(crate) struct WriterState {
    pub(crate) e: f64,
    pub(crate) retracted: f64,
    pub(crate) last: Option<(f64, f64)>,
    pub(crate) wipe: Vec<(f64, f64)>,
    pub(crate) last_print_f: f64,
    pub(crate) z: f64,
    pub(crate) lifted: f64,
    /// C++ `m_to_lift`: hop height queued by `lazy_lift` until the next XY travel.
    pub(crate) to_lift: f64,
    pub(crate) to_lift_type: ZHopType,
    /// C++ `loverhangs` in the 0.4 mm Z window around the current layer.
    pub(crate) lift_overhangs: Vec<Polygon>,
    pub(crate) last_accel: f64,
    pub(crate) print_accel: f64,
    pub(crate) first_layer: bool,
    /// Upcoming extrusion is an outer/overhang wall (C++ short-travel accel).
    pub(crate) short_travel_role: bool,
    /// Last `; LINE_WIDTH:` value (C++ `m_last_width`).
    pub(crate) last_line_width: Option<f64>,
}

impl<'a> Writer<'a> {
    pub(crate) fn new(settings: &'a SliceSettings) -> Self {
        Self {
            settings,
            out: String::new(),
            state: WriterState {
                print_accel: settings.print_acceleration_mm_s2(true, PrintAccel::Default),
                ..WriterState::default()
            },
            travel_f: settings.travel_speed_mm_s * 60.0,
        }
    }

    pub(crate) fn finish(mut self, layer_count: usize) -> String {
        self.out = apply_layer_cooling_slowdown(&self.out, self.settings);
        self.out = apply_part_cooling(&self.out, self.settings);
        let stats = process_gcode(&self.out, self.settings);
        stats.fill_placeholders(&mut self.out, layer_count);
        self.out.push_str(&stats.footer_lines());
        self.out
    }

    pub(crate) fn set_print_role(&mut self, kind: PrintAccel) {
        self.state.print_accel = self
            .settings
            .print_acceleration_mm_s2(self.state.first_layer, kind);
        self.state.short_travel_role = kind == PrintAccel::OuterWall;
    }

    pub(crate) fn emit_accel(&mut self, accel: f64) -> Result<(), GcodeError> {
        if accel <= 0.0 {
            return Ok(());
        }
        let rounded = accel.round();
        if (rounded - self.state.last_accel).abs() < 0.5 {
            return Ok(());
        }
        self.state.last_accel = rounded;
        // C++ `GCodeWriter::set_acceleration_impl` for `gcfMarlinLegacy` / Klipper:
        // `M204 S` with `full_gcode_comment == false`. Envelope still uses `M204 P/R/T`.
        writeln!(self.out, "M204 S{:.0}", rounded)?;
        Ok(())
    }

    pub(crate) fn emit_marked(
        &mut self,
        mark: bool,
        start: &str,
        end: &str,
        body: impl FnOnce(&mut Self) -> Result<(), GcodeError>,
    ) -> Result<(), GcodeError> {
        if mark {
            writeln!(self.out, "{start}")?;
        }
        body(self)?;
        if mark {
            writeln!(self.out, "{end}")?;
        }
        Ok(())
    }

    fn emit_retract_e(&mut self, amount: f64) -> Result<(), GcodeError> {
        if amount <= 1e-9 {
            return Ok(());
        }
        self.state.e -= amount;
        self.state.retracted += amount;
        writeln!(
            self.out,
            "G1 E{:.5} F{:.0} ; retract",
            self.state.e,
            self.settings.retraction_speed_mm_s * 60.0
        )?;
        Ok(())
    }

    fn wipe(&mut self, remaining: f64) -> Result<(), GcodeError> {
        if remaining <= 1e-9 || self.state.wipe.len() < 2 {
            return Ok(());
        }
        let path_len = xy_len(&self.state.wipe);
        if path_len <= TRAVEL_EPS_MM {
            return Ok(());
        }
        let mut wipe_dist = self.settings.wipe_distance_mm;
        if path_len < wipe_dist {
            wipe_dist = path_len;
        }
        wipe_dist = wipe_dist.max(1e-9);
        let clipped = clip_prefix(&self.state.wipe, wipe_dist);
        if clipped.len() < 2 {
            return Ok(());
        }
        let actual = xy_len(&clipped).max(1e-9);
        writeln!(self.out, "; WIPE_START")?;
        let wipe_f = self.wipe_feed_mm_min();
        for window in clipped.windows(2) {
            let seg = xy_dist(window[0], window[1]);
            let d_e = remaining * (seg / actual) * 0.95;
            self.state.e -= d_e;
            self.state.retracted += d_e;
            writeln!(
                self.out,
                "G1 X{:.3} Y{:.3} E{:.5} F{:.0} ;_WIPE",
                window[1].0, window[1].1, self.state.e, wipe_f
            )?;
            self.state.last = Some(window[1]);
        }
        writeln!(self.out, "; WIPE_END")?;
        Ok(())
    }

    fn wipe_feed_mm_min(&self) -> f64 {
        if self.settings.role_base_wipe_speed && self.state.last_print_f > 1e-9 {
            self.state.last_print_f
        } else {
            self.settings.travel_speed_mm_s * self.settings.wipe_speed_percent / 100.0 * 60.0
        }
    }

    pub(crate) fn retract(&mut self) -> Result<(), GcodeError> {
        let length = self.settings.retraction_length_mm;
        if length <= 1e-9 {
            self.state.wipe.clear();
            return Ok(());
        }
        let remaining = (length - self.state.retracted).max(0.0);
        if remaining > 1e-9 {
            let can_wipe = self.settings.wipe
                && self.settings.wipe_distance_mm > 1e-9
                && self.state.wipe.len() >= 2;
            if can_wipe {
                let before = remaining * self.settings.retract_before_wipe.clamp(0.0, 1.0);
                self.emit_retract_e(before)?;
                let leftover = (length - self.state.retracted).max(0.0);
                self.wipe(leftover)?;
                let still = (length - self.state.retracted).max(0.0);
                self.emit_retract_e(still)?;
            } else {
                self.emit_retract_e(remaining)?;
            }
        }
        self.state.wipe.clear();
        self.queue_lift();
        Ok(())
    }

    /// C++ `lazy_lift`: remember the hop until the next XY travel.
    fn queue_lift(&mut self) {
        if self.state.lifted > 1e-9
            || self.state.to_lift > 1e-9
            || !self.settings.z_hop_in_range(self.state.z)
        {
            return;
        }
        self.state.to_lift = self.settings.z_hop_mm;
        self.state.to_lift_type = self.settings.z_hop_type;
    }

    /// C++ `travel_to_xyz` hop that was delayed by `lazy_lift`.
    /// Auto: spiral if the clipped travel hits `loverhangs`, else slope.
    fn apply_lazy_lift(&mut self, dest: (f64, f64)) -> Result<(), GcodeError> {
        if self.state.to_lift <= 1e-9 {
            return Ok(());
        }
        let hop = self.state.to_lift;
        let hop_z = self.state.z + hop;
        self.state.to_lift = 0.0;
        self.state.lifted = hop;
        let z_feed = self.settings.z_travel_speed_mm_s() * 60.0;
        let Some(from) = self.state.last else {
            writeln!(self.out, "G1 Z{:.3} F{:.0} ; normal lift Z", hop_z, z_feed)?;
            self.state.z = hop_z;
            return Ok(());
        };
        let dist = xy_dist(from, dest);
        if dist > TRAVEL_EPS_MM {
            let lift = match self.state.to_lift_type {
                ZHopType::Auto => {
                    if travel_through_overhang(from, dest, hop, &self.state.lift_overhangs) {
                        ZHopType::Spiral
                    } else {
                        ZHopType::Slope
                    }
                }
                other => other,
            };
            match lift {
                ZHopType::Spiral => {
                    if let Some((i, j)) =
                        spiral_ij_on_bed(from, dest, spiral_radius(hop), self.settings)
                    {
                        writeln!(self.out, "G17")?;
                        writeln!(
                            self.out,
                            "G2 Z{:.3} I{:.3} J{:.3} P1 F{:.0} ; spiral lift Z",
                            hop_z, i, j, z_feed
                        )?;
                    } else {
                        writeln!(self.out, "G1 Z{:.3} F{:.0} ; normal lift Z", hop_z, z_feed)?;
                    }
                }
                ZHopType::Slope => {
                    if hop.atan2(dist) < SLOPE_THRESHOLD_RAD {
                        let run = hop / SLOPE_THRESHOLD_RAD.tan();
                        let ux = (dest.0 - from.0) / dist;
                        let uy = (dest.1 - from.1) / dist;
                        writeln!(
                            self.out,
                            "G1 X{:.3} Y{:.3} Z{:.3} F{:.0} ; slope lift Z",
                            from.0 + ux * run,
                            from.1 + uy * run,
                            hop_z,
                            self.travel_f
                        )?;
                    }
                }
                ZHopType::Normal | ZHopType::Auto => {
                    writeln!(self.out, "G1 Z{:.3} F{:.0} ; normal lift Z", hop_z, z_feed)?;
                }
            }
        } else {
            writeln!(self.out, "G1 Z{:.3} F{:.0} ; normal lift Z", hop_z, z_feed)?;
        }
        self.state.z = hop_z;
        Ok(())
    }

    fn unlift(&mut self) -> Result<(), GcodeError> {
        if self.state.lifted <= 1e-9 {
            self.state.to_lift = 0.0;
            return Ok(());
        }
        self.state.z -= self.state.lifted;
        self.state.lifted = 0.0;
        self.state.to_lift = 0.0;
        writeln!(
            self.out,
            "G1 Z{:.3} F{:.0} ; restore layer Z",
            self.state.z,
            self.settings.z_travel_speed_mm_s() * 60.0
        )?;
        Ok(())
    }

    pub(crate) fn unretract(&mut self) -> Result<(), GcodeError> {
        self.unlift()?;
        if self.state.retracted <= 1e-9 {
            return Ok(());
        }
        let d_e = self.state.retracted + self.settings.retract_restart_extra_mm;
        self.state.e += d_e;
        self.state.retracted = 0.0;
        writeln!(
            self.out,
            "G1 E{:.5} F{:.0} ; unretract",
            self.state.e,
            self.settings.deretract_speed_mm_s() * 60.0
        )?;
        Ok(())
    }

    pub(crate) fn travel_to(&mut self, dest: (f64, f64)) -> Result<(), GcodeError> {
        if let Some(prev) = self.state.last {
            let dist = xy_dist(prev, dest);
            if dist < TRAVEL_EPS_MM {
                return Ok(());
            }
            if dist + 1e-9 >= self.settings.retraction_minimum_travel_mm {
                self.retract()?;
            }
            self.emit_accel(self.settings.travel_acceleration_for_move(
                self.state.first_layer,
                self.state.short_travel_role,
                dist,
            ))?;
            self.apply_lazy_lift(dest)?;
        }
        if self.state.lifted > 1e-9 {
            writeln!(
                self.out,
                "G0 X{:.3} Y{:.3} Z{:.3} F{:.0}",
                dest.0, dest.1, self.state.z, self.travel_f
            )?;
        } else {
            writeln!(
                self.out,
                "G0 X{:.3} Y{:.3} F{:.0}",
                dest.0, dest.1, self.travel_f
            )?;
        }
        self.state.last = Some(dest);
        Ok(())
    }

    pub(crate) fn emit_one_path(
        &mut self,
        path: &[Point],
        closed: bool,
        e_per_mm: f64,
        print_f: f64,
        external_perimeter: bool,
    ) -> Result<(), GcodeError> {
        if path.len() < 2 {
            return Ok(());
        }
        let pts: Vec<(f64, f64)> = path.iter().copied().map(xy).collect();
        let start = pts[0];
        self.travel_to(start)?;
        self.unretract()?;
        let print_accel = self.state.print_accel;
        self.emit_accel(print_accel)?;
        let n = pts.len();
        let end = if closed { n } else { n - 1 };
        let mut trail = vec![start];
        let marker = if external_perimeter {
            ";_EXTRUDE_SET_SPEED;_EXTERNAL_PERIMETER"
        } else {
            ""
        };
        for i in 0..end {
            let a = pts[i];
            let b = pts[(i + 1) % n];
            let dist = xy_dist(a, b);
            self.state.e += dist * e_per_mm;
            writeln!(
                self.out,
                "G1 X{:.3} Y{:.3} E{:.5} F{:.0}{marker}",
                b.0, b.1, self.state.e, print_f
            )?;
            self.state.last = Some(b);
            trail.push(b);
        }
        self.state.last_print_f = print_f;
        self.state.wipe = trail.into_iter().rev().collect();
        Ok(())
    }
}

pub(crate) fn xy_dist(a: (f64, f64), b: (f64, f64)) -> f64 {
    ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt()
}

fn xy_len(path: &[(f64, f64)]) -> f64 {
    path.windows(2).map(|w| xy_dist(w[0], w[1])).sum()
}

fn clip_prefix(path: &[(f64, f64)], max_len: f64) -> Vec<(f64, f64)> {
    if path.len() < 2 || max_len <= TRAVEL_EPS_MM {
        return Vec::new();
    }
    let mut out = vec![path[0]];
    let mut remaining = max_len;
    for window in path.windows(2) {
        let d = xy_dist(window[0], window[1]);
        if d <= remaining {
            out.push(window[1]);
            remaining -= d;
            if remaining <= TRAVEL_EPS_MM {
                break;
            }
        } else {
            let t = remaining / d;
            out.push((
                window[0].0 + (window[1].0 - window[0].0) * t,
                window[0].1 + (window[1].1 - window[0].1) * t,
            ));
            break;
        }
    }
    out
}

fn spiral_radius(hop: f64) -> f64 {
    hop / (2.0 * std::f64::consts::PI * SLOPE_THRESHOLD_RAD.atan())
}

fn spiral_ij(from: (f64, f64), to: (f64, f64), radius: f64) -> (f64, f64) {
    let dx = to.0 - from.0;
    let dy = to.1 - from.1;
    let len = (dx * dx + dy * dy).sqrt();
    if len <= TRAVEL_EPS_MM {
        return (radius, 0.0);
    }
    let nx = dx / len;
    let ny = dy / len;
    (-ny * radius, nx * radius)
}

/// C++ `travel_to_xyz`: try CCW perp, then CW; otherwise no in-bed spiral.
fn spiral_ij_on_bed(
    from: (f64, f64),
    to: (f64, f64),
    radius: f64,
    settings: &SliceSettings,
) -> Option<(f64, f64)> {
    let (i, j) = spiral_ij(from, to, radius);
    if settings.spiral_arc_within_bed(from.0 + i, from.1 + j, radius) {
        return Some((i, j));
    }
    if settings.spiral_arc_within_bed(from.0 - i, from.1 - j, radius) {
        return Some((-i, -j));
    }
    None
}

/// C++ Auto hop: clipped travel intersecting `loverhangs` → spiral, else slope.
fn travel_through_overhang(
    from: (f64, f64),
    dest: (f64, f64),
    hop: f64,
    overhangs: &[Polygon],
) -> bool {
    if overhangs.is_empty() {
        return false;
    }
    let dist = xy_dist(from, dest);
    if dist <= TRAVEL_EPS_MM {
        return false;
    }
    let clip = (hop / SLOPE_THRESHOLD_RAD.tan()).min(dist);
    let ux = (dest.0 - from.0) / dist;
    let uy = (dest.1 - from.1) / dist;
    let end = (from.0 + ux * clip, from.1 + uy * clip);
    let px = -uy * TRAVEL_HIT_HALF_WIDTH_MM;
    let py = ux * TRAVEL_HIT_HALF_WIDTH_MM;
    let stroke = vec![
        Point::from_mm(from.0 + px, from.1 + py),
        Point::from_mm(end.0 + px, end.1 + py),
        Point::from_mm(end.0 - px, end.1 - py),
        Point::from_mm(from.0 - px, from.1 - py),
    ];
    !intersect_polygons(&[stroke], overhangs).is_empty()
}

pub(crate) fn lift_overhangs_in_window(layers: &[Layer], print_z: f64) -> Vec<Polygon> {
    let z0 = (print_z - LIFT_PROTECT_Z_MM).max(0.0);
    layers
        .iter()
        .filter(|l| l.print_z_mm + 1e-9 >= z0 && l.print_z_mm <= print_z + 1e-9)
        .flat_map(|l| l.lift_overhangs.iter().cloned())
        .collect()
}

fn xy(p: Point) -> (f64, f64) {
    (unscale(p.x), unscale(p.y))
}
