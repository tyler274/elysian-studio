//! Device StatusPanel: camera stream, AMS trays, task / temps, HMS, account.

use iced::widget::{
    button, checkbox, column, container, image, pick_list, progress_bar, row, scrollable, slider,
    text, text_input, Space,
};
use iced::{Alignment, Background, Border, Color, ContentFit, Element, Fill};

use bambu_device::{AmsTray, PrinterBackend};
use bambu_protocol::{
    capture_chamber, describe_hms, describe_rtsps, jpeg_to_frame, load_cached_catalog,
    ChamberCapture, CloudBackend, JpegStream,
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
            r = r.push(text("AMS map: —").size(11).color(theme::TEXT_MUTED));
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
            r = r.push(text("AMS load: —").size(11).color(theme::TEXT_MUTED));
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
                    quiet_btn(text(format!("Unload A{ams_id}")).size(11))
                        .on_press(Message::AmsUnload { ams_id }),
                );
            }
        }
        if let Some(vt) = &self.ams.vt_tray {
            r = r.push(
                quiet_btn(text("Load ext").size(11)).on_press(Message::AmsLoad {
                    ams_id: vt.ams_id,
                    slot_id: 0,
                }),
            );
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
            text("Chamber camera — P1/A1 JPEG :6000 stream, X1/H2 RTSPS :322")
                .size(12)
                .color(theme::TEXT_MUTED)
                .into()
        };
        let size = if self.chamber_width > 0 {
            format!("{}×{}", self.chamber_width, self.chamber_height)
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
                    quiet_btn(text("Grab frame").size(12)).on_press(Message::Chamber),
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
        card(
            "Controls",
            column![
                row![
                    quiet_btn("Pause").on_press(Message::Pause),
                    quiet_btn("Resume").on_press(Message::Resume),
                    quiet_btn("Stop").on_press(Message::Stop),
                ]
                .spacing(6),
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

    fn ams_pane(&self) -> Element<'_, Message> {
        let mut trays = row![].spacing(8);
        if self.ams.trays.is_empty() && self.ams.vt_tray.is_none() {
            trays = trays.push(
                text("No AMS trays in last push_status")
                    .size(12)
                    .color(theme::TEXT_MUTED),
            );
        }
        for tray in &self.ams.trays {
            let active = self.ams.active_slot == Some(tray.id);
            trays = trays.push(tray_card(tray, active));
        }
        if let Some(vt) = &self.ams.vt_tray {
            trays = trays.push(tray_card(vt, false));
        }
        let trays = trays.wrap();
        let humidity = self
            .ams
            .humidity
            .map(|h| format!("humidity {h}"))
            .unwrap_or_else(|| "humidity —".into());
        card(
            "AMS",
            column![
                text(humidity).size(12).color(theme::TEXT_MUTED),
                trays,
                self.ams_chips(),
                self.ams_load_row(),
            ]
            .spacing(8),
        )
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
        let right = column![
            self.ams_pane(),
            self.hms_pane(),
            card(
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
            ),
        ]
        .spacing(10)
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

fn tray_card(tray: &AmsTray, active: bool) -> Element<'_, Message> {
    let color = parse_tray_color(&tray.color);
    let remain = tray
        .remain
        .map(|r| format!("{r}%"))
        .unwrap_or_else(|| "—".into());
    let kind = if tray.filament_type.is_empty() {
        "empty"
    } else {
        tray.filament_type.as_str()
    };
    let label = if tray.ams_id == 254 {
        "Ext".to_string()
    } else {
        format!("A{} T{}", tray.ams_id, tray.id)
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

pub(crate) async fn snapshot_backend<B: PrinterBackend>(
    backend: B,
) -> Result<MonitorSnapshot, String> {
    let st = backend.status().await.map_err(|e| e.to_string())?;
    let ams = backend.ams().await.unwrap_or_default();
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
    Ok(MonitorSnapshot {
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
    })
}

pub(crate) async fn fetch_monitor(
    _send_via: SendVia,
    host: String,
    code: String,
    serial: String,
) -> Result<MonitorSnapshot, String> {
    if lan_ready(&host, &code) {
        return snapshot_backend(lan_from(host, code, serial)).await;
    }
    let backend = CloudBackend::from_config_dir(bambu_protocol::default_config_dir())
        .map_err(|e| e.to_string())?;
    snapshot_backend(backend).await
}

pub(crate) async fn run_cmd(
    _send_via: SendVia,
    host: String,
    code: String,
    serial: String,
    cmd: PrintCmd,
) -> Result<String, String> {
    async fn go<B: PrinterBackend>(backend: B, cmd: PrintCmd) -> Result<String, String> {
        match cmd {
            PrintCmd::Pause => backend.pause().await,
            PrintCmd::Resume => backend.resume().await,
            PrintCmd::Stop => backend.stop().await,
            PrintCmd::Speed(level) => backend.set_print_speed(level).await,
            PrintCmd::Light(on) => backend.set_chamber_light(on).await,
            PrintCmd::Bed(temp) => backend.set_bed_temp(temp).await,
            PrintCmd::Nozzle(temp) => backend.set_nozzle_temp(temp).await,
            PrintCmd::Fan { index, speed } => backend.set_fan(index, speed).await,
            PrintCmd::AmsLoad { ams_id, slot_id } => {
                backend.ams_load(ams_id, slot_id, 220, 220).await
            }
            PrintCmd::AmsUnload { ams_id } => backend.ams_unload(ams_id).await,
            PrintCmd::HmsResume { ref err, ref job } => backend.hms_resume(err, job).await,
            PrintCmd::HmsIgnore { ref err, ref job } => backend.hms_ignore(err, job).await,
        }
        .map_err(|e| e.to_string())?;
        Ok(match cmd {
            PrintCmd::Pause => "pause sent".into(),
            PrintCmd::Resume => "resume sent".into(),
            PrintCmd::Stop => "stop sent".into(),
            PrintCmd::Speed(level) => format!("print_speed {level} sent"),
            PrintCmd::Light(true) => "chamber light on".into(),
            PrintCmd::Light(false) => "chamber light off".into(),
            PrintCmd::Bed(temp) => format!("set_bed_temp {temp} sent"),
            PrintCmd::Nozzle(temp) => format!("set_nozzle_temp {temp} sent"),
            PrintCmd::Fan { index, speed } => format!("set_fan {index}/{speed} sent"),
            PrintCmd::AmsLoad { ams_id, slot_id } => {
                format!("ams load A{ams_id} T{slot_id} sent")
            }
            PrintCmd::AmsUnload { ams_id } => format!("ams unload A{ams_id} sent"),
            PrintCmd::HmsResume { .. } => "hms resume sent".into(),
            PrintCmd::HmsIgnore { .. } => "hms ignore sent".into(),
        })
    }
    if lan_ready(&host, &code) {
        return go(lan_from(host, code, serial), cmd).await;
    }
    let backend = CloudBackend::from_config_dir(bambu_protocol::default_config_dir())
        .map_err(|e| e.to_string())?;
    go(backend, cmd).await
}

pub(crate) fn grab_chamber(host: String, code: String) -> Result<ChamberResult, String> {
    match capture_chamber(&host, &code) {
        Ok(ChamberCapture::Jpeg(jpeg)) => {
            let frame = jpeg_to_frame(&jpeg).map_err(|err| err.to_string())?;
            Ok(ChamberResult::from_frame(jpeg.len(), frame))
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

fn emit(tx: &mut iced::futures::channel::mpsc::Sender<Message>, msg: Message) -> bool {
    match tx.try_send(msg) {
        Ok(()) => true,
        Err(err) if err.is_full() => true,
        Err(_) => false,
    }
}

fn camera_worker(
    host: String,
    code: String,
    mut tx: iced::futures::channel::mpsc::Sender<Message>,
) {
    loop {
        match JpegStream::connect(&host, &code) {
            Ok(mut stream) => loop {
                match stream.next_jpeg() {
                    Ok(jpeg) => {
                        let bytes = jpeg.len();
                        match jpeg_to_frame(&jpeg) {
                            Ok(frame) => {
                                if !emit(
                                    &mut tx,
                                    Message::ChamberShot(Ok(ChamberResult::from_frame(
                                        bytes, frame,
                                    ))),
                                ) {
                                    return;
                                }
                            }
                            Err(err) => {
                                if !emit(&mut tx, Message::ChamberShot(Err(err.to_string()))) {
                                    return;
                                }
                                break;
                            }
                        }
                    }
                    Err(err) => {
                        if err.to_string().contains("implausible JPEG") {
                            match describe_rtsps(&host, &code) {
                                Ok(live) => {
                                    let first = live.sdp.lines().next().unwrap_or("RTSPS");
                                    let _ = emit(
                                        &mut tx,
                                        Message::ChamberShot(Ok(ChamberResult::Rtsps {
                                            detail: format!("chamber RTSPS {} · {first}", live.url),
                                        })),
                                    );
                                    return;
                                }
                                Err(rtsps_err) => {
                                    if !emit(
                                        &mut tx,
                                        Message::ChamberShot(Err(format!(
                                            "{err}; RTSPS: {rtsps_err}"
                                        ))),
                                    ) {
                                        return;
                                    }
                                }
                            }
                        } else if !emit(&mut tx, Message::ChamberShot(Err(err.to_string()))) {
                            return;
                        }
                        break;
                    }
                }
            },
            Err(err) => match describe_rtsps(&host, &code) {
                Ok(live) => {
                    let first = live.sdp.lines().next().unwrap_or("RTSPS");
                    let _ = emit(
                        &mut tx,
                        Message::ChamberShot(Ok(ChamberResult::Rtsps {
                            detail: format!("chamber RTSPS {} · {first}", live.url),
                        })),
                    );
                    return;
                }
                Err(rtsps_err) => {
                    if !emit(
                        &mut tx,
                        Message::ChamberShot(Err(format!("{err}; RTSPS: {rtsps_err}"))),
                    ) {
                        return;
                    }
                }
            },
        }
        std::thread::sleep(std::time::Duration::from_secs(2));
    }
}

pub(crate) fn camera_frames(
    host: String,
    code: String,
) -> impl iced::futures::Stream<Item = Message> {
    iced::stream::channel(1, async move |output| {
        let _ = std::thread::Builder::new()
            .name("bambu-camera".into())
            .spawn(move || camera_worker(host, code, output));
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
}
