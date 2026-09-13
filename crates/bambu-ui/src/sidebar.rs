//! Prepare / Preview left panes. `App` / `Message` stay in `main`.

use iced::widget::{button, checkbox, column, pick_list, row, scrollable, slider, text};
use iced::{Element, Fill};

use crate::{format_eta, role_legend, Message, SIDEBAR_WIDTH};

impl crate::App {
    pub(crate) fn prepare_sidebar(&self) -> Element<'_, Message> {
        let plate_row = self.plate_buttons();
        let process_names: Vec<String> = self
            .process_profiles
            .iter()
            .map(|p| p.name.clone())
            .collect();
        let machine_names: Vec<String> = self
            .machine_profiles
            .iter()
            .map(|p| p.name.clone())
            .collect();
        scrollable(
            column![
                text("Prepare").size(18),
                text(format!("GPU: {}", self.adapter)).size(12),
                text("Printer / process").size(16),
                pick_list(
                    process_names,
                    self.process_name.clone(),
                    Message::ProcessProfile
                )
                .placeholder("process JSON")
                .padding([3, 8])
                .text_size(13),
                pick_list(
                    machine_names,
                    self.machine_name.clone(),
                    Message::MachineProfile
                )
                .placeholder("machine JSON")
                .padding([3, 8])
                .text_size(13),
                text(format!(
                    "Bed {:.0}×{:.0} mm",
                    self.scene.bed.width(),
                    self.scene.bed.height()
                ))
                .size(12),
                text(format!(
                    "{} · {}",
                    self.settings.printer_structure, self.settings.curr_bed_type
                ))
                .size(12),
                row![
                    button("-").padding([3, 8]).on_press(Message::WallLoops(
                        self.settings.wall_loops.saturating_sub(1).max(1)
                    )),
                    text(format!("Walls {}", self.settings.wall_loops)).size(13),
                    button("+")
                        .padding([3, 8])
                        .on_press(Message::WallLoops((self.settings.wall_loops + 1).min(10))),
                ]
                .spacing(6),
                text(format!("Layer {:.2} mm", self.settings.layer_height_mm)).size(13),
                slider(
                    0.08..=0.32,
                    self.settings.layer_height_mm,
                    Message::LayerHeight
                )
                .step(0.01),
                text(format!(
                    "Infill {:.0}% {}",
                    self.settings.infill_density * 100.0,
                    self.settings.infill_pattern.as_str()
                ))
                .size(13),
                slider(0.0..=1.0, self.settings.infill_density, Message::Infill).step(0.01),
                checkbox(self.settings.enable_support)
                    .label("Supports")
                    .on_toggle(Message::EnableSupport),
                text(format!("Nozzle {}°C", self.settings.temperature_c)).size(13),
                slider(
                    180.0..=280.0,
                    f64::from(self.settings.temperature_c),
                    Message::NozzleTemp,
                )
                .step(1.0),
                checkbox(self.by_object)
                    .label("By-object sequence")
                    .on_toggle(Message::ByObject),
                self.filament_library(),
                text("Plates").size(16),
                plate_row,
                text("Objects").size(16),
                self.object_panel(),
                self.transform_panel(),
                text("Paint (click triangle)").size(16),
                checkbox(self.paint_blocker)
                    .label("Blocker (else Enforcer)")
                    .on_toggle(Message::PaintBlocker),
                text(format!("Brush {:.1} mm", self.brush_mm)).size(12),
                slider(0.5..=12.0, f64::from(self.brush_mm), |v| {
                    Message::BrushRadius(v as f32)
                })
                .step(0.5),
                button("Paint support")
                    .padding([3, 8])
                    .on_press(Message::PaintSupport),
                button("Paint seam")
                    .padding([3, 8])
                    .on_press(Message::PaintSeam),
                button("Paint fuzzy")
                    .padding([3, 8])
                    .on_press(Message::PaintFuzzy),
                button("Paint off")
                    .padding([3, 8])
                    .on_press(Message::PaintClear),
                button("Calibration block")
                    .padding([3, 8])
                    .on_press(Message::Calibration),
                button("Reset camera")
                    .padding([3, 8])
                    .on_press(Message::ResetCamera),
                text("Right-drag: orbit · Middle-drag: pan · Scroll: zoom · Left: tool / paint")
                    .size(12),
            ]
            .spacing(6)
            .padding([10, 12])
            .width(SIDEBAR_WIDTH),
        )
        .height(Fill)
        .into()
    }

    pub(crate) fn preview_sidebar(&self) -> Element<'_, Message> {
        let max_layer = self.scene.toolpaths.layer_zs.len().saturating_sub(1) as f64;
        let max_move = self.scene.toolpaths.vertices.len().saturating_sub(1) as f64;
        let preview_z = self.scene.preview_z();
        let layer_frac = if max_layer <= 0.0 {
            1.0
        } else {
            f64::from(self.scene.preview_layer) / max_layer
        };
        let eta = self
            .estimated_seconds
            .map(|s| format_eta(s * layer_frac))
            .unwrap_or_else(|| "—".into());
        scrollable(
            column![
                text("Preview").size(18),
                text(format!(
                    "Layer {}  Z {:.2} mm  ~{eta}",
                    self.scene.preview_layer + 1,
                    if preview_z.is_finite() {
                        preview_z
                    } else {
                        0.0
                    }
                ))
                .size(12),
                slider(
                    0.0..=max_layer.max(1.0),
                    f64::from(self.scene.preview_layer),
                    |v| Message::PreviewLayer(v as u32)
                )
                .step(1.0),
                slider(
                    0.0..=max_move.max(1.0),
                    f64::from(self.scene.preview_vertices),
                    |v| Message::PreviewMove(v as u32)
                )
                .step(1.0),
                text(role_legend()).size(11),
                checkbox(self.scene.hide_infill)
                    .label("Hide infill")
                    .on_toggle(Message::HideInfill),
                checkbox(self.scene.hide_support)
                    .label("Hide support")
                    .on_toggle(Message::HideSupport),
                button("Reset camera")
                    .padding([3, 8])
                    .on_press(Message::ResetCamera),
                text("Slice from the top bar, then scrub layers.").size(12),
            ]
            .spacing(6)
            .padding([10, 12])
            .width(SIDEBAR_WIDTH),
        )
        .height(Fill)
        .into()
    }
}
