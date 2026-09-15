//! Device StatusPanel: camera stream, AMS trays, task / temps, HMS, account.

use iced::widget::{
    button, checkbox, column, container, image, pick_list, progress_bar, row, scrollable, slider,
    text, text_input, Space,
};
use iced::{Alignment, Background, Border, Color, ContentFit, Element, Fill};

use bambu_device::{
    AmsState, AmsTray, AmsUnit, MachineState, NozzleRackState, NozzleSlot, PrinterBackend,
};
use bambu_protocol::{
    capture_chamber, cloud_error_is_rate_limited, default_config_dir, describe_hms, jpeg_to_frame,
    load_cached_catalog, load_cloud_session, save_cloud_session, stream_rtsps_frames,
    stream_ttcode_frames, ChamberCapture, CloudApi, CloudBackend, CloudSession, JpegStream,
};

use crate::theme;
use crate::{lan_from, ChamberResult, Message, MonitorSnapshot, PrintCmd, SendVia};

fn quiet_btn<'a>(content: impl Into<Element<'a, Message>>) -> button::Button<'a, Message> {
    button(content)
        .padding([4, 10])
        .style(|_, status| theme::quiet(status))
}

fn field<'a>(
    placeholder: &'a str,
    value: &str,
    msg: fn(String) -> Message,
) -> text_input::TextInput<'a, Message> {
    text_input(placeholder, value)
        .on_input(msg)
        .style(theme::field)
}

pub(crate) fn parse_tray_color(raw: &str) -> Color {
    let hex: String = raw
        .trim()
        .trim_start_matches('#')
        .chars()
        .filter(|c| c.is_ascii_hexdigit())
        .take(6)
        .collect();
    if hex.len() < 6 {
        return Color::from_rgb(0.25, 0.26, 0.28);
    }
    let n = u32::from_str_radix(&hex, 16).unwrap_or(0);
    Color::from_rgb8(
        ((n >> 16) & 0xFF) as u8,
        ((n >> 8) & 0xFF) as u8,
        (n & 0xFF) as u8,
    )
}

pub(crate) const CAMERA_CLOUD_DISCOVER: &str =
    "Cloud MQTT has no JPEG tunnel — discovering LAN IP for :6000…";
pub(crate) const CAMERA_CLOUD_NEED_LAN: &str =
    "Need printer IP + LAN access code (same Wi‑Fi) or a cloud serial for TUTK/Agora liveview.";
pub(crate) const CAMERA_CLOUD_TUTK: &str =
    "connecting camera (LAN JPEG :6000 / RTSPS :322, else cloud TUTK/Agora)…";

pub(crate) fn lan_ready(host: &str, code: &str) -> bool {
    !host.is_empty() && !code.is_empty()
}

impl crate::App {
    pub(crate) fn can_monitor(&self) -> bool {
        lan_ready(&self.host, &self.access_code) || self.has_bearer
    }

