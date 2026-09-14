//! Live monitor chrome: speed/light, AMS chips, P1/A1 JPEG thumbnail.

use iced::advanced::layout::{self, Node};
use iced::advanced::renderer::{self, Quad};
use iced::advanced::widget::{Tree, Widget};
use iced::advanced::{Layout, Renderer};
use iced::mouse;
use iced::widget::{
    button, checkbox, column, pick_list, row, scrollable, slider, text, text_input,
};
use iced::{Background, Border, Color, Element, Fill, Length, Rectangle, Size};

use bambu_device::{Frame, PrinterBackend};
use bambu_protocol::{
    capture_chamber, describe_hms, jpeg_to_frame, load_cached_catalog, ChamberCapture, CloudBackend,
};

use crate::{lan_from, ChamberResult, Message, MonitorSnapshot, PrintCmd, SendVia};

const THUMB_W: u32 = 80;
const THUMB_H_MAX: u32 = 56;

#[derive(Debug, Clone)]
pub(crate) struct JpegThumb {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pixels: Vec<[u8; 4]>,
}

impl JpegThumb {
    pub(crate) fn from_frame(frame: &Frame) -> Self {
        let width = THUMB_W.min(frame.width.max(1));
        let height = ((u64::from(width) * u64::from(frame.height.max(1)))
            / u64::from(frame.width.max(1)))
        .max(1)
        .min(u64::from(THUMB_H_MAX)) as u32;
        let mut pixels = Vec::with_capacity((width * height) as usize);
        for y in 0..height {
            let sy = y * frame.height / height;
            for x in 0..width {
                let sx = x * frame.width / width;
                let i = ((sy * frame.width + sx) * 4) as usize;
                let px = frame
                    .rgba
                    .get(i..i + 4)
                    .and_then(|s| <[u8; 4]>::try_from(s).ok())
                    .unwrap_or([40, 40, 48, 255]);
                pixels.push(px);
            }
        }
        Self {
            width,
            height,
            pixels,
        }
    }
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for JpegThumb {
    fn size(&self) -> Size<Length> {
        Size::new(
            Length::Fixed(self.width as f32),
            Length::Fixed(self.height as f32),
        )
    }

    fn layout(
        &mut self,
        _tree: &mut Tree,
        _renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) -> Node {
        layout::atomic(
            limits,
            Length::Fixed(self.width as f32),
            Length::Fixed(self.height as f32),
        )
    }

    fn draw(
        &self,
        _tree: &Tree,
        renderer: &mut iced::Renderer,
        _theme: &iced::Theme,
        _style: &renderer::Style,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();
        let cw = self.width.max(1) as f32;
        let ch = self.height.max(1) as f32;
        let pw = bounds.width / cw;
        let ph = bounds.height / ch;
        for y in 0..self.height {
            for x in 0..self.width {
                let i = (y * self.width + x) as usize;
                let Some([r, g, b, a]) = self.pixels.get(i).copied() else {
                    continue;
                };
                renderer.fill_quad(
                    Quad {
                        bounds: Rectangle {
                            x: bounds.x + x as f32 * pw,
                            y: bounds.y + y as f32 * ph,
                            width: pw.max(1.0),
                            height: ph.max(1.0),
                        },
                        border: Border::default(),
                        shadow: iced::Shadow::default(),
                        snap: true,
                    },
                    Background::Color(Color::from_rgba8(r, g, b, f32::from(a) / 255.0)),
                );
            }
        }
    }
}

impl<'a> From<JpegThumb> for Element<'a, Message> {
    fn from(thumb: JpegThumb) -> Self {
        Element::new(thumb)
    }
}

impl crate::App {
    pub(crate) fn ams_chips(&self) -> Element<'_, Message> {
        let mut r = row![];
        if self.settings.filament_map.is_empty() {
            r = r.push(text("AMS map: —").size(11));
        }
        for (i, mapped) in self.settings.filament_map.iter().enumerate() {
            r = r.push(
                button(text(format!("F{}→T{mapped}", i + 1)).size(11))
                    .on_press(Message::CycleAmsMap(i)),
            );
        }
        r.spacing(4).into()
    }

