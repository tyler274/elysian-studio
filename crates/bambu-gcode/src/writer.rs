//! Orchestrate one G-code export: envelope, per-layer roles, cooling, placeholders.

use std::fmt::Write as _;

use bambu_config::{Flow, FlowRole, PrintAccel, SliceSettings, WallSequence};
use bambu_geom::{Polygon, Polyline};
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
        w.state.layer_contours = layer.contours.clone();
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
        w.state.layer_height_mm = layer.height_mm;
        w.state.layer_print_z_mm = layer.print_z_mm;
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
            "Prime tower",
            PrintAccel::Default,
            e(
                &layer.prime_tower,
                false,
                feeds.prime_tower,
                FlowRole::SupportMaterial,
                first,
            ),
        )?;
        if settings.support_filament > 0 && !layer.support.is_empty() {
            w.emit_toolchange(settings.support_filament)?;
        }
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
        if settings.support_interface_filament > 0 && !layer.support_interface.is_empty() {
            w.emit_toolchange(settings.support_interface_filament)?;
        }
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
        if !layer.support_ironing.is_empty() {
            // C++ `erSupportIroning` after support fills, before part ironing.
            let h = layer.height_mm * settings.support_ironing_flow.max(0.0);
            let flow = Flow::for_role(settings, FlowRole::SupportMaterial, h, first);
            let factor = settings.gcode_path_flow_factor(FlowRole::SupportMaterial, first);
            w.emit_role(
                "Support ironing",
                PrintAccel::Default,
                Extrude {
                    paths: &layer.support_ironing,
                    closed: false,
                    e_per_mm: flow.e_per_mm() * factor,
                    print_f: settings.support_ironing_speed_mm_s * 60.0,
                    mm3_per_mm: flow.mm3_per_mm() * factor,
                    width_mm: flow.width_mm,
                    arc_tolerance_mm: settings.arc_fit_tolerance_mm(FlowRole::SupportMaterial),
                },
            )?;
        }

        // C++ `is_infill_first && !first_layer`: infill before perimeters.
        // Ironing stays last (`extrude_infill(..., true)`).
        if w.state.current_tool.is_some() {
            w.emit_toolchange(settings.wall_filament.max(1))?;
        }
        let infill_first = settings.is_infill_first && !first;
        for walls_now in [!infill_first, infill_first] {
            if walls_now {
                let emit_outer = |w: &mut Writer<'_>, paths: &[Polyline]| {
                    w.set_print_role(PrintAccel::OuterWall);
                    let flow =
                        Flow::for_role(settings, FlowRole::ExternalPerimeter, flow_h, object_first);
                    let factor =
                        settings.gcode_path_flow_factor(FlowRole::ExternalPerimeter, first);
                    w.emit_wall_paths(
                        "Outer wall",
                        Extrude {
                            paths,
                            closed: true,
                            e_per_mm: flow.e_per_mm() * factor,
                            print_f: feeds.wall,
                            mm3_per_mm: flow.mm3_per_mm() * factor,
                            width_mm: flow.width_mm,
                            arc_tolerance_mm: settings
                                .arc_fit_tolerance_mm(FlowRole::ExternalPerimeter),
                        },
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
                let emit_sequence = |w: &mut Writer<'_>,
                                     outer: &[Polyline],
                                     inner: &[Polyline],
                                     gap: &[Polyline]|
                 -> Result<(), GcodeError> {
                    match settings.wall_sequence {
                        WallSequence::OuterInner => {
                            emit_outer(w, outer)?;
                            emit_inner(w, inner)?;
                        }
                        WallSequence::InnerOuter => {
                            emit_inner(w, inner)?;
                            emit_outer(w, outer)?;
                        }
                        WallSequence::InnerOuterInner => {
                            // C++ classic: children-first remaining inners, outer,
                            // then depth-1 (`elrSecondPerimeter`).
                            let n = inner.len().min(outer.len());
                            let (first_inner, remaining) = inner.split_at(n);
                            let remaining: Vec<_> = remaining.iter().rev().cloned().collect();
                            emit_inner(w, &remaining)?;
                            emit_outer(w, outer)?;
                            emit_inner(w, first_inner)?;
                        }
                    }
                    w.emit_role("Gap infill", PrintAccel::Default, {
                        let flow =
                            Flow::for_role(settings, FlowRole::Perimeter, flow_h, object_first);
                        let factor = settings.gcode_path_flow_factor(FlowRole::Perimeter, first);
                        Extrude {
                            paths: gap,
                            closed: false,
                            e_per_mm: flow.e_per_mm() * factor,
                            print_f: feeds.gap,
                            mm3_per_mm: flow.mm3_per_mm() * factor,
                            width_mm: flow.width_mm,
                            arc_tolerance_mm: settings.arc_fit_tolerance_mm(FlowRole::Perimeter),
                        }
                    })
                };
                let per_region = layer.region_settings.len() > 1
                    && layer.region_outer_walls.len() == layer.region_settings.len()
                    && layer.region_inner_walls.len() == layer.region_settings.len();
                if per_region {
                    for (r, cfg) in layer.region_settings.iter().enumerate() {
                        let outer = &layer.region_outer_walls[r];
                        let inner = &layer.region_inner_walls[r];
                        let gap = layer
                            .region_gap_infill
                            .get(r)
                            .map(Vec::as_slice)
                            .unwrap_or(&[]);
                        if outer.is_empty() && inner.is_empty() && gap.is_empty() {
                            continue;
                        }
                        w.emit_region_toolchange(cfg.wall_filament)?;
                        emit_sequence(&mut w, outer, inner, gap)?;
                    }
                } else {
                    emit_sequence(
                        &mut w,
                        &layer.outer_walls,
                        &layer.inner_walls,
                        &layer.gap_infill,
                    )?;
                }
            } else {
                let ctx = InfillCtx {
                    settings,
                    feeds: &feeds,
                    first,
                    object_first,
                    flow_h,
                };
                let per_region = layer.region_fills.len() > 1
                    && layer.region_fills.len() == layer.region_settings.len();
                if per_region {
                    for (r, cfg) in layer.region_settings.iter().enumerate() {
                        let fills = &layer.region_fills[r];
                        emit_infill(
                            &mut w,
                            &ctx,
                            InfillBundle {
                                sparse: &fills.sparse,
                                combined: &fills.combined,
                                combined_h: fills.combined_height_mm,
                                solid: &fills.solid,
                                floating: &fills.floating,
                                floating_areas: &fills.floating_areas,
                                bridge: &fills.bridge,
                                bottom: &fills.bottom,
                                top: &fills.top,
                                sparse_filament: cfg.sparse_infill_filament,
                                solid_filament: cfg.solid_infill_filament,
                            },
                        )?;
                    }
                } else {
                    emit_infill(
                        &mut w,
                        &ctx,
                        InfillBundle {
                            sparse: &layer.infill,
                            combined: &layer.combined_infill,
                            combined_h: layer.combined_infill_height_mm,
                            solid: &layer.solid_infill,
                            floating: &layer.floating_vertical_shell,
                            floating_areas: &layer.floating_areas,
                            bridge: &layer.bridge,
                            bottom: &layer.bottom_surface,
                            top: &layer.top_surface,
                            sparse_filament: settings.sparse_infill_filament,
                            solid_filament: settings.solid_infill_filament,
                        },
                    )?;
                }
            }
        }
        if !layer.ironing.is_empty() {
            w.emit_region_toolchange(settings.solid_infill_filament)?;
            w.set_print_role(PrintAccel::Default);
            let iron_flow =
                Flow::from_settings(settings, layer.height_mm * settings.ironing_flow.max(0.0));
            // C++ `erIroning` is not top solid, so first-layer ironing still
            // takes `print_flow_ratio * initial_layer_flow_ratio`.
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

struct InfillCtx<'a> {
    settings: &'a SliceSettings,
    feeds: &'a LayerFeeds,
    first: bool,
    object_first: bool,
    flow_h: f64,
}

struct InfillBundle<'a> {
    sparse: &'a [Polyline],
    combined: &'a [Polyline],
    combined_h: f64,
    solid: &'a [Polyline],
    floating: &'a [Polyline],
    floating_areas: &'a [Polygon],
    bridge: &'a [Polyline],
    bottom: &'a [Polyline],
    top: &'a [Polyline],
    sparse_filament: i32,
    solid_filament: i32,
}