    pub(crate) fn ams_chips(&self) -> Element<'_, Message> {
        let mut r = row![];
        if self.settings.filament_map.is_empty() {
            r = r.push(
                text("Filament mapping: —")
                    .size(11)
                    .color(theme::TEXT_MUTED),
            );
        }
        for (i, mapped) in self.settings.filament_map.iter().enumerate() {
            r = r.push(
                quiet_btn(text(format!("F{}→T{mapped}", i + 1)).size(11))
                    .on_press(Message::CycleAmsMap(i)),
            );
        }
        r.spacing(4).into()
    }

    pub(crate) fn ams_load_row(&self) -> Element<'_, Message> {
        let mut r = row![];
        if self.ams.trays.is_empty() && self.ams.vt_tray.is_none() {
            r = r.push(text("Load / Unload: —").size(11).color(theme::TEXT_MUTED));
        }
        let mut unloaded = Vec::new();
        for tray in &self.ams.trays {
            let ams_id = tray.ams_id;
            let slot_id = tray.id;
            r = r.push(
                quiet_btn(text(format!("Load T{slot_id}")).size(11))
                    .on_press(Message::AmsLoad { ams_id, slot_id }),
            );
            if !unloaded.contains(&ams_id) {
                unloaded.push(ams_id);
                r = r.push(
                    quiet_btn(text("Unload").size(11)).on_press(Message::AmsUnload { ams_id }),
                );
            }
        }
        if let Some(vt) = &self.ams.vt_tray {
            r = r.push(quiet_btn(text("Load External Spool").size(11)).on_press(
                Message::AmsLoad {
                    ams_id: vt.ams_id,
                    slot_id: 0,
                },
            ));
        }
        r.spacing(4).wrap().into()
    }

    pub(crate) fn monitor_line(&self) -> String {
        if self.mqtt_status.is_empty() {
            return "AMS: —".into();
        }
        let humidity = self
            .ams
            .humidity
            .map(|h| format!(" RH{h}"))
            .unwrap_or_default();
        format!(
            "{} · {}% · L{}/{} · {}m · nozzle {:.0}/{:.0}°C · bed {:.0}/{:.0}°C · wifi {} · spd {}{humidity}",
            if self.machine.gcode_state.is_empty() {
                "—"
            } else {
                self.machine.gcode_state.as_str()
            },
            self.machine.mc_percent,
            self.machine.layer_num,
            self.machine.total_layer_num,
            self.machine.mc_remaining_time_min,
            self.machine.nozzle_temp_c,
            self.machine.nozzle_target_c,
            self.machine.bed_temp_c,
            self.machine.bed_target_c,
            if self.machine.wifi_signal.is_empty() {
                "—"
            } else {
                self.machine.wifi_signal.as_str()
            },
            self.machine.spd_lvl
        )
    }

    fn camera_pane(&self) -> Element<'_, Message> {
        let body: Element<'_, Message> = if let Some(handle) = &self.chamber_handle {
            image(handle.clone())
                .width(Fill)
                .height(360)
                .content_fit(ContentFit::Contain)
                .into()
        } else if !self.camera_note.is_empty() {
            text(self.camera_note.as_str())
                .size(12)
                .color(theme::TEXT_MUTED)
                .into()
        } else {
            text("P1/A1 JPEG :6000 on LAN, or cloud TUTK. X1/H2: LAN RTSPS :322, else cloud Agora RTC.")
                .size(12)
                .color(theme::TEXT_MUTED)
                .into()
        };
        let size = if self.chamber_width > 0 {
            let tag = if self.camera_live { "live" } else { "paused" };
            format!("{}×{} {tag}", self.chamber_width, self.chamber_height)
        } else if self.camera_live {
            "connecting…".into()
        } else {
            "no frame".into()
        };
        card(
            "Camera",
            column![
                container(body)
                    .width(Fill)
                    .height(360)
                    .padding(8)
                    .style(|_| container::Style {
                        background: Some(Background::Color(theme::HEADER)),
                        border: Border {
                            color: theme::CARD_BORDER,
                            width: 1.0,
                            radius: 4.0.into(),
                        },
                        ..container::Style::default()
                    }),
                row![
                    text(size).size(11).color(theme::TEXT_MUTED),
                    Space::new().width(Fill),
                    quiet_btn(text("Play").size(12)).on_press(Message::CameraPlay),
                    quiet_btn(text("Stop").size(12)).on_press(Message::CameraStop),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            ]
            .spacing(8),
        )
    }

    fn task_pane(&self) -> Element<'_, Message> {
        let state = if self.machine.gcode_state.is_empty() {
            "IDLE"
        } else {
            self.machine.gcode_state.as_str()
        };
        let file = if self.machine.gcode_file.is_empty() {
            "—"
        } else {
            self.machine.gcode_file.as_str()
        };
        let remain = if self.machine.mc_remaining_time_min >= 60 {
            format!(
                "{}h {}m",
                self.machine.mc_remaining_time_min / 60,
                self.machine.mc_remaining_time_min % 60
            )
        } else {
            format!("{}m", self.machine.mc_remaining_time_min)
        };
        card(
            "Task",
            column![
                text(format!("{state} · {file}")).size(14),
                progress_bar(0.0..=100.0, f32::from(self.machine.mc_percent)).girth(8),
                text(format!(
                    "{}% · layer {}/{} · remaining {remain}",
                    self.machine.mc_percent, self.machine.layer_num, self.machine.total_layer_num
                ))
                .size(12)
                .color(theme::TEXT_MUTED),
                row![
                    temp_chip(
                        "Nozzle",
                        self.machine.nozzle_temp_c,
                        self.machine.nozzle_target_c,
                    ),
                    temp_chip("Bed", self.machine.bed_temp_c, self.machine.bed_target_c),
                    temp_chip("Chamber", self.machine.chamber_temp_c, 0.0),
                ]
                .spacing(8),
                text(format!(
                    "wifi {} · speed {} · fan {}",
                    if self.machine.wifi_signal.is_empty() {
                        "—"
                    } else {
                        self.machine.wifi_signal.as_str()
                    },
                    self.machine.spd_lvl,
                    self.control_fan
                ))
                .size(12)
                .color(theme::TEXT_MUTED),
            ]
            .spacing(8),
        )
    }

    fn controls_pane(&self) -> Element<'_, Message> {
        let stop_row: Element<'_, Message> = if self.stop_confirm {
            column![
                text("Are you sure you want to stop this print?")
                    .size(12)
                    .color(theme::TEXT),
                row![
                    quiet_btn("No").on_press(Message::StopCancel),
                    quiet_btn("Stop").on_press(Message::StopConfirm),
                ]
                .spacing(6),
            ]
            .spacing(4)
            .into()
        } else {
            row![
                quiet_btn("Pause").on_press(Message::Pause),
                quiet_btn("Resume").on_press(Message::Resume),
                quiet_btn("Stop").on_press(Message::Stop),
            ]
            .spacing(6)
            .into()
        };
        card(
            "Controls",
            column![
                stop_row,
                text("Print speed").size(12).color(theme::TEXT_MUTED),
                row![
                    quiet_btn("1").on_press(Message::PrintSpeed(1)),
                    quiet_btn("2").on_press(Message::PrintSpeed(2)),
                    quiet_btn("3").on_press(Message::PrintSpeed(3)),
                    quiet_btn("4").on_press(Message::PrintSpeed(4)),
                ]
                .spacing(4),
                row![
                    quiet_btn("Light on").on_press(Message::ChamberLight(true)),
                    quiet_btn("Light off").on_press(Message::ChamberLight(false)),
                ]
                .spacing(6),
                text("Bed / nozzle °C").size(12).color(theme::TEXT_MUTED),
                row![
                    field("bed", &self.control_bed, Message::BedSet),
                    quiet_btn("Set bed").on_press(Message::SendBed),
                ]
                .spacing(4),
                row![
                    field("nozzle", &self.control_nozzle, Message::NozzleSet),
                    quiet_btn("Set nozzle").on_press(Message::SendNozzle),
                ]
                .spacing(4),
                text(format!("Cooling fan {}", self.control_fan))
                    .size(12)
                    .color(theme::TEXT_MUTED),
                slider(0.0..=255.0, f64::from(self.control_fan), Message::FanSet)
                    .step(1.0)
                    .style(theme::range),
                quiet_btn("Set fan").on_press(Message::SendFan),
            ]
            .spacing(8),
        )
    }

    pub(crate) fn ams_pane(&self) -> Element<'_, Message> {
        let units = if self.ams.units.is_empty()
            && (!self.ams.trays.is_empty() || self.ams.humidity.is_some())
        {
            vec![AmsUnit {
                id: 0,
                humidity: self.ams.humidity,
                humidity_percent: None,
                temp: self.ams.unit_temp,
                dry_time_min: None,
                dry_status: 0,
                ..Default::default()
            }]
        } else {
            self.ams.units.clone()
        };
        let mut body = column![].spacing(10);
        if units.is_empty() && self.ams.vt_tray.is_none() {
            body = body.push(
                text("AMS has not been initialized. Please initialize it before use.")
                    .size(12)
                    .color(theme::TEXT_MUTED),
            );
        }
        for unit in &units {
            body = body.push(self.ams_unit_card(unit));
        }
        if let Some(vt) = &self.ams.vt_tray {
            body = body.push(tray_card(vt, false));
        }
        card(
            "AMS",
            column![body, self.ams_chips(), self.ams_load_row()].spacing(8),
        )
    }

    fn ams_unit_card(&self, unit: &AmsUnit) -> Element<'_, Message> {
        let trays: Vec<_> = self
            .ams
            .trays
            .iter()
            .filter(|t| t.ams_id == unit.id)
            .collect();
        let mut tray_row = row![].spacing(8);
        if trays.is_empty() {
            tray_row = tray_row.push(text("no trays").size(11).color(theme::TEXT_MUTED));
        }
        for tray in &trays {
            let active = self.ams.active_slot == Some(tray.id) && trays.len() <= 4;
            tray_row = tray_row.push(tray_card(tray, active));
        }
        let summary = ams_unit_summary(unit);
        let open = self.dry_ams == Some(unit.id);
        let mut col = column![
            row![
                humidity_icon(unit.humidity, unit.is_drying()),
                column![
                    text(unit.display_name()).size(13),
                    quiet_btn(text(summary).size(11)).on_press(Message::AmsDryToggle(unit.id)),
                ]
                .spacing(2),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
            tray_row.wrap(),
        ]
        .spacing(8);
        if open {
            col = col.push(
                self.ams_dry_popup(
                    unit,
                    trays
                        .first()
                        .map(|t| t.filament_type.as_str())
                        .unwrap_or("PLA"),
                ),
            );
        }
        container(col)
            .padding(8)
            .width(Fill)
            .style(|_| theme::chip())
            .into()
    }

    fn ams_dry_popup(&self, unit: &AmsUnit, filament: &str) -> Element<'_, Message> {
        let remain = unit.dry_time_min.map(|m| {
            if m >= 60 {
                format!("{}h {}m", m / 60, m % 60)
            } else {
                format!("{m} min")
            }
        });
        let humidity = match (unit.humidity, unit.humidity_percent) {
            (_, Some(p)) => format!("{p}%"),
            (Some(h), None) => format!("level {h}"),
            _ => "—".into(),
        };
        let temp = unit
            .temp
            .map(|t| format!("{t:.0}°C"))
            .unwrap_or_else(|| "—".into());
        let mut col = column![
            text("AMS Dryness Control").size(13),
            text(unit.dry_status_label()).size(12),
            text(format!("Humidity  {humidity}"))
                .size(11)
                .color(theme::TEXT_MUTED),
            text(format!("Temperature  {temp}"))
                .size(11)
                .color(theme::TEXT_MUTED),
            text(format!("Left Time  {}", remain.as_deref().unwrap_or("—")))
                .size(11)
                .color(theme::TEXT_MUTED),
        ]
        .spacing(6);
        if unit.supports_drying() {
            col = col.push(
                column![
                    text("Filament Drying Settings").size(12),
                    text(format!("Filament  {filament}"))
                        .size(11)
                        .color(theme::TEXT_MUTED),
                    row![
                        field("Temperature °C", &self.dry_temp, Message::AmsDryTemp),
                        field("Hours", &self.dry_hours, Message::AmsDryHours),
                    ]
                    .spacing(6),
                    checkbox(self.dry_rotate)
                        .label("Rotate spool when drying")
                        .on_toggle(Message::AmsDryRotate)
                        .style(theme::tick),
                    row![
                        quiet_btn(text("Start").size(11)).on_press(Message::AmsDryStart(unit.id)),
                        quiet_btn(text("Stop").size(11)).on_press(Message::AmsDryStop(unit.id)),
                    ]
                    .spacing(6),
                ]
                .spacing(6),
            );
        }
        col.into()
    }

    pub(crate) fn rack_pane(&self) -> Option<Element<'_, Message>> {
        if !self.machine.nozzle_rack.supported {
            return None;
        }
        let rack = &self.machine.nozzle_rack;
        let mut slots = row![].spacing(6);
        for (i, slot) in rack.toolhead.iter().enumerate() {
            let label = if i == 0 { "Toolhead L" } else { "Toolhead R" };
            slots = slots.push(nozzle_slot_card(label, slot));
        }
        for slot in &rack.rack {
            slots = slots.push(nozzle_slot_card(&format!("{}", slot.id + 1), slot));
        }
        let warn = if self.rack_pending.is_some() {
            column![
                text("Warning").size(13),
                text("The toolhead and hotend rack may move. Please keep your hands away from the chamber.")
                    .size(11)
                    .color(theme::TEXT_MUTED),
                row![
                    quiet_btn(text("OK").size(11)).on_press(Message::RackWarnConfirm),
                    quiet_btn(text("Cancel").size(11)).on_press(Message::RackWarnCancel),
                ]
                .spacing(6),
            ]
            .spacing(6)
        } else {
            column![].spacing(0)
        };
        Some(card(
            "Induction Hotend Rack",
            column![
                text(format!(
                    "{} · {}",
                    rack.status_label(),
                    rack.position_label()
                ))
                .size(12)
                .color(theme::TEXT_MUTED),
                slots.wrap(),
                row![
                    quiet_btn(text("Read All").size(11)).on_press(Message::RackReadAll),
                    quiet_btn(text("Go Home").size(11)).on_press(Message::RackMove(0)),
                    quiet_btn(text("Row A").size(11)).on_press(Message::RackMove(1)),
                    quiet_btn(text("Row B").size(11)).on_press(Message::RackMove(2)),
                    quiet_btn(text("Confirm").size(11)).on_press(Message::RackConfirmAll),
                ]
                .spacing(6)
                .wrap(),
                warn,
            ]
            .spacing(8),
        ))
    }

    fn hms_pane(&self) -> Element<'_, Message> {
        card(
            "HMS",
            column![
                text(if self.hms_lines.is_empty() {
                    "no HMS".into()
                } else {
                    self.hms_lines.join("\n")
                })
                .size(11),
                row![
                    quiet_btn("Refresh catalog").on_press(Message::RefreshHms),
                    quiet_btn("Resume").on_press(Message::HmsResume),
                    quiet_btn("Ignore").on_press(Message::HmsIgnore),
                ]
                .spacing(6),
            ]
            .spacing(8),
        )
    }

    pub(crate) fn monitor_controls(&self) -> Element<'_, Message> {
        column![self.task_pane(), self.controls_pane()]
            .spacing(10)
            .into()
    }

    pub(crate) fn device_page(&self) -> Element<'_, Message> {
        let device_labels: Vec<String> = self.cloud_devices.iter().map(|d| d.label()).collect();
        let selected_device = self
            .selected_device
            .as_ref()
            .and_then(|id| self.cloud_devices.iter().find(|d| &d.dev_id == id))
            .map(|d| d.label());
        let left = column![self.camera_pane(), self.monitor_controls()]
            .spacing(10)
            .width(Fill);
        let mut right = column![].spacing(10);
        if let Some(rack) = self.rack_pane() {
            right = right.push(rack);
        }
        right = right
            .push(self.ams_pane())
            .push(self.hms_pane())
            .push(card(
                "Connection",
                column![
                    checkbox(self.live_monitor)
                        .label("Live monitor")
                        .on_toggle(Message::LiveMonitor)
                        .style(theme::tick),
                    quiet_btn("MQTT / AMS status").on_press(Message::RefreshStatus),
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
                    .size(12)
                    .color(theme::TEXT_MUTED),
                    text(format!(
                        "Bearer {}",
                        if self.has_bearer {
                            "present"
                        } else {
                            "missing"
                        }
                    ))
                    .size(12)
                    .color(theme::TEXT_MUTED),
                    row![
                        quiet_btn("Import Studio").on_press(Message::ImportStudio),
                        quiet_btn("Extract keys").on_press(Message::ExtractKeys),
                    ]
                    .spacing(6),
                    row![
                        quiet_btn("Discover printers").on_press(Message::Discover),
                        quiet_btn("Refresh devices").on_press(Message::RefreshDevices),
                    ]
                    .spacing(6),
                    pick_list(device_labels, selected_device, Message::PickDevice)
                        .placeholder("cloud device")
                        .style(theme::choice)
                        .menu_style(theme::menu),
                    pick_list(
                        vec![SendVia::LanFtps, SendVia::CloudUpload],
                        Some(self.send_via),
                        Message::SendVia
                    )
                    .style(theme::choice)
                    .menu_style(theme::menu),
                    field("printer IP", &self.host, Message::Host),
                    field("LAN access code", &self.access_code, Message::AccessCode).secure(true),
                    field("serial (optional)", &self.serial, Message::Serial),
                    self.login_fields(),
                    checkbox(self.project_opts.bed_leveling)
                        .label("Bed level")
                        .on_toggle(Message::ProjectBedLevel)
                        .style(theme::tick),
                    checkbox(self.project_opts.flow_cali)
                        .label("Flow cali")
                        .on_toggle(Message::ProjectFlowCali)
                        .style(theme::tick),
                    checkbox(self.project_opts.vibration_cali)
                        .label("Vibration cali")
                        .on_toggle(Message::ProjectVibrationCali)
                        .style(theme::tick),
                    checkbox(self.project_opts.layer_inspect)
                        .label("Layer inspect")
                        .on_toggle(Message::ProjectLayerInspect)
                        .style(theme::tick),
                    checkbox(self.project_opts.timelapse)
                        .label("Timelapse")
                        .on_toggle(Message::ProjectTimelapse)
                        .style(theme::tick),
                ]
                .spacing(8),
            ))
            .width(360);
        scrollable(
            column![
                text("Device").size(18),
                text(format!("GPU: {}", self.adapter))
                    .size(12)
                    .color(theme::TEXT_MUTED),
                text(self.monitor_line()).size(12).color(theme::TEXT_MUTED),
                row![left, right].spacing(16),
            ]
            .spacing(10)
            .padding(16)
            .width(Fill),
        )
        .height(Fill)
        .style(theme::scroll)
        .into()
    }

    fn login_fields(&self) -> Element<'_, Message> {
        if self.has_bearer {
            return text("cloud token on disk")
                .size(11)
                .color(theme::TEXT_MUTED)
                .into();
        }
        column![
            text("Bambu cloud (OAuth)").size(13),
            quiet_btn("Sign in with Bambu").on_press(Message::CloudOAuth),
            text("or email / password")
                .size(12)
                .color(theme::TEXT_MUTED),
            field("account email", &self.login_account, Message::LoginAccount),
            field("password", &self.login_password, Message::LoginPassword).secure(true),
            field(
                "email code (if asked)",
                &self.login_code,
                Message::LoginCode
            )
            .secure(true),
            quiet_btn("Cloud login").on_press(Message::CloudLogin),
        ]
        .spacing(6)
        .into()
    }
}

