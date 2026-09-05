//! Orchestrate one G-code export: envelope, per-layer roles, cooling, placeholders.

use std::fmt::Write as _;

use bambu_config::{Flow, PrintAccel, SliceSettings};
use bambu_slicer::SliceResult;

use crate::envelope::first_layer_print_box;
use crate::motion::{lift_overhangs_in_window, Writer};
use crate::paths::{overhang_rings, Extrude};
use crate::GcodeError;

pub fn write_gcode(settings: &SliceSettings, sliced: &SliceResult) -> Result<String, GcodeError> {
    let max_z = sliced.layers.last().map(|l| l.print_z_mm).unwrap_or(0.0);
    let mut w = Writer::new(settings);
    w.emit_header(max_z)?;
    let custom_ctx =
        if settings.machine_start_gcode.is_empty() && settings.machine_end_gcode.is_empty() {
            None
        } else {
            let (first_min, first_size) = first_layer_print_box(sliced);
            Some(settings.placeholder_custom_gcode_context(
                sliced.layers.len().saturating_sub(1),
                sliced.layers.len(),
                max_z,
                first_min,
                first_size,
            ))
        };
    w.emit_start(custom_ctx.as_ref())?;

    let (object_min, object_max) = crate::timelapse::object_xy_bbox(sliced);
    for (layer_i, layer) in sliced.layers.iter().enumerate() {
        let first = layer_i == 0;
        w.state.lift_overhangs = lift_overhangs_in_window(&sliced.layers, layer.print_z_mm);
        if layer_i > 0 && settings.retract_when_changing_layer {
            w.retract()?;
        }
        writeln!(w.out, "; CHANGE_LAYER")?;
        // C++ `process_layer`: `; Z_HEIGHT: %g` then `; LAYER_HEIGHT: %g`.
        // First-layer height is print_z; later layers use the slice delta.
        let height = if first {
            layer.print_z_mm
        } else {
            layer.height_mm
        };
        writeln!(w.out, "; Z_HEIGHT: {}", layer.print_z_mm)?;
        writeln!(w.out, "; LAYER_HEIGHT: {height}")?;
        writeln!(w.out, ";LAYER:{}", layer.index)?;
        w.state.first_layer = first;
        w.emit_accel(settings.travel_acceleration_for_layer(first))?;
        writeln!(w.out, "G1 Z{:.3} F600", layer.print_z_mm)?;
        w.state.z = layer.print_z_mm;
        w.state.lifted = 0.0;
        w.emit_layer_change_gcode(layer_i, layer.print_z_mm, sliced.layers.len(), max_z)?;
        if layer_i == 1 && !settings.layer_change_gcode.is_empty() {
            writeln!(w.out, "; open powerlost recovery")?;
            writeln!(w.out, "M1003 S1")?;
        }
        if layer_i == 1 {
            w.emit_scan_first_layer()?;
            w.emit_second_layer_temps()?;
        }
        writeln!(w.out, ";_SET_FAN_SPEED_CHANGING_LAYER")?;
        w.emit_wrapping_detection(layer_i, layer.print_z_mm, max_z)?;

        let flow = Flow::from_settings(settings, layer.height_mm);
        let e_per_mm = flow.e_per_mm();
        let mm3_per_mm = flow.mm3_per_mm();
        let feeds = LayerFeeds::for_layer(settings, first);
        let support_polys = if layer_i == 0 {
            None
        } else {
            overhang_rings(settings, sliced.layers.get(layer_i - 1))
        };
        let e = |paths, closed, print_f| Extrude {
            paths,
            closed,
            e_per_mm,
            print_f,
            mm3_per_mm,
        };

        w.emit_role(
            "Skirt",
            PrintAccel::Default,
            e(&layer.skirt, true, feeds.wall),
        )?;
        w.emit_role(
            "Brim",
            PrintAccel::Default,
            e(&layer.brim, true, feeds.wall),
        )?;
        w.emit_role(
            "Support",
            PrintAccel::Default,
            e(&layer.support, false, feeds.support),
        )?;
        w.emit_role(
            "Support interface",
            PrintAccel::Default,
            e(&layer.support_interface, false, feeds.support_interface),
        )?;

        w.set_print_role(PrintAccel::OuterWall);
        w.emit_wall_paths(
            "Outer wall",
            e(&layer.outer_walls, true, feeds.wall),
            support_polys.as_deref(),
            settings.enable_overhang_speed,
            !first,
        )?;
        w.set_print_role(PrintAccel::InnerWall);
        w.emit_wall_paths(
            "Inner wall",
            e(&layer.inner_walls, true, feeds.inner),
            support_polys.as_deref(),
            settings.enable_overhang_speed,
            !first,
        )?;
        w.emit_role(
            "Gap infill",
            PrintAccel::Default,
            e(&layer.gap_infill, false, feeds.gap),
        )?;
        w.emit_role(
            "Sparse infill",
            PrintAccel::SparseInfill,
            e(&layer.infill, false, feeds.sparse),
        )?;
        w.emit_role(
            "Internal solid infill",
            PrintAccel::Default,
            e(&layer.solid_infill, false, feeds.solid),
        )?;
        w.emit_floating_shell_paths(
            e(&layer.floating_vertical_shell, false, feeds.vertical_shell),
            &layer.floating_areas,
            feeds.bridge,
            first,
        )?;
        if !layer.bridge.is_empty() {
            w.emit_feature("Bridge")?;
            w.set_print_role(PrintAccel::Default);
            w.emit_marked(
                settings.overhang_fan_applies(5, true, false),
                ";_OVERHANG_FAN_START",
                ";_OVERHANG_FAN_END",
                |w| w.emit_paths(e(&layer.bridge, false, feeds.bridge)),
            )?;
        }
        w.emit_role(
            "Bottom surface",
            PrintAccel::Default,
            e(&layer.bottom_surface, false, feeds.wall),
        )?;
        w.emit_role(
            "Top surface",
            PrintAccel::TopSurface,
            e(&layer.top_surface, false, feeds.top),
        )?;
        if !layer.ironing.is_empty() {
            w.emit_feature("Ironing")?;
            w.set_print_role(PrintAccel::Default);
            let iron_flow =
                Flow::from_settings(settings, layer.height_mm * settings.ironing_flow.max(0.0));
            let iron_closed = layer.ironing.iter().any(|p| p.len() > 2);
            w.emit_marked(
                settings.ironing_fan_speed >= 0,
                ";_IRONING_FAN_START",
                ";_IRONING_FAN_END",
                |w| {
                    w.emit_paths(Extrude {
                        paths: &layer.ironing,
                        closed: iron_closed,
                        e_per_mm: iron_flow.e_per_mm(),
                        print_f: settings.ironing_speed_mm_s * 60.0,
                        mm3_per_mm: iron_flow.mm3_per_mm(),
                    })
                },
            )?;
        }
        w.emit_time_lapse(layer_i, layer, object_min, object_max, max_z)?;
    }

    w.emit_end(custom_ctx.as_ref())?;
    Ok(w.finish(sliced.layers.len()))
}

