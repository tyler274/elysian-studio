//! C++ `GCodeWriter` motion: retract, wipe, lazy Z-hop, travel, one extrusion path.

use std::fmt::Write as _;

use bambu_config::{FlowRole, PrintAccel, SliceSettings, ZHopType};
use bambu_geom::{
    clip_end, douglas_peucker, fit_arcs_and_simplify, intersect_polygons, offset_polygons, unscale,
    ArcDir, PathFit, PathFitKind, Point, Polygon, Polyline,
};
use bambu_slicer::{point_in_polygons, Layer};

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
    /// C++ `is_perimeter(role)` for the extrusion this travel is heading toward.
    pub(crate) dest_is_perimeter: bool,
    /// C++ destination `erSupportMaterial` / `erSupportTransition`.
    pub(crate) dest_is_support: bool,
    /// Last `T` command (C++ current extruder), if we emitted one.
    pub(crate) current_tool: Option<i32>,
    /// C++ `is_perimeter(last) && last != erPerimeter` after the previous extrusion.
    pub(crate) last_leave_forces_retract: bool,
    /// Last `; LINE_WIDTH:` value (C++ `m_last_width`).
    pub(crate) last_line_width: Option<f64>,
    /// C++ internal infill islands for `travel_inside_internal_regions`.
    pub(crate) internal_islands: Vec<Polygon>,
    /// Current-layer wall polylines for `travel_cross_perimeters`.
    pub(crate) wall_paths: Vec<Polyline>,
    /// C++ `Layer::lslices` stand-in for `reduce_crossing_wall` detours.
    pub(crate) layer_contours: Vec<Polygon>,
    /// C++ `SupportLayer::support_islands` for support-island travel.
    pub(crate) support_islands: Vec<Polygon>,
    /// Current slab height (`Layer::height`) for scarf Z.
    pub(crate) layer_height_mm: f64,
    /// C++ `m_nominal_z` (top of the current slab).
    pub(crate) layer_print_z_mm: f64,
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
        self.state.dest_is_perimeter =
            matches!(kind, PrintAccel::OuterWall | PrintAccel::InnerWall);
        self.state.dest_is_support = false;
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
        let Some(mut from) = self.state.last else {
            self.emit_xy_travel(dest)?;
            self.state.last = Some(dest);
            return Ok(());
        };
        let dist = xy_dist(from, dest);
        if dist < TRAVEL_EPS_MM {
            return Ok(());
        }
        let mut hops = self.avoid_crossing_hops(from, dest);
        let path_len = hops_length(from, &hops);
        if path_len + 1e-9 >= self.settings.retraction_minimum_travel_mm
            && !self.skip_retract(from, dest)
        {
            self.retract()?;
            if let Some(now) = self.state.last {
                if xy_dist(now, from) > TRAVEL_EPS_MM {
                    from = now;
                    hops = self.avoid_crossing_hops(from, dest);
                }
            }
        }
        self.emit_accel(self.settings.travel_acceleration_for_move(
            self.state.first_layer,
            self.state.short_travel_role,
            hops_length(from, &hops),
        ))?;
        self.apply_lazy_lift(dest)?;
        for hop in hops {
            if let Some(prev) = self.state.last {
                if xy_dist(prev, hop) < TRAVEL_EPS_MM {
                    continue;
                }
            }
            self.emit_xy_travel(hop)?;
            self.state.last = Some(hop);
        }
        self.state.last = Some(dest);
        Ok(())
    }

    fn emit_xy_travel(&mut self, dest: (f64, f64)) -> Result<(), GcodeError> {
        if self.state.lifted > 1e-9 {
            writeln!(
                self.out,
                "G1 X{:.3} Y{:.3} Z{:.3} F{:.0}",
                dest.0, dest.1, self.state.z, self.travel_f
            )?;
        } else {
            writeln!(
                self.out,
                "G1 X{:.3} Y{:.3} F{:.0}",
                dest.0, dest.1, self.travel_f
            )?;
        }
        Ok(())
    }

    /// C++ `AvoidCrossingPerimeters::travel_to` stand-in: walk the shorter contour arc.
    fn avoid_crossing_hops(&self, from: (f64, f64), dest: (f64, f64)) -> Vec<(f64, f64)> {
        if !self.settings.reduce_crossing_wall
            || xy_dist(from, dest) + 1e-9 < self.settings.retraction_minimum_travel_mm
        {
            return vec![dest];
        }
        let mut boundaries = self.state.layer_contours.clone();
        if self.settings.avoid_crossing_wall_includes_support {
            boundaries.extend(self.state.support_islands.iter().cloned());
        }
        if boundaries.is_empty() {
            return vec![dest];
        }
        let spacing = self
            .settings
            .line_width_for(FlowRole::ExternalPerimeter, self.state.first_layer)
            .max(0.2);
        let grown = offset_polygons(&boundaries, spacing);
        let rings = if grown.is_empty() {
            &boundaries
        } else {
            &grown
        };
        let Some(mut hops) = contour_detour(from, dest, rings) else {
            return vec![dest];
        };
        hops.push(dest);
        let extra = hops_length(from, &hops) - xy_dist(from, dest);
        if extra
            > self
                .settings
                .max_travel_detour_limit_mm(xy_dist(from, dest))
        {
            return vec![dest];
        }
        hops
    }

    /// C++ `GCode::needs_retraction` after the minimum-travel check.
    fn skip_retract(&self, from: (f64, f64), dest: (f64, f64)) -> bool {
        // Leaving external/overhang perimeter always retracts (C++ `erExternalPerimeter`).
        if self.state.last_leave_forces_retract {
            return false;
        }
        if self.skip_support_island_retract(from, dest) {
            return true;
        }
        self.skip_infill_retract(from, dest)
    }

    /// C++ support-material travel fully inside `support_islands`.
    fn skip_support_island_retract(&self, from: (f64, f64), dest: (f64, f64)) -> bool {
        self.state.dest_is_support
            && !self.state.support_islands.is_empty()
            && travel_inside_islands(from, dest, &self.state.support_islands)
    }

    /// C++ `reduce_infill_retraction` + `travel_inside_internal_regions_no_wall_crossing`.
    fn skip_infill_retract(&self, from: (f64, f64), dest: (f64, f64)) -> bool {
        if self.state.dest_is_perimeter
            || !self.settings.should_reduce_infill_retraction()
            || self.settings.infill_density <= 0.0
            || self.state.internal_islands.is_empty()
        {
            return false;
        }
        travel_inside_islands(from, dest, &self.state.internal_islands)
            && !travel_crosses_polylines(from, dest, &self.state.wall_paths)
    }

    pub(crate) fn emit_one_path(
        &mut self,
        path: &[Point],
        closed: bool,
        e_per_mm: f64,
        print_f: f64,
        external_perimeter: bool,
        arc_tolerance_mm: f64,
    ) -> Result<(), GcodeError> {
        if path.len() < 2 {
            return Ok(());
        }
        let mut pts = path.to_vec();
        if closed && pts.first() != pts.last() {
            pts.push(pts[0]);
        }
        let scarf = self.should_scarf(closed, external_perimeter);
        // C++ `GCode::extrude_loop`: clip `seam_gap` unless spiral vase or a scarf
        // seam is active (`clip_length = 0` when `enable_seam_slope`).
        if closed && !self.settings.spiral_mode && !scarf {
            let gap = self.settings.seam_gap_mm();
            if gap > TRAVEL_EPS_MM {
                pts = clip_end(&pts, gap);
            }
        }
        if pts.len() < 2 {
            return Ok(());
        }
        let start = xy(pts[0]);
        self.travel_to(start)?;
        self.unretract()?;
        let print_accel = self.state.print_accel;
        self.emit_accel(print_accel)?;
        let marker = if external_perimeter {
            ";_EXTRUDE_SET_SPEED;_EXTERNAL_PERIMETER"
        } else {
            ""
        };
        if scarf {
            self.emit_scarfed_linear_path(&pts, e_per_mm, print_f, marker)?;
        } else if self.settings.enable_arc_fitting && !self.settings.spiral_mode {
            // C++ `LayerRegion::simplify_path`: arc-fit when enabled, else Douglas-Peucker
            // with `resolution` (including spiral mode, which cannot emit G2/G3).
            let (simplified, fits) = fit_arcs_and_simplify(&pts, arc_tolerance_mm.max(0.001));
            self.emit_fitted_path(&simplified, &fits, e_per_mm, print_f, marker)?;
        } else {
            let simplified = douglas_peucker(&pts, self.settings.resolution_mm.max(0.001));
            self.emit_linear_path(&simplified, e_per_mm, print_f, marker)?;
        }
        self.state.last_print_f = print_f;
        // C++ `m_last_processor_extrusion_role`: outer walls are `erExternalPerimeter`.
        self.state.last_leave_forces_retract = external_perimeter;
        Ok(())
    }

    /// C++ `enable_seam_slope` without hole / conditional-angle gating.
    pub(crate) fn should_scarf(&self, closed: bool, external_perimeter: bool) -> bool {
        closed
            && !self.state.first_layer
            && !self.settings.spiral_mode
            && self.state.dest_is_perimeter
            && self.settings.seam_slope_min_length_mm > TRAVEL_EPS_MM
            && self.state.layer_height_mm > TRAVEL_EPS_MM
            && self.settings.scarf_applies_to_wall(external_perimeter)
    }

    /// C++ `ExtrusionLoopSloped` start ramp: Z from `start_ratio` to 1 over
    /// `min(min_length, loop)` (or the whole loop). Linear G1 only.
    fn emit_scarfed_linear_path(
        &mut self,
        pts: &[Point],
        e_per_mm: f64,
        print_f: f64,
        marker: &str,
    ) -> Result<(), GcodeError> {
        let loop_len: f64 = pts.windows(2).map(|w| xy_dist(xy(w[0]), xy(w[1]))).sum();
        if loop_len < TRAVEL_EPS_MM {
            return Ok(());
        }
        let scarf_len = if self.settings.seam_slope_entire_loop {
            loop_len
        } else {
            self.settings.seam_slope_min_length_mm.min(loop_len)
        };
        if scarf_len < TRAVEL_EPS_MM {
            return self.emit_linear_path(pts, e_per_mm, print_f, marker);
        }
        let steps = self.settings.seam_slope_steps.max(1) as f64;
        let max_seg = (scarf_len / steps).max(TRAVEL_EPS_MM);
        let start_ratio = self.settings.scarf_start_ratio(self.state.layer_height_mm);
        let height = self.state.layer_height_mm;
        let print_z = self.state.layer_print_z_mm;
        let start_z = lerp(print_z - height, print_z, start_ratio);
        if (self.state.z - start_z).abs() > TRAVEL_EPS_MM {
            writeln!(
                self.out,
                "G1 Z{:.3} F{:.0}",
                start_z,
                self.settings.z_travel_speed_mm_s() * 60.0
            )?;
            self.state.z = start_z;
        }
        let mut walked = 0.0;
        let mut trail = vec![xy(pts[0])];
        for window in pts.windows(2) {
            let b = xy(window[1]);
            let dist = xy_dist(xy(window[0]), b);
            if dist < TRAVEL_EPS_MM {
                continue;
            }
            let mut remaining = dist;
            let mut from = xy(window[0]);
            while remaining > TRAVEL_EPS_MM {
                if walked + TRAVEL_EPS_MM >= scarf_len {
                    self.push_extrude_xy(b, remaining * e_per_mm, print_f, marker, &mut trail)?;
                    walked += remaining;
                    break;
                }
                let take = remaining.min(scarf_len - walked).min(max_seg);
                let t = take / remaining;
                let dest = (from.0 + (b.0 - from.0) * t, from.1 + (b.1 - from.1) * t);
                walked += take;
                remaining -= take;
                from = dest;
                let ratio = lerp(start_ratio, 1.0, (walked / scarf_len).min(1.0));
                let z = lerp(print_z - height, print_z, ratio);
                self.push_extrude_xyz(
                    dest,
                    z,
                    take * e_per_mm * ratio,
                    print_f,
                    marker,
                    &mut trail,
                )?;
            }
        }
        self.state.wipe = trail.into_iter().rev().collect();
        Ok(())
    }

    fn push_extrude_xy(
        &mut self,
        dest: (f64, f64),
        de: f64,
        print_f: f64,
        marker: &str,
        trail: &mut Vec<(f64, f64)>,
    ) -> Result<(), GcodeError> {
        self.state.e += de;
        writeln!(
            self.out,
            "G1 X{:.3} Y{:.3} E{:.5} F{:.0}{marker}",
            dest.0, dest.1, self.state.e, print_f
        )?;
        self.state.last = Some(dest);
        trail.push(dest);
        Ok(())
    }

    fn push_extrude_xyz(
        &mut self,
        dest: (f64, f64),
        z: f64,
        de: f64,
        print_f: f64,
        marker: &str,
        trail: &mut Vec<(f64, f64)>,
    ) -> Result<(), GcodeError> {
        self.state.e += de;
        writeln!(
            self.out,
            "G1 X{:.3} Y{:.3} Z{:.3} E{:.5} F{:.0}{marker}",
            dest.0, dest.1, z, self.state.e, print_f
        )?;
        self.state.last = Some(dest);
        self.state.z = z;
        trail.push(dest);
        Ok(())
    }

    fn emit_linear_path(
        &mut self,
        pts: &[Point],
        e_per_mm: f64,
        print_f: f64,
        marker: &str,
    ) -> Result<(), GcodeError> {
        let mut trail = vec![xy(pts[0])];
        for window in pts.windows(2) {
            let a = xy(window[0]);
            let b = xy(window[1]);
            let dist = xy_dist(a, b);
            if dist < TRAVEL_EPS_MM {
                continue;
            }
            self.state.e += dist * e_per_mm;
            writeln!(
                self.out,
                "G1 X{:.3} Y{:.3} E{:.5} F{:.0}{marker}",
                b.0, b.1, self.state.e, print_f
            )?;
            self.state.last = Some(b);
            trail.push(b);
        }
        self.state.wipe = trail.into_iter().rev().collect();
        Ok(())
    }

    fn emit_fitted_path(
        &mut self,
        pts: &[Point],
        fits: &[PathFit],
        e_per_mm: f64,
        print_f: f64,
        marker: &str,
    ) -> Result<(), GcodeError> {
        if pts.len() < 2 {
            return Ok(());
        }
        let mut trail = vec![xy(pts[0])];
        for fit in fits {
            match fit.kind {
                PathFitKind::Linear => {
                    for i in (fit.start + 1)..=fit.end {
                        let b = xy(pts[i]);
                        let a = *trail.last().unwrap_or(&xy(pts[fit.start]));
                        let dist = xy_dist(a, b);
                        if dist < TRAVEL_EPS_MM {
                            continue;
                        }
                        self.state.e += dist * e_per_mm;
                        writeln!(
                            self.out,
                            "G1 X{:.3} Y{:.3} E{:.5} F{:.0}{marker}",
                            b.0, b.1, self.state.e, print_f
                        )?;
                        self.state.last = Some(b);
                        trail.push(b);
                    }
                }
                PathFitKind::Arc(arc) => {
                    if arc.length_mm < TRAVEL_EPS_MM {
                        continue;
                    }
                    let end = xy(arc.end);
                    let start = xy(arc.start);
                    let i = arc.center_mm.0 - start.0;
                    let j = arc.center_mm.1 - start.1;
                    let cmd = match arc.dir {
                        ArcDir::Ccw => "G3",
                        ArcDir::Cw => "G2",
                    };
                    self.state.e += arc.length_mm * e_per_mm;
                    writeln!(
                        self.out,
                        "{cmd} X{:.3} Y{:.3} I{:.3} J{:.3} E{:.5} F{:.0}{marker}",
                        end.0, end.1, i, j, self.state.e, print_f
                    )?;
                    self.state.last = Some(end);
                    for p in pts
                        .iter()
                        .take(fit.end.min(pts.len() - 1) + 1)
                        .skip(fit.start + 1)
                    {
                        trail.push(xy(*p));
                    }
                    if trail.last().copied() != Some(end) {
                        trail.push(end);
                    }
                }
            }
        }
        self.state.wipe = trail.into_iter().rev().collect();
        Ok(())
    }
}