fn card<'a>(title: &'a str, body: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    container(column![text(title).size(theme::TITLE_SIZE), body.into()].spacing(8))
        .padding(12)
        .width(Fill)
        .style(|_| theme::card())
        .into()
}

fn temp_chip<'a>(label: &str, actual: f32, target: f32) -> Element<'a, Message> {
    let value = if target > 0.0 {
        format!("{label} {actual:.0}/{target:.0}°C")
    } else {
        format!("{label} {actual:.0}°C")
    };
    container(text(value).size(12))
        .padding([4, 8])
        .style(|_| theme::chip())
        .into()
}

fn humidity_icon<'a>(level: Option<u8>, drying: bool) -> Element<'a, Message> {
    let filled = level.unwrap_or(0).min(4);
    let mut bars = column![].spacing(2);
    for i in (0..5).rev() {
        let on = (i as u8) <= filled;
        let color = if on {
            match i {
                0 => Color::from_rgb8(0xD0, 0x1B, 0x1B),
                1 => Color::from_rgb8(0xE6, 0x7E, 0x22),
                2 => Color::from_rgb8(0xF1, 0xC4, 0x0F),
                3 => Color::from_rgb8(0x27, 0xAE, 0x60),
                _ => Color::from_rgb8(0x1A, 0x9B, 0x8A),
            }
        } else {
            Color::from_rgb8(0xC2, 0xC2, 0xC2)
        };
        bars = bars.push(container(Space::new().width(14).height(3)).style(move |_| {
            container::Style {
                background: Some(Background::Color(color)),
                border: Border {
                    radius: 1.0.into(),
                    ..Border::default()
                },
                ..container::Style::default()
            }
        }));
    }
    let mut col = column![bars].spacing(2);
    if drying {
        col = col.push(
            text("Drying")
                .size(9)
                .color(Color::from_rgb8(0xE6, 0x7E, 0x22)),
        );
    }
    col.into()
}