impl Writer<'_> {
    pub(crate) fn emit_feature(&mut self, feature: &str) -> Result<(), GcodeError> {
        writeln!(self.out, "; FEATURE: {feature}")?;
        let width = self.settings.line_width_mm;
        let changed = self
            .state
            .last_line_width
            .map(|prev| (prev - width).abs() > 1e-9)
            .unwrap_or(true);
        if changed {
            writeln!(self.out, "; LINE_WIDTH: {width}")?;
            self.state.last_line_width = Some(width);
        }
        Ok(())
    }

    fn emit_role(
        &mut self,
        feature: &str,
        role: PrintAccel,
        job: Extrude<'_>,
    ) -> Result<(), GcodeError> {
        if job.paths.is_empty() {
            return Ok(());
        }
        self.emit_feature(feature)?;
        self.set_print_role(role);
        self.emit_paths(job)
    }
}

struct LayerFeeds {
    wall: f64,
    inner: f64,
    sparse: f64,
    gap: f64,
    solid: f64,
    vertical_shell: f64,
    support: f64,
    support_interface: f64,
    bridge: f64,
    top: f64,
}

impl LayerFeeds {
    fn for_layer(settings: &SliceSettings, first: bool) -> Self {
        let first_f = settings.first_layer_speed_mm_s * 60.0;
        Self {
            wall: if first {
                first_f
            } else {
                settings.print_speed_mm_s * 60.0
            },
            inner: if first {
                first_f
            } else {
                settings.inner_wall_speed_mm_s * 60.0
            },
            sparse: if first {
                settings.first_layer_infill_speed_mm_s * 60.0
            } else {
                settings.infill_speed_mm_s * 60.0
            },
            gap: if first {
                first_f
            } else {
                settings.gap_infill_speed_mm_s * 60.0
            },
            solid: if first {
                first_f
            } else {
                settings.solid_infill_speed_mm_s * 60.0
            },
            vertical_shell: if first {
                first_f
            } else {
                settings.vertical_shell_speed_mm_s() * 60.0
            },
            support: if first {
                first_f
            } else {
                settings.support_speed_mm_s * 60.0
            },
            support_interface: if first {
                first_f
            } else {
                settings.support_interface_speed_mm_s * 60.0
            },
            bridge: if first {
                first_f
            } else {
                settings.bridge_speed_mm_s * 60.0
            },
            top: if first {
                first_f
            } else {
                settings.top_surface_speed_mm_s * 60.0
            },
        }
    }
}