pub(crate) fn xy_dist(a: (f64, f64), b: (f64, f64)) -> f64 {
    ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt()
}

fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
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

fn travel_inside_islands(from: (f64, f64), dest: (f64, f64), islands: &[Polygon]) -> bool {
    const SAMPLES: i32 = 8;
    for i in 0..=SAMPLES {
        let t = f64::from(i) / f64::from(SAMPLES);
        let p = Point::from_mm(
            from.0 + (dest.0 - from.0) * t,
            from.1 + (dest.1 - from.1) * t,
        );
        if !point_in_polygons(p, islands) {
            return false;
        }
    }
    true
}

fn travel_crosses_polylines(from: (f64, f64), dest: (f64, f64), paths: &[Polyline]) -> bool {
    for path in paths {
        for window in path.windows(2) {
            let a = xy(window[0]);
            let b = xy(window[1]);
            if segments_properly_intersect(from, dest, a, b) {
                return true;
            }
        }
    }
    false
}

fn orient(a: (f64, f64), b: (f64, f64), c: (f64, f64)) -> f64 {
    (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0)
}

fn segments_properly_intersect(
    a0: (f64, f64),
    a1: (f64, f64),
    b0: (f64, f64),
    b1: (f64, f64),
) -> bool {
    let o1 = orient(a0, a1, b0);
    let o2 = orient(a0, a1, b1);
    let o3 = orient(b0, b1, a0);
    let o4 = orient(b0, b1, a1);
    o1 * o2 < 0.0 && o3 * o4 < 0.0
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

fn hops_length(start: (f64, f64), hops: &[(f64, f64)]) -> f64 {
    let mut prev = start;
    let mut len = 0.0;
    for &p in hops {
        len += xy_dist(prev, p);
        prev = p;
    }
    len
}

fn closed_vertex_count(poly: &Polygon) -> usize {
    let n = poly.len();
    if n >= 2 && poly[0] == poly[n - 1] {
        n - 1
    } else {
        n
    }
}

fn path_length(pts: &[(f64, f64)]) -> f64 {
    pts.windows(2).map(|w| xy_dist(w[0], w[1])).sum()
}

/// Walk the shorter contour arc between the first and last chord intersections.
fn contour_detour(
    from: (f64, f64),
    dest: (f64, f64),
    rings: &[Polygon],
) -> Option<Vec<(f64, f64)>> {
    struct Hit {
        t: f64,
        poly: usize,
        edge: usize,
        pt: (f64, f64),
    }
    let mut hits = Vec::new();
    for (pi, poly) in rings.iter().enumerate() {
        let n = closed_vertex_count(poly);
        if n < 3 {
            continue;
        }
        for i in 0..n {
            let a = xy(poly[i]);
            let b = xy(poly[(i + 1) % n]);
            if let Some((t, pt)) = segment_intersection_t(from, dest, a, b) {
                hits.push(Hit {
                    t,
                    poly: pi,
                    edge: i,
                    pt,
                });
            }
        }
    }
    hits.sort_by(|a, b| a.t.total_cmp(&b.t));
    if hits.len() < 2 {
        return None;
    }
    let first = hits.first()?;
    let last = hits.last()?;
    if first.poly != last.poly {
        return None;
    }
    let poly = &rings[first.poly];
    let fwd = walk_contour(poly, first.edge, first.pt, last.edge, last.pt, true);
    let back = walk_contour(poly, first.edge, first.pt, last.edge, last.pt, false);
    let walk = if path_length(&fwd) <= path_length(&back) {
        fwd
    } else {
        back
    };
    if walk.len() < 2 {
        return None;
    }
    Some(walk.into_iter().skip(1).collect())
}

fn walk_contour(
    poly: &Polygon,
    from_edge: usize,
    from_pt: (f64, f64),
    to_edge: usize,
    to_pt: (f64, f64),
    forward: bool,
) -> Vec<(f64, f64)> {
    let n = closed_vertex_count(poly);
    let mut pts = vec![from_pt];
    if from_edge == to_edge || n < 3 {
        pts.push(to_pt);
        return pts;
    }
    if forward {
        let mut i = (from_edge + 1) % n;
        loop {
            pts.push(xy(poly[i]));
            if i == to_edge {
                break;
            }
            i = (i + 1) % n;
        }
    } else {
        let stop = (to_edge + 1) % n;
        let mut i = from_edge;
        loop {
            pts.push(xy(poly[i]));
            if i == stop {
                break;
            }
            i = (i + n - 1) % n;
        }
    }
    pts.push(to_pt);
    pts
}

fn segment_intersection_t(
    a0: (f64, f64),
    a1: (f64, f64),
    b0: (f64, f64),
    b1: (f64, f64),
) -> Option<(f64, (f64, f64))> {
    let dx = a1.0 - a0.0;
    let dy = a1.1 - a0.1;
    let ex = b1.0 - b0.0;
    let ey = b1.1 - b0.1;
    let denom = dx * ey - dy * ex;
    if denom.abs() < 1e-12 {
        return None;
    }
    let t = ((b0.0 - a0.0) * ey - (b0.1 - a0.1) * ex) / denom;
    let u = ((b0.0 - a0.0) * dy - (b0.1 - a0.1) * dx) / denom;
    if t <= 1e-6 || t >= 1.0 - 1e-6 || u <= 1e-6 || u >= 1.0 - 1e-6 {
        return None;
    }
    Some((t, (a0.0 + t * dx, a0.1 + t * dy)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bambu_config::ReduceInfillRetractionMode;

    #[test]
    fn clip_suffix_shortens_closed_square() {
        let path = vec![
            Point::from_mm(0.0, 0.0),
            Point::from_mm(10.0, 0.0),
            Point::from_mm(10.0, 10.0),
            Point::from_mm(0.0, 10.0),
            Point::from_mm(0.0, 0.0),
        ];
        let clipped = clip_end(&path, 0.06);
        assert_eq!(clipped.len(), 5);
        let (x, y) = clipped.last().unwrap().to_mm();
        assert!(x.abs() < 1e-6, "{x}");
        assert!((y - 0.06).abs() < 1e-6, "{y}");
    }

    #[test]
    fn proper_intersection_ignores_shared_endpoint() {
        assert!(segments_properly_intersect(
            (0.0, 0.0),
            (2.0, 2.0),
            (0.0, 2.0),
            (2.0, 0.0)
        ));
        assert!(!segments_properly_intersect(
            (0.0, 0.0),
            (1.0, 0.0),
            (1.0, 0.0),
            (2.0, 0.0)
        ));
    }

    fn island_square() -> Vec<Polygon> {
        vec![vec![
            Point::from_mm(0.0, 0.0),
            Point::from_mm(10.0, 0.0),
            Point::from_mm(10.0, 10.0),
            Point::from_mm(0.0, 10.0),
        ]]
    }

    #[test]
    fn skip_retract_inside_infill_islands() {
        let mut settings = SliceSettings::default();
        settings.reduce_infill_retraction_mode = ReduceInfillRetractionMode::Enabled;
        settings.infill_density = 0.2;
        let mut w = Writer::new(&settings);
        w.state.internal_islands = island_square();
        w.state.dest_is_perimeter = false;
        w.state.last_leave_forces_retract = false;
        assert!(w.skip_retract((1.0, 1.0), (8.0, 8.0)));
    }

    #[test]
    fn leaving_external_perimeter_forces_retract() {
        let mut settings = SliceSettings::default();
        settings.reduce_infill_retraction_mode = ReduceInfillRetractionMode::Enabled;
        settings.infill_density = 0.2;
        let mut w = Writer::new(&settings);
        w.state.internal_islands = island_square();
        w.state.dest_is_perimeter = false;
        w.state.last_leave_forces_retract = true;
        assert!(!w.skip_retract((1.0, 1.0), (8.0, 8.0)));
    }

    #[test]
    fn skip_retract_inside_support_islands() {
        let settings = SliceSettings::default();
        let mut w = Writer::new(&settings);
        w.state.dest_is_support = true;
        w.state.support_islands = island_square();
        w.state.last_leave_forces_retract = false;
        assert!(w.skip_retract((1.0, 1.0), (8.0, 8.0)));
    }

    #[test]
    fn support_interface_does_not_use_support_islands() {
        let settings = SliceSettings::default();
        let mut w = Writer::new(&settings);
        w.state.dest_is_support = false;
        w.state.support_islands = island_square();
        w.state.internal_islands.clear();
        w.state.last_leave_forces_retract = false;
        assert!(!w.skip_retract((1.0, 1.0), (8.0, 8.0)));
    }

    fn xy_travel_count(gcode: &str) -> usize {
        gcode
            .lines()
            .filter(|l| {
                l.trim().starts_with("G1 X")
                    && !l.split_whitespace().any(|tok| tok.starts_with('E'))
            })
            .count()
    }

    #[test]
    fn reduce_crossing_wall_walks_around_square() {
        let from = (-1.0, 5.0);
        let dest = (11.0, 5.0);
        let hops = contour_detour(from, dest, &island_square()).expect("two intersections");
        assert!(hops.len() >= 2, "should walk a corner, got {hops:?}");
        assert!(
            hops.iter()
                .any(|p| p.0.abs() < 1e-6 || (p.0 - 10.0).abs() < 1e-6),
            "detour should hug a vertical side: {hops:?}"
        );
    }

    #[test]
    fn reduce_crossing_wall_emits_multi_hop_travel() {
        let mut settings = SliceSettings::default();
        settings.reduce_crossing_wall = true;
        settings.retraction_minimum_travel_mm = 1.0;
        settings.wipe = false;
        let mut w = Writer::new(&settings);
        w.state.last = Some((-1.0, 5.0));
        w.state.layer_contours = island_square();
        w.travel_to((11.0, 5.0)).unwrap();
        let n = xy_travel_count(&w.out);
        assert!(n >= 3, "expected contour hops, got {n} in {}", w.out);

        settings.max_travel_detour_distance = 0.1;
        let mut capped = Writer::new(&settings);
        capped.state.last = Some((-1.0, 5.0));
        capped.state.layer_contours = island_square();
        capped.travel_to((11.0, 5.0)).unwrap();
        assert_eq!(
            xy_travel_count(&capped.out),
            1,
            "tiny max detour keeps the straight hop"
        );

        settings.max_travel_detour_distance = 0.0;
        settings.reduce_crossing_wall = false;
        let mut off = Writer::new(&settings);
        off.state.last = Some((-1.0, 5.0));
        off.state.layer_contours = island_square();
        off.travel_to((11.0, 5.0)).unwrap();
        assert_eq!(xy_travel_count(&off.out), 1);
    }

    #[test]
    fn avoid_crossing_wall_includes_support_islands() {
        let mut settings = SliceSettings::default();
        settings.reduce_crossing_wall = true;
        settings.retraction_minimum_travel_mm = 1.0;
        settings.wipe = false;
        let mut without = Writer::new(&settings);
        without.state.last = Some((-1.0, 5.0));
        without.state.support_islands = island_square();
        without.travel_to((11.0, 5.0)).unwrap();
        assert_eq!(xy_travel_count(&without.out), 1);

        settings.avoid_crossing_wall_includes_support = true;
        let mut with = Writer::new(&settings);
        with.state.last = Some((-1.0, 5.0));
        with.state.support_islands = island_square();
        with.travel_to((11.0, 5.0)).unwrap();
        let n = xy_travel_count(&with.out);
        assert!(
            n >= 3,
            "support island should detour, got {n} in {}",
            with.out
        );
    }
}