pub(crate) fn ams_unit_summary(unit: &AmsUnit) -> String {
    let rh = unit
        .humidity
        .map(|h| format!("RH{h}"))
        .unwrap_or_else(|| "RH—".into());
    let pct = unit
        .humidity_percent
        .map(|p| format!("{p}%"))
        .unwrap_or_else(|| "—".into());
    let temp = unit
        .temp
        .map(|t| format!("{t:.0}°C"))
        .unwrap_or_else(|| "—".into());
    let dry = if unit.is_drying() {
        match unit.dry_time_min {
            Some(m) => format!(" · {} · {m}m left", unit.dry_status_label()),
            None => format!(" · {}", unit.dry_status_label()),
        }
    } else {
        format!(" · {}", unit.dry_status_label())
    };
    format!("{} · {rh} {pct} {temp}{dry}", unit.display_name())
}

pub(crate) fn drying_preset(filament: &str) -> (u16, u16) {
    match filament.trim().to_ascii_uppercase().as_str() {
        "PETG" => (65, 8),
        "ABS" | "ASA" => (80, 8),
        "TPU" | "TPE" => (55, 8),
        "PA" | "NYLON" | "PA-CF" => (80, 12),
        _ => (55, 8),
    }
}

fn nozzle_slot_card<'a>(label: &str, slot: &NozzleSlot) -> Element<'a, Message> {
    let heading = label.to_string();
    let dia = if slot.empty || slot.diameter <= 0.0 {
        "empty".into()
    } else {
        format!("{:.1} mm", slot.diameter)
    };
    let kind = if slot.empty {
        String::from("Empty")
    } else if slot.nozzle_type.is_empty() {
        String::from("Unknown")
    } else {
        slot.nozzle_type.clone()
    };
    container(
        column![
            text(heading).size(11),
            text(dia).size(11).color(theme::TEXT_MUTED),
            text(kind).size(11).color(theme::TEXT_MUTED),
        ]
        .spacing(2)
        .width(56),
    )
    .padding(6)
    .style(|_| theme::chip())
    .into()
}