    pub(crate) fn ams_load_row(&self) -> Element<'_, Message> {
        let mut r = row![];
        if self.ams.trays.is_empty() && self.ams.vt_tray.is_none() {
            r = r.push(text("AMS load: —").size(11));
        }
        let mut unloaded = Vec::new();
        for tray in &self.ams.trays {
            let ams_id = tray.ams_id;
            let slot_id = tray.id;
            r = r.push(
                button(text(format!("Load T{slot_id}")).size(11))
                    .on_press(Message::AmsLoad { ams_id, slot_id }),
            );
            if !unloaded.contains(&ams_id) {
                unloaded.push(ams_id);
                r = r.push(
                    button(text(format!("Unload A{ams_id}")).size(11))
                        .on_press(Message::AmsUnload { ams_id }),
                );
            }
        }
        if let Some(vt) = &self.ams.vt_tray {
            r = r.push(
                button(text("Load ext").size(11)).on_press(Message::AmsLoad {
                    ams_id: vt.ams_id,
                    slot_id: 0,
                }),
            );
        }
        r.spacing(4).into()
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

    pub(crate) fn monitor_controls(&self) -> Element<'_, Message> {
        let thumb: Element<'_, Message> = if let Some(thumb) = &self.chamber_thumb {
            thumb.clone().into()
        } else if !self.camera_note.is_empty() {
            text(self.camera_note.as_str()).size(11).into()
        } else {
            text("P1/A1 JPEG thumbnail (X1/H2: RTSPS text)")
                .size(11)
                .into()
        };
        column![
            text(self.monitor_line()).size(12),
            self.ams_chips(),
            self.ams_load_row(),
            row![
                button("Pause").on_press(Message::Pause),
                button("Resume").on_press(Message::Resume),
                button("Stop").on_press(Message::Stop),
            ]
            .spacing(6),
            text("Print speed").size(13),
            row![
                button("1").on_press(Message::PrintSpeed(1)),
                button("2").on_press(Message::PrintSpeed(2)),
                button("3").on_press(Message::PrintSpeed(3)),
                button("4").on_press(Message::PrintSpeed(4)),
            ]
            .spacing(4),
            row![
                button("Light on").on_press(Message::ChamberLight(true)),
                button("Light off").on_press(Message::ChamberLight(false)),
            ]
            .spacing(6),
            text("Bed / nozzle °C").size(13),
            row![
                text_input("bed", &self.control_bed).on_input(Message::BedSet),
                button("Set bed").on_press(Message::SendBed),
            ]
            .spacing(4),
            row![
                text_input("nozzle", &self.control_nozzle).on_input(Message::NozzleSet),
                button("Set nozzle").on_press(Message::SendNozzle),
            ]
            .spacing(4),
            text(format!("Cooling fan {}", self.control_fan)).size(13),
            slider(0.0..=255.0, f64::from(self.control_fan), Message::FanSet).step(1.0),
            button("Set fan").on_press(Message::SendFan),
            text("Chamber").size(13),
            thumb,
        ]
        .spacing(6)
        .into()
    }

    pub(crate) fn device_page(&self) -> Element<'_, Message> {
        let device_labels: Vec<String> = self.cloud_devices.iter().map(|d| d.label()).collect();
        let selected_device = self
            .selected_device
            .as_ref()
            .and_then(|id| self.cloud_devices.iter().find(|d| &d.dev_id == id))
            .map(|d| d.label());
        scrollable(
            column![
                text("Device").size(18),
                text(format!("GPU: {}", self.adapter)).size(12),
                self.monitor_controls(),
                text("HMS").size(16),
                button("Refresh HMS catalog").on_press(Message::RefreshHms),
                text(if self.hms_lines.is_empty() {
                    "no HMS".into()
                } else {
                    self.hms_lines.join("\n")
                })
                .size(11),
                row![
                    button("HMS resume").on_press(Message::HmsResume),
                    button("HMS ignore").on_press(Message::HmsIgnore),
                ]
                .spacing(6),
                checkbox(self.live_monitor)
                    .label("Live monitor")
                    .on_toggle(Message::LiveMonitor),
                button("MQTT / AMS status").on_press(Message::RefreshStatus),
                text("Account / LAN").size(16),
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
                button("Extract keys").on_press(Message::ExtractKeys),
                button("Discover printers").on_press(Message::Discover),
                button("Refresh devices").on_press(Message::RefreshDevices),
                pick_list(device_labels, selected_device, Message::PickDevice)
                    .placeholder("cloud device"),
                pick_list(
                    vec![SendVia::LanFtps, SendVia::CloudUpload],
                    Some(self.send_via),
                    Message::SendVia
                ),
                text_input("printer IP", &self.host).on_input(Message::Host),
                text_input("LAN access code", &self.access_code)
                    .secure(true)
                    .on_input(Message::AccessCode),
                text_input("serial (optional)", &self.serial).on_input(Message::Serial),
                self.login_fields(),
                checkbox(self.project_opts.bed_leveling)
                    .label("Bed level")
                    .on_toggle(Message::ProjectBedLevel),
                checkbox(self.project_opts.flow_cali)
                    .label("Flow cali")
                    .on_toggle(Message::ProjectFlowCali),
                checkbox(self.project_opts.vibration_cali)
                    .label("Vibration cali")
                    .on_toggle(Message::ProjectVibrationCali),
                checkbox(self.project_opts.layer_inspect)
                    .label("Layer inspect")
                    .on_toggle(Message::ProjectLayerInspect),
                checkbox(self.project_opts.timelapse)
                    .label("Timelapse")
                    .on_toggle(Message::ProjectTimelapse),
                button("Chamber / RTSPS live").on_press(Message::Chamber),
            ]
            .spacing(8)
            .padding(16)
            .width(480),
        )
        .height(Fill)
        .into()
    }

    fn login_fields(&self) -> Element<'_, Message> {
        if self.has_bearer {
            return text("cloud token on disk").size(11).into();
        }
        column![
            text("Bambu cloud (OAuth)").size(13),
            button("Sign in with Bambu").on_press(Message::CloudOAuth),
            text("or email / password").size(12),
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
    send_via: SendVia,
    host: String,
    code: String,
    serial: String,
) -> Result<MonitorSnapshot, String> {
    if send_via == SendVia::CloudUpload {
        let backend = CloudBackend::from_config_dir(bambu_protocol::default_config_dir())
            .map_err(|e| e.to_string())?;
        return snapshot_backend(backend).await;
    }
    if host.is_empty() || code.is_empty() {
        return Err("printer IP and LAN access code required".into());
    }
    snapshot_backend(lan_from(host, code, serial)).await
}

pub(crate) async fn run_cmd(
    send_via: SendVia,
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
    if send_via == SendVia::CloudUpload {
        let backend = CloudBackend::from_config_dir(bambu_protocol::default_config_dir())
            .map_err(|e| e.to_string())?;
        return go(backend, cmd).await;
    }
    if host.is_empty() || code.is_empty() {
        return Err("printer IP and LAN access code required".into());
    }
    go(lan_from(host, code, serial), cmd).await
}

pub(crate) fn grab_chamber(host: String, code: String) -> Result<ChamberResult, String> {
    match capture_chamber(&host, &code) {
        Ok(ChamberCapture::Jpeg(jpeg)) => {
            let frame = jpeg_to_frame(&jpeg).map_err(|err| err.to_string())?;
            Ok(ChamberResult::Jpeg {
                bytes: jpeg.len(),
                thumb: JpegThumb::from_frame(&frame),
            })
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
