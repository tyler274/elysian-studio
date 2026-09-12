//! Orchestrate one G-code export: envelope, per-layer roles, cooling, placeholders.

use std::fmt::Write as _;

use bambu_config::{Flow, FlowRole, PrintAccel, SliceSettings, WallSequence};
use bambu_geom::Polyline;
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
        w.state.internal_islands = layer.infill_region.clone();
        w.state.support_islands = layer.support_region.clone();
        w.state.wall_paths = layer
            .outer_walls
            .iter()
            .chain(layer.inner_walls.iter())
            .cloned()
            .collect();
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

        let object_first = layer_i == settings.raft_layers as usize;
        let flow_h = layer.height_mm;
        let feeds = LayerFeeds::for_layer(settings, first);
        let support_polys = if layer_i == 0 {
            None
        } else {
            overhang_rings(settings, sliced.layers.get(layer_i - 1))
        };
        let e = |paths, closed, print_f, role, role_first| {
            let flow = Flow::for_role(settings, role, flow_h, role_first);
            let factor = settings.gcode_path_flow_factor(role, first);
            Extrude {
                paths,
                closed,
                e_per_mm: flow.e_per_mm() * factor,
                print_f,
                mm3_per_mm: flow.mm3_per_mm() * factor,
                width_mm: flow.width_mm,
                arc_tolerance_mm: settings.arc_fit_tolerance_mm(role),
            }
        };

        w.emit_role(
            "Skirt",
            PrintAccel::Default,
            e(
                &layer.skirt,
                true,
                feeds.wall,
                FlowRole::ExternalPerimeter,
                // C++ `skirt_flow()` keeps first-layer width so loops stay aligned.
                true,
            ),
        )?;
        w.emit_role(
            "Brim",
            PrintAccel::Default,
            e(
                &layer.brim,
                true,
                feeds.wall,
                FlowRole::ExternalPerimeter,
                first,
            ),
        )?;
        w.emit_role(
            "Support",
            PrintAccel::Default,
            e(
                &layer.support,
                false,
                feeds.support,
                FlowRole::SupportMaterial,
                first,
            ),
        )?;
        w.emit_role(
            "Support interface",
            PrintAccel::Default,
            e(
                &layer.support_interface,
                false,
                feeds.support_interface,
                FlowRole::SupportMaterial,
                first,
            ),
        )?;

        // C++ `is_infill_first && !first_layer`: infill before perimeters.
        // Ironing stays last (`extrude_infill(..., true)`).
        let infill_first = settings.is_infill_first && !first;
        for walls_now in [!infill_first, infill_first] {
            if walls_now {
                let emit_outer = |w: &mut Writer<'_>| {
                    w.set_print_role(PrintAccel::OuterWall);
                    w.emit_wall_paths(
                        "Outer wall",
                        e(
                            &layer.outer_walls,
                            true,
                            feeds.wall,
                            FlowRole::ExternalPerimeter,
                            object_first,
                        ),
                        support_polys.as_deref(),
                        settings.enable_overhang_speed,
                        !first,
                    )
                };
                let emit_inner = |w: &mut Writer<'_>, paths: &[Polyline]| {
                    w.set_print_role(PrintAccel::InnerWall);
                    let flow = Flow::for_role(settings, FlowRole::Perimeter, flow_h, object_first);
                    let factor = settings.gcode_path_flow_factor(FlowRole::Perimeter, first);
                    w.emit_wall_paths(
                        "Inner wall",
                        Extrude {
                            paths,
                            closed: true,
                            e_per_mm: flow.e_per_mm() * factor,
                            print_f: feeds.inner,
                            mm3_per_mm: flow.mm3_per_mm() * factor,
                            width_mm: flow.width_mm,
                            arc_tolerance_mm: settings.arc_fit_tolerance_mm(FlowRole::Perimeter),
                        },
                        support_polys.as_deref(),
                        settings.enable_overhang_speed,
                        !first,
                    )
                };
                match settings.wall_sequence {
                    WallSequence::OuterInner => {
                        emit_outer(&mut w)?;
                        emit_inner(&mut w, &layer.inner_walls)?;
                    }
                    WallSequence::InnerOuter => {
                        emit_inner(&mut w, &layer.inner_walls)?;
                        emit_outer(&mut w)?;
                    }
                    WallSequence::InnerOuterInner => {
                        // C++ classic: children-first remaining inners, outer,
                        // then depth-1 (`elrSecondPerimeter`).
                        let n = layer.inner_walls.len().min(layer.outer_walls.len());
                        let (first_inner, remaining) = layer.inner_walls.split_at(n);
                        let remaining: Vec<_> = remaining.iter().rev().cloned().collect();
                        emit_inner(&mut w, &remaining)?;
                        emit_outer(&mut w)?;
                        emit_inner(&mut w, first_inner)?;
                    }
                }
                w.emit_role(
                    "Gap infill",
                    PrintAccel::Default,
                    e(
                        &layer.gap_infill,
                        false,
                        feeds.gap,
                        FlowRole::Perimeter,
                        object_first,
                    ),
                )?;
            } else {
                w.emit_role(
                    "Sparse infill",
                    PrintAccel::SparseInfill,
                    e(
                        &layer.infill,
                        false,
                        feeds.sparse,
                        FlowRole::SparseInfill,
                        object_first,
                    ),
                )?;
                if !layer.combined_infill.is_empty() {
                    let h = layer.combined_infill_height_mm.max(flow_h);
                    let flow = Flow::for_role(settings, FlowRole::SparseInfill, h, object_first);
                    let factor = settings.gcode_path_flow_factor(FlowRole::SparseInfill, first);
                    w.emit_role(
                        "Sparse infill",
                        PrintAccel::SparseInfill,
                        Extrude {
                            paths: &layer.combined_infill,
                            closed: false,
                            e_per_mm: flow.e_per_mm() * factor,
                            print_f: feeds.sparse,
                            mm3_per_mm: flow.mm3_per_mm() * factor,
                            width_mm: flow.width_mm,
                            arc_tolerance_mm: settings.arc_fit_tolerance_mm(FlowRole::SparseInfill),
                        },
                    )?;
                }
                w.emit_role(
                    "Internal solid infill",
                    PrintAccel::Default,
                    e(
                        &layer.solid_infill,
                        false,
                        feeds.solid,
                        FlowRole::SolidInfill,
                        object_first,
                    ),
                )?;
                w.emit_floating_shell_paths(
                    e(
                        &layer.floating_vertical_shell,
                        false,
                        feeds.vertical_shell,
                        FlowRole::SolidInfill,
                        object_first,
                    ),
                    &layer.floating_areas,
                    feeds.bridge,
                    first,
                )?;
                if !layer.bridge.is_empty() {
                    let bridge_flow = Flow::bridging_flow(
                        settings,
                        FlowRole::SolidInfill,
                        flow_h,
                        object_first,
                        settings.thick_bridges,
                    );
                    let factor = settings.gcode_path_flow_factor(FlowRole::SolidInfill, first);
                    w.emit_feature("Bridge", bridge_flow.width_mm)?;
                    w.set_print_role(PrintAccel::Default);
                    w.emit_marked(
                        settings.overhang_fan_applies(5, true, false),
                        ";_OVERHANG_FAN_START",
                        ";_OVERHANG_FAN_END",
                        |w| {
                            w.emit_paths(Extrude {
                                paths: &layer.bridge,
                                closed: false,
                                e_per_mm: bridge_flow.e_per_mm() * factor,
                                print_f: feeds.bridge,
                                mm3_per_mm: bridge_flow.mm3_per_mm() * factor,
                                width_mm: bridge_flow.width_mm,
                                arc_tolerance_mm: settings
                                    .arc_fit_tolerance_mm(FlowRole::SolidInfill),
                            })
                        },
                    )?;
                }
                w.emit_role(
                    "Bottom surface",
                    PrintAccel::Default,
                    e(
                        &layer.bottom_surface,
                        false,
                        feeds.wall,
                        FlowRole::SolidInfill,
                        object_first,
                    ),
                )?;
                w.emit_role(
                    "Top surface",
                    PrintAccel::TopSurface,
                    e(
                        &layer.top_surface,
                        false,
                        feeds.top,
                        FlowRole::TopSolidInfill,
                        object_first,
                    ),
                )?;
            }
        }
        if !layer.ironing.is_empty() {
            w.set_print_role(PrintAccel::Default);
            let iron_flow =
                Flow::from_settings(settings, layer.height_mm * settings.ironing_flow.max(0.0));
            // C++ `erIroning` is not top solid, so first-layer ironing still
            // takes `initial_layer_flow_ratio`.
            let factor = settings.gcode_path_flow_factor(FlowRole::SolidInfill, first);
            w.emit_feature("Ironing", iron_flow.width_mm)?;
            w.emit_marked(
                settings.ironing_fan_speed >= 0,
                ";_IRONING_FAN_START",
                ";_IRONING_FAN_END",
                |w| {
                    w.emit_paths(Extrude {
                        paths: &layer.ironing,
                        // C++ ironing is `ExtrusionPath`, not a closed loop.
                        closed: false,
                        e_per_mm: iron_flow.e_per_mm() * factor,
                        print_f: settings.ironing_speed_mm_s * 60.0,
                        mm3_per_mm: iron_flow.mm3_per_mm() * factor,
                        width_mm: iron_flow.width_mm,
                        arc_tolerance_mm: settings.arc_fit_tolerance_mm(FlowRole::TopSolidInfill),
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
    pub(crate) fn emit_feature(&mut self, feature: &str, width: f64) -> Result<(), GcodeError> {
        writeln!(self.out, "; FEATURE: {feature}")?;
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
        self.emit_feature(feature, job.width_mm)?;
        self.set_print_role(role);
        self.state.dest_is_support = feature == "Support";
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