fn tray_card(tray: &AmsTray, active: bool) -> Element<'_, Message> {
    let color = parse_tray_color(&tray.color);
    let remain = tray
        .remain
        .map(|r| format!("{r}%"))
        .unwrap_or_else(|| "—".into());
    let kind = if tray.filament_type.is_empty() {
        "Empty"
    } else {
        tray.filament_type.as_str()
    };
    let label = if tray.ams_id == 254 {
        "External Spool".to_string()
    } else {
        match tray.id {
            0 => "A".into(),
            1 => "B".into(),
            2 => "C".into(),
            3 => "D".into(),
            n => format!("T{n}"),
        }
    };
    let border = if active {
        theme::PREPARE
    } else {
        theme::CARD_BORDER
    };
    container(
        column![
            container(Space::new().width(Fill).height(22)).style(move |_| container::Style {
                background: Some(Background::Color(color)),
                border: Border {
                    color: border,
                    width: if active { 2.0 } else { 1.0 },
                    radius: 3.0.into(),
                },
                ..container::Style::default()
            }),
            text(label).size(11),
            text(kind).size(11).color(theme::TEXT_MUTED),
            text(remain).size(11).color(theme::TEXT_MUTED),
            quiet_btn(text("Load").size(11)).on_press(Message::AmsLoad {
                ams_id: tray.ams_id,
                slot_id: tray.id,
            }),
        ]
        .spacing(4)
        .width(88),
    )
    .padding(8)
    .style(|_| theme::chip())
    .into()
}

fn monitor_snapshot(st: MachineState, ams: AmsState) -> MonitorSnapshot {
    let catalog = load_cached_catalog(bambu_protocol::default_config_dir(), "en");
    let hms_lines = st
        .hms
        .iter()
        .map(|h| describe_hms(catalog.as_ref(), *h, "en"))
        .collect::<Vec<_>>();
    let trays = if ams.trays.is_empty() {
        format!("{} slots", ams.slot_count)
    } else {
        ams.trays
            .iter()
            .map(|t| {
                format!(
                    "T{} {} {}",
                    t.id,
                    t.filament_type,
                    if t.color.is_empty() { "—" } else { &t.color }
                )
            })
            .collect::<Vec<_>>()
            .join(" · ")
    };
    MonitorSnapshot {
        line: format!(
            "{} {}% L{}/{} nozzle {:.0}/{:.0}°C bed {:.0}/{:.0}°C wifi {} spd {} · AMS {trays}",
            st.gcode_state,
            st.mc_percent,
            st.layer_num,
            st.total_layer_num,
            st.nozzle_temp_c,
            st.nozzle_target_c,
            st.bed_temp_c,
            st.bed_target_c,
            st.wifi_signal,
            st.spd_lvl
        ),
        machine: st,
        ams,
        hms_lines,
    }
}

pub(crate) fn retain_ams(prev: &AmsState, next: AmsState) -> AmsState {
    if next.reports_hardware() || !prev.reports_hardware() {
        next
    } else {
        prev.clone()
    }
}

pub(crate) fn retain_machine(prev: &MachineState, mut next: MachineState) -> MachineState {
    next.nozzle_rack = retain_nozzle_rack(&prev.nozzle_rack, next.nozzle_rack);
    if next.ota_version.is_empty() && !prev.ota_version.is_empty() {
        next.ota_version = prev.ota_version.clone();
    }
    next
}

