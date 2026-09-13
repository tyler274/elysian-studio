//! Sidebar widgets (profiles, paint, account, monitor). `App` / `Message` stay in `main`.

use iced::widget::{
    button, checkbox, column, pick_list, row, scrollable, slider, text, text_input,
};
use iced::{Element, Fill};

use crate::{format_eta, role_legend, Message, SendVia};

impl crate::App {
    pub(crate) fn sidebar(&self) -> Element<'_, Message> {
        let max_layer = self.scene.toolpaths.layer_zs.len().saturating_sub(1) as f64;
        let max_move = self.scene.toolpaths.vertices.len().saturating_sub(1) as f64;
        let plate_row = self.plate_buttons();
        let process_names: Vec<String> = self
            .process_profiles
            .iter()
            .map(|p| p.name.clone())
            .collect();
        let filament_names: Vec<String> = self
            .filament_profiles
            .iter()
            .map(|p| p.name.clone())
            .collect();
        let machine_names: Vec<String> = self
            .machine_profiles
            .iter()
            .map(|p| p.name.clone())
            .collect();
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
        let device_labels: Vec<String> = self.cloud_devices.iter().map(|d| d.label()).collect();
        let selected_device = self
            .selected_device
            .as_ref()
            .and_then(|id| self.cloud_devices.iter().find(|d| &d.dev_id == id))
            .map(|d| d.label());
        scrollable(
            column![
                text("Bambu Studio").size(22),
                text("Rust rewrite · iced + wgpu").size(14),
                text(format!("GPU: {}", self.adapter)).size(13),
                text("Profiles").size(16),
                pick_list(
                    process_names,
                    self.process_name.clone(),
                    Message::ProcessProfile
                )
                .placeholder("process JSON"),
                pick_list(
                    filament_names,
                    self.filament_name.clone(),
                    Message::FilamentProfile
                )
                .placeholder("filament JSON"),
                pick_list(
                    machine_names,
                    self.machine_name.clone(),
                    Message::MachineProfile
                )
                .placeholder("machine JSON"),
                text(format!("Bed {:.0} mm", self.scene.bed_mm)).size(12),
                text("Process").size(16),
                row![
                    button("-").on_press(Message::WallLoops(
                        self.settings.wall_loops.saturating_sub(1).max(1)
                    )),
                    text(format!("Walls {}", self.settings.wall_loops)).size(13),
                    button("+")
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
                text("Filament").size(16),
                text(format!(
                    "{} {} · {}°C · {} tool(s)",
                    self.settings.filament_vendor,
                    self.settings.filament_type,
                    self.settings.temperature_c,
                    self.settings.filament_count.max(1)
                ))
                .size(13),
                text(format!("Nozzle {}°C", self.settings.temperature_c)).size(13),
                slider(
                    180.0..=280.0,
                    f64::from(self.settings.temperature_c),
                    Message::NozzleTemp,
                )
                .step(1.0),
                text("Printer").size(16),
                text(format!(
                    "{} · {}",
                    self.settings.printer_structure, self.settings.curr_bed_type
                ))
                .size(13),
                checkbox(self.by_object)
                    .label("By-object sequence")
                    .on_toggle(Message::ByObject),
                text("Plates").size(16),
                plate_row,
                text("Objects").size(16),
                self.object_panel(),
                text("Preview / G-code scrubber").size(16),
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
                text("Paint (click triangle)").size(16),
                checkbox(self.paint_blocker)
                    .label("Blocker (else Enforcer)")
                    .on_toggle(Message::PaintBlocker),
                text(format!("Brush {:.1} mm", self.brush_mm)).size(12),
                slider(0.5..=12.0, f64::from(self.brush_mm), |v| {
                    Message::BrushRadius(v as f32)
                })
                .step(0.5),
                button("Paint support").on_press(Message::PaintSupport),
                button("Paint seam").on_press(Message::PaintSeam),
                button("Paint fuzzy").on_press(Message::PaintFuzzy),
                button("Paint off").on_press(Message::PaintClear),
                button("Open model").on_press(Message::OpenModel),
                button("Calibration block").on_press(Message::Calibration),
                button("Slice").on_press(Message::Slice),
                button("Reset camera").on_press(Message::ResetCamera),
                text("Account").size(16),
                text(format!(
                    "region {} · user {}",
                    if self.cloud_region.is_empty() {
                        "us"
                    } else {
                        self.cloud_region.as_str()
                    },
                    if self.cloud_user.is_empty() {
                        "—"
                    } else {
                        self.cloud_user.as_str()
                    }
                ))
                .size(12),
                text(format!(
                    "Bearer {}",
                    if self.has_bearer {
                        "present"
                    } else {
                        "missing"
                    }
                ))
                .size(12),
                button("Import Studio").on_press(Message::ImportStudio),
                button("Refresh devices").on_press(Message::RefreshDevices),
                pick_list(device_labels, selected_device, Message::PickDevice)
                    .placeholder("cloud device"),
                pick_list(
                    vec![SendVia::LanFtps, SendVia::CloudUpload],
                    Some(self.send_via),
                    Message::SendVia
                ),
                button("Extract keys").on_press(Message::ExtractKeys),
                button("Discover printers").on_press(Message::Discover),
                text_input("printer IP", &self.host).on_input(Message::Host),
                text_input("LAN access code", &self.access_code)
                    .secure(true)
                    .on_input(Message::AccessCode),
                text_input("serial (optional)", &self.serial).on_input(Message::Serial),
                self.login_fields(),
                checkbox(self.live_monitor)
                    .label("Live monitor")
                    .on_toggle(Message::LiveMonitor),
                button("MQTT / AMS status").on_press(Message::RefreshStatus),
                self.monitor_controls(),
                text("HMS").size(16),
                button("Refresh HMS catalog").on_press(Message::RefreshHms),
                text(if self.hms_lines.is_empty() {
                    "no HMS".into()
                } else {
                    self.hms_lines.join("\n")
                })
                .size(11),
                button("Send last slice").on_press(Message::Send),
                button("Chamber / RTSPS live").on_press(Message::Chamber),
                text(&self.status).size(13),
                text("Drag: orbit · Scroll: zoom · Click: paint").size(12),
            ]
            .spacing(8)
            .padding(16)
            .width(320),
        )
        .height(Fill)
        .into()
    }

    fn login_fields(&self) -> Element<'_, Message> {
        if self.has_bearer {
            return text("cloud token on disk").size(11).into();
        }
        column![
            text("Email login (no Bearer)").size(13),
            text_input("account email", &self.login_account).on_input(Message::LoginAccount),
            text_input("password", &self.login_password)
                .secure(true)
                .on_input(Message::LoginPassword),
            text_input("email code (if asked)", &self.login_code)
                .secure(true)
                .on_input(Message::LoginCode),
            button("Cloud login").on_press(Message::CloudLogin),
        ]
        .spacing(6)
        .into()
    }
}