fn emit_infill(
    w: &mut Writer<'_>,
    ctx: &InfillCtx<'_>,
    bundle: InfillBundle<'_>,
) -> Result<(), GcodeError> {
    let settings = ctx.settings;
    let feeds = ctx.feeds;
    let first = ctx.first;
    let object_first = ctx.object_first;
    let flow_h = ctx.flow_h;
    if !bundle.sparse.is_empty() || !bundle.combined.is_empty() {
        w.emit_region_toolchange(bundle.sparse_filament)?;
    }
    w.emit_role(
        "Sparse infill",
        PrintAccel::SparseInfill,
        infill_extrude(
            ctx,
            bundle.sparse,
            false,
            feeds.sparse,
            FlowRole::SparseInfill,
            object_first,
        ),
    )?;
    if !bundle.combined.is_empty() {
        let h = bundle.combined_h.max(flow_h);
        let flow = Flow::for_role(settings, FlowRole::SparseInfill, h, object_first);
        let factor = settings.gcode_path_flow_factor(FlowRole::SparseInfill, first);
        w.emit_role(
            "Sparse infill",
            PrintAccel::SparseInfill,
            Extrude {
                paths: bundle.combined,
                closed: false,
                e_per_mm: flow.e_per_mm() * factor,
                print_f: feeds.sparse,
                mm3_per_mm: flow.mm3_per_mm() * factor,
                width_mm: flow.width_mm,
                arc_tolerance_mm: settings.arc_fit_tolerance_mm(FlowRole::SparseInfill),
            },
        )?;
    }
    if !bundle.solid.is_empty()
        || !bundle.floating.is_empty()
        || !bundle.bridge.is_empty()
        || !bundle.bottom.is_empty()
        || !bundle.top.is_empty()
    {
        w.emit_region_toolchange(bundle.solid_filament)?;
    }
    w.emit_role(
        "Internal solid infill",
        PrintAccel::Default,
        infill_extrude(
            ctx,
            bundle.solid,
            false,
            feeds.solid,
            FlowRole::SolidInfill,
            object_first,
        ),
    )?;
    w.emit_floating_shell_paths(
        infill_extrude(
            ctx,
            bundle.floating,
            false,
            feeds.vertical_shell,
            FlowRole::SolidInfill,
            object_first,
        ),
        bundle.floating_areas,
        feeds.bridge,
        first,
    )?;
    if !bundle.bridge.is_empty() {
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
                    paths: bundle.bridge,
                    closed: false,
                    e_per_mm: bridge_flow.e_per_mm() * factor,
                    print_f: feeds.bridge,
                    mm3_per_mm: bridge_flow.mm3_per_mm() * factor,
                    width_mm: bridge_flow.width_mm,
                    arc_tolerance_mm: settings.arc_fit_tolerance_mm(FlowRole::SolidInfill),
                })
            },
        )?;
    }
    w.emit_role(
        "Bottom surface",
        PrintAccel::Default,
        infill_extrude(
            ctx,
            bundle.bottom,
            false,
            feeds.wall,
            FlowRole::SolidInfill,
            object_first,
        ),
    )?;
    w.emit_role(
        "Top surface",
        PrintAccel::TopSurface,
        infill_extrude(
            ctx,
            bundle.top,
            false,
            feeds.top,
            FlowRole::TopSolidInfill,
            object_first,
        ),
    )?;
    Ok(())
}