/// Empty `print.device.holder` still sets `supported`; keep the last populated rack.
pub(crate) fn retain_nozzle_rack(
    prev: &NozzleRackState,
    mut next: NozzleRackState,
) -> NozzleRackState {
    if !next.supported {
        return if prev.supported { prev.clone() } else { next };
    }
    if prev.supported {
        if next.toolhead.is_empty() && !prev.toolhead.is_empty() {
            next.toolhead = prev.toolhead.clone();
        }
        if next.rack.is_empty() && !prev.rack.is_empty() {
            next.rack = prev.rack.clone();
        }
        if next.status < 0 && prev.status >= 0 {
            next.status = prev.status;
        }
        if next.position < 0 && prev.position >= 0 {
            next.position = prev.position;
        }
    }
    next
}

pub(crate) async fn fetch_monitor(
    _send_via: SendVia,
    host: String,
    code: String,
    serial: String,
) -> Result<MonitorSnapshot, String> {
    if lan_ready(&host, &code) {
        let (st, ams) = lan_from(host, code, serial)
            .machine_and_ams()
            .await
            .map_err(|e| e.to_string())?;
        return Ok(monitor_snapshot(st, ams));
    }
    let (st, ams) = cloud_backend(&serial)?
        .machine_and_ams()
        .await
        .map_err(|e| e.to_string())?;
    Ok(monitor_snapshot(st, ams))
}

pub(crate) async fn run_cmd(
    _send_via: SendVia,
    host: String,
    code: String,
    serial: String,
    cmd: PrintCmd,
) -> Result<String, String> {
    async fn go<B: PrinterBackend>(backend: B, cmd: PrintCmd) -> Result<String, String> {
        let note = match &cmd {
            PrintCmd::Pause => "pause sent".into(),
            PrintCmd::Resume => "resume sent".into(),
            PrintCmd::Stop => "stop sent".into(),
            PrintCmd::Speed(level) => format!("print_speed {level} sent"),
            PrintCmd::Light(true) => "chamber light on".into(),
            PrintCmd::Light(false) => "chamber light off".into(),
            PrintCmd::Bed(temp) => format!("set_bed_temp {temp} sent"),
            PrintCmd::Nozzle(temp) => format!("set_nozzle_temp {temp} sent"),
            PrintCmd::Fan { index, speed } => format!("set_fan {index}/{speed} sent"),
            PrintCmd::AmsLoad {
                ams_id,
                slot_id,
                old_temp,
                new_temp,
            } => format!("ams load A{ams_id} T{slot_id} {old_temp}/{new_temp} sent"),
            PrintCmd::AmsUnload { ams_id } => format!("ams unload A{ams_id} sent"),
            PrintCmd::AmsDry { ams_id, hours, .. } => format!("ams dry A{ams_id} {hours}h sent"),
            PrintCmd::AmsDryStop { ams_id } => format!("ams dry stop A{ams_id} sent"),
            PrintCmd::RackMove(0) => "rack home sent".into(),
            PrintCmd::RackMove(1) => "rack Row A sent".into(),
            PrintCmd::RackMove(2) => "rack Row B sent".into(),
            PrintCmd::RackMove(action) => format!("rack move {action} sent"),
            PrintCmd::RackRead(_) => "rack read sent".into(),
            PrintCmd::RackConfirm(_) => "rack confirm sent".into(),
            PrintCmd::HmsResume { .. } => "hms resume sent".into(),
            PrintCmd::HmsIgnore { .. } => "hms ignore sent".into(),
        };
        match cmd {
            PrintCmd::Pause => backend.pause().await,
            PrintCmd::Resume => backend.resume().await,
            PrintCmd::Stop => backend.stop().await,
            PrintCmd::Speed(level) => backend.set_print_speed(level).await,
            PrintCmd::Light(on) => backend.set_chamber_light(on).await,
            PrintCmd::Bed(temp) => backend.set_bed_temp(temp).await,
            PrintCmd::Nozzle(temp) => backend.set_nozzle_temp(temp).await,
            PrintCmd::Fan { index, speed } => backend.set_fan(index, speed).await,
            PrintCmd::AmsLoad {
                ams_id,
                slot_id,
                old_temp,
                new_temp,
            } => backend.ams_load(ams_id, slot_id, old_temp, new_temp).await,
            PrintCmd::AmsUnload { ams_id } => backend.ams_unload(ams_id).await,
            PrintCmd::AmsDry {
                ams_id,
                ref filament,
                temp,
                hours,
                rotate,
            } => {
                backend
                    .ams_drying(ams_id, filament, temp, hours, rotate, 30)
                    .await
            }
            PrintCmd::AmsDryStop { ams_id } => backend.ams_drying_stop(ams_id).await,
            PrintCmd::RackMove(action) => backend.nozzle_holder_ctrl(action).await,
            PrintCmd::RackRead(id) => backend.holder_nozzle_refresh(id).await,
            PrintCmd::RackConfirm(id) => backend.nozzle_info_confirm(id).await,
            PrintCmd::HmsResume { ref err, ref job } => backend.hms_resume(err, job).await,
            PrintCmd::HmsIgnore { ref err, ref job } => backend.hms_ignore(err, job).await,
        }
        .map_err(|e| e.to_string())?;
        Ok(note)
    }
    if lan_ready(&host, &code) {
        return go(lan_from(host, code, serial), cmd).await;
    }
    go(cloud_backend(&serial)?, cmd).await
}

pub(crate) fn grab_chamber(host: String, code: String) -> Result<ChamberResult, String> {
    match capture_chamber(&host, &code) {
        Ok(ChamberCapture::Jpeg(jpeg)) => {
            let frame = jpeg_to_frame(&jpeg).map_err(|err| err.to_string())?;
            Ok(ChamberResult::from_frame(jpeg.len(), frame))
        }
        Ok(ChamberCapture::Frame(frame)) => {
            let bytes = frame.rgba.len();
            Ok(ChamberResult::from_frame(bytes, frame))
        }
        Ok(ChamberCapture::Rtsps { url, options }) => {
            let first = options.lines().next().unwrap_or("RTSPS");
            Ok(ChamberResult::Rtsps {
                detail: format!("chamber RTSPS {url} · {first}"),
            })
        }
        Err(err) => Err(err.to_string()),
    }
}

