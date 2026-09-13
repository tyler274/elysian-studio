//! Live monitor chrome: speed/light, AMS chips, P1/A1 JPEG thumbnail.

use iced::advanced::layout::{self, Node};
use iced::advanced::renderer::{self, Quad};
use iced::advanced::widget::{Tree, Widget};
use iced::advanced::{Layout, Renderer};
use iced::mouse;
use iced::widget::{button, column, row, text};
use iced::{Background, Border, Color, Element, Length, Rectangle, Size};

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
            "{} · {}% · L{}/{} · {}m · nozzle {:.0}°C{humidity}",
            if self.machine.gcode_state.is_empty() {
                "—"
            } else {
                self.machine.gcode_state.as_str()
            },
            self.machine.mc_percent,
            self.machine.layer_num,
            self.machine.total_layer_num,
            self.machine.mc_remaining_time_min,
            self.machine.nozzle_temp_c
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
            text("Chamber").size(13),
            thumb,
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
            "{} {}% L{}/{} nozzle {:.0}°C · AMS {trays}",
            st.gcode_state, st.mc_percent, st.layer_num, st.total_layer_num, st.nozzle_temp_c
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
        }
        .map_err(|e| e.to_string())?;
        Ok(match cmd {
            PrintCmd::Pause => "pause sent".into(),
            PrintCmd::Resume => "resume sent".into(),
            PrintCmd::Stop => "stop sent".into(),
            PrintCmd::Speed(level) => format!("print_speed {level} sent"),
            PrintCmd::Light(true) => "chamber light on".into(),
            PrintCmd::Light(false) => "chamber light off".into(),
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