fn infill_extrude<'a>(
    ctx: &InfillCtx<'_>,
    paths: &'a [Polyline],
    closed: bool,
    print_f: f64,
    role: FlowRole,
    role_first: bool,
) -> Extrude<'a> {
    let flow = Flow::for_role(ctx.settings, role, ctx.flow_h, role_first);
    let factor = ctx.settings.gcode_path_flow_factor(role, ctx.first);
    Extrude {
        paths,
        closed,
        e_per_mm: flow.e_per_mm() * factor,
        print_f,
        mm3_per_mm: flow.mm3_per_mm() * factor,
        width_mm: flow.width_mm,
        arc_tolerance_mm: ctx.settings.arc_fit_tolerance_mm(role),
    }
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

    /// C++ `set_extruder` stand-in: retract, then `T{filament_map[id]}`.
    fn emit_toolchange(&mut self, filament_1based: i32) -> Result<(), GcodeError> {
        let t = self.settings.tool_command_for_filament(filament_1based);
        if self.state.current_tool == Some(t) {
            return Ok(());
        }
        self.retract()?;
        writeln!(self.out, "T{t}")?;
        self.state.current_tool = Some(t);
        Ok(())
    }

    /// Skip the first `T` when it matches the implicit start nozzle.
    fn emit_region_toolchange(&mut self, filament_1based: i32) -> Result<(), GcodeError> {
        let filament = filament_1based.max(1);
        let t = self.settings.tool_command_for_filament(filament);
        let implicit = self
            .settings
            .tool_command_for_filament(self.settings.wall_filament.max(1));
        if self.state.current_tool.is_none() && t == implicit {
            return Ok(());
        }
        self.emit_toolchange(filament)
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
    prime_tower: f64,
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
            prime_tower: if first {
                first_f
            } else {
                settings.prime_tower_max_speed_mm_s * 60.0
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