fn cloud_backend(serial: &str) -> Result<CloudBackend, String> {
    let dir = bambu_protocol::default_config_dir();
    let mut session = load_cloud_session(&dir).map_err(|err| err.to_string())?;
    if !serial.trim().is_empty() {
        session.serial = serial.trim().to_string();
    }
    if !session.is_ready() {
        return Err(
            "cloud MQTT needs a login and a bound printer (last device is restored from /bind)"
                .into(),
        );
    }
    Ok(CloudBackend::new(session))
}

pub(crate) fn print_speed_active(state: &str) -> bool {
    matches!(state, "RUNNING" | "PAUSE" | "PAUSED")
}

fn emit_latest(tx: &mut iced::futures::channel::mpsc::Sender<Message>, msg: Message) -> bool {
    match tx.try_send(msg) {
        Ok(()) => true,
        Err(err) if err.is_full() => true,
        Err(_) => false,
    }
}

fn camera_worker(mut job: CameraJob, mut tx: iced::futures::channel::mpsc::Sender<Message>) {
    loop {
        let lan = lan_ready(&job.host, &job.code);
        let cloud = !job.token.is_empty() && !job.serial.is_empty();
        tracing::debug!(
            target: "bambu_ui::camera",
            lan,
            cloud,
            serial_len = job.serial.len(),
            user_id_len = job.user_id.len(),
            firmware_len = job.firmware.len(),
            "camera worker tick"
        );
        if lan {
            match JpegStream::connect(&job.host, &job.code) {
                Ok(mut stream) => loop {
                    match stream.next_jpeg() {
                        Ok(jpeg) => {
                            let bytes = jpeg.len();
                            match jpeg_to_frame(&jpeg) {
                                Ok(frame) => {
                                    if !emit_latest(
                                        &mut tx,
                                        Message::ChamberShot(Ok(ChamberResult::from_frame(
                                            bytes, frame,
                                        ))),
                                    ) {
                                        return;
                                    }
                                }
                                Err(err) => {
                                    if !emit_latest(
                                        &mut tx,
                                        Message::ChamberShot(Err(err.to_string())),
                                    ) {
                                        return;
                                    }
                                    break;
                                }
                            }
                        }
                        Err(_) => break,
                    }
                },
                Err(err) => {
                    tracing::debug!(
                        target: "bambu_ui::camera",
                        error = %err,
                        "LAN JPEG :6000 failed"
                    );
                    match stream_rtsps_frames(&job.host, &job.code, |frame| {
                        let bytes = frame.rgba.len();
                        emit_latest(
                            &mut tx,
                            Message::ChamberShot(Ok(ChamberResult::from_frame(bytes, frame))),
                        )
                    }) {
                        Ok(()) => return,
                        Err(err) => {
                            tracing::debug!(
                                target: "bambu_ui::camera",
                                error = %err,
                                "LAN RTSPS :322 failed"
                            );
                        }
                    }
                }
            }
        }
        if cloud {
            if job.firmware.is_empty() {
                job.firmware = mqtt_ota_version(&job);
            }
            match cloud_tutk_loop(&job, &mut tx) {
                WorkerCtrl::Stop => return,
                WorkerCtrl::Retry => {}
                WorkerCtrl::Wait(delay) => {
                    tracing::debug!(
                        target: "bambu_ui::camera",
                        secs = delay.as_secs(),
                        "cloud camera backoff"
                    );
                    std::thread::sleep(delay);
                    continue;
                }
            }
        } else if lan {
            if !emit_latest(
                &mut tx,
                Message::ChamberShot(Err("LAN JPEG :6000 and RTSPS :322 both failed".into())),
            ) {
                return;
            }
        }
        std::thread::sleep(std::time::Duration::from_secs(2));
    }
}

enum WorkerCtrl {
    Stop,
    Retry,
    Wait(std::time::Duration),
}

fn camera_error_backoff(err: &str) -> std::time::Duration {
    if cloud_error_is_rate_limited(err) {
        std::time::Duration::from_secs(60)
    } else if err.contains("HTTP 403") || err.to_ascii_lowercase().contains("forbidden") {
        std::time::Duration::from_secs(30)
    } else {
        std::time::Duration::from_secs(2)
    }
}

fn mqtt_ota_version(job: &CameraJob) -> String {
    let session = CloudSession {
        region: job.region.clone(),
        user_id: job.user_id.clone(),
        access_token: job.token.clone(),
        refresh_token: job.refresh.clone(),
        serial: job.serial.clone(),
    };
    if !session.is_ready() {
        tracing::debug!(
            target: "bambu_ui::camera",
            "skip MQTT ota; cloud session incomplete"
        );
        return String::new();
    }
    let Ok(rt) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return String::new();
    };
    tracing::debug!(target: "bambu_ui::camera", "fetch MQTT ota for ttcode mint");
    match rt.block_on(CloudBackend::new(session).status()) {
        Ok(st) => {
            tracing::debug!(
                target: "bambu_ui::camera",
                firmware_len = st.ota_version.len(),
                "MQTT ota for camera"
            );
            st.ota_version
        }
        Err(err) => {
            tracing::debug!(
                target: "bambu_ui::camera",
                error = %err,
                "MQTT ota fetch failed"
            );
            String::new()
        }
    }
}

fn cloud_tutk_loop(
    job: &CameraJob,
    tx: &mut iced::futures::channel::mpsc::Sender<Message>,
) -> WorkerCtrl {
    tracing::debug!(
        target: "bambu_ui::camera",
        serial_len = job.serial.len(),
        firmware_len = job.firmware.len(),
        region = %job.region,
        "cloud ttcode mint"
    );
    let mut api = CloudApi::new(&job.region, &job.token, &job.refresh).with_user_id(&job.user_id);
    let firmware = if job.firmware.is_empty() {
        None
    } else {
        Some(job.firmware.as_str())
    };
    let result = stream_ttcode_frames(&mut api, &job.serial, &job.code, firmware, |frame| {
        let bytes = frame.rgba.len();
        emit_latest(
            tx,
            Message::ChamberShot(Ok(ChamberResult::from_frame(bytes, frame))),
        )
    });
    persist_refreshed_cloud(&api, job);
    match result {
        Ok(()) => WorkerCtrl::Retry,
        Err(err) => {
            let text = err.to_string();
            tracing::debug!(
                target: "bambu_ui::camera",
                error = %text,
                rate_limited = cloud_error_is_rate_limited(&text),
                "cloud camera failed"
            );
            if !emit_latest(tx, Message::ChamberShot(Err(text.clone()))) {
                return WorkerCtrl::Stop;
            }
            if cloud_error_is_rate_limited(&text) || text.contains("HTTP 403") {
                WorkerCtrl::Wait(camera_error_backoff(&text))
            } else {
                WorkerCtrl::Retry
            }
        }
    }
}

fn persist_refreshed_cloud(api: &CloudApi, job: &CameraJob) {
    let token_changed = !api.access_token.is_empty() && api.access_token != job.token;
    let uid_filled = !api.user_id.is_empty() && api.user_id != job.user_id;
    if !token_changed && !uid_filled {
        return;
    }
    let dir = default_config_dir();
    let Ok(mut session) = load_cloud_session(&dir) else {
        return;
    };
    session.access_token = api.access_token.clone();
    if !api.refresh_token.is_empty() {
        session.refresh_token = api.refresh_token.clone();
    }
    if !api.user_id.is_empty() {
        session.user_id = api.user_id.clone();
    }
    let _ = save_cloud_session(&dir, &session);
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) struct CameraJob {
    pub host: String,
    pub code: String,
    pub serial: String,
    pub region: String,
    pub token: String,
    pub refresh: String,
    pub user_id: String,
    pub firmware: String,
}

pub(crate) fn camera_frames(job: CameraJob) -> impl iced::futures::Stream<Item = Message> {
    iced::stream::channel(4, async move |output| {
        let _ = std::thread::Builder::new()
            .name("bambu-camera".into())
            .spawn(move || camera_worker(job, output));
        std::future::pending::<()>().await;
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_tray_color_rrggbb() {
        let c = parse_tray_color("00AE42FF");
        assert!((c.r - 0.0).abs() < 0.01);
        assert!(c.g > 0.6);
    }

    #[test]
    fn lan_ready_needs_host_and_code() {
        assert!(lan_ready("192.168.1.9", "12345678"));
        assert!(!lan_ready("", "12345678"));
        assert!(!lan_ready("192.168.1.9", ""));
    }

    #[test]
    fn camera_backoff_is_long_on_cloudflare_1015() {
        assert_eq!(
            camera_error_backoff("ttcode: cloud HTTP 429: error code: 1015").as_secs(),
            60
        );
        assert_eq!(
            camera_error_backoff("ttcode: cloud HTTP 403: error code 8: forbidden").as_secs(),
            30
        );
        assert_eq!(camera_error_backoff("LAN JPEG :6000 failed").as_secs(), 2);
    }

    #[test]
    fn print_speed_active_only_while_printing() {
        assert!(print_speed_active("RUNNING"));
        assert!(print_speed_active("PAUSE"));
        assert!(print_speed_active("PAUSED"));
        assert!(!print_speed_active("IDLE"));
        assert!(!print_speed_active("FINISH"));
        assert!(!print_speed_active(""));
    }

    #[test]
    fn ams_unit_summary_shows_two_distinct_readings() {
        let a = AmsUnit {
            id: 0,
            humidity: Some(2),
            humidity_percent: Some(28),
            temp: Some(32.5),
            dry_time_min: Some(90),
            dry_status: 2,
            ams_type: 3,
            ..Default::default()
        };
        let b = AmsUnit {
            id: 1,
            humidity: Some(4),
            humidity_percent: Some(55),
            temp: Some(27.0),
            dry_time_min: None,
            dry_status: 0,
            ams_type: 3,
            ..Default::default()
        };
        let sa = ams_unit_summary(&a);
        let sb = ams_unit_summary(&b);
        assert!(sa.contains("AMS 2 Pro(1)"));
        assert!(sa.contains("RH2"));
        assert!(sa.contains("32°C") || sa.contains("33°C"));
        assert!(sa.contains("Drying"));
        assert!(sb.contains("AMS 2 Pro(2)"));
        assert!(sb.contains("RH4"));
        assert!(sb.contains("27°C"));
        assert!(sb.contains("Idle"));
        assert!(!sb.contains("Drying"));
        assert_ne!(sa, sb);
    }

    #[test]
    fn retain_ams_keeps_last_hardware() {
        let prev = AmsState {
            trays: vec![AmsTray {
                id: 0,
                filament_type: "PLA".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let kept = retain_ams(&prev, AmsState::default());
        assert_eq!(kept.trays.len(), 1);
        let next = AmsState {
            trays: vec![AmsTray {
                id: 1,
                filament_type: "PETG".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let updated = retain_ams(&prev, next);
        assert_eq!(updated.trays[0].filament_type, "PETG");
    }

    #[test]
    fn retain_nozzle_rack_keeps_slots_on_empty_holder() {
        let prev = NozzleRackState {
            supported: true,
            status: 0,
            position: 1,
            toolhead: vec![NozzleSlot {
                id: 0,
                diameter: 0.4,
                ..Default::default()
            }],
            rack: vec![NozzleSlot {
                id: 0,
                diameter: 0.2,
                ..Default::default()
            }],
            ..Default::default()
        };
        let mut next = NozzleRackState {
            supported: true,
            ..Default::default()
        };
        next.status = -1;
        next.position = -1;
        let kept = retain_nozzle_rack(&prev, next);
        assert_eq!(kept.toolhead.len(), 1);
        assert_eq!(kept.rack.len(), 1);
        assert_eq!(kept.status, 0);
        assert_eq!(kept.position, 1);
        assert!(kept.supported);
        let omitted = retain_nozzle_rack(&prev, NozzleRackState::default());
        assert_eq!(omitted.toolhead.len(), 1);
        assert!(omitted.supported);
    }
}
